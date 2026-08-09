//! The capture notification: the one thing a *successful* capture ever shows,
//! and only when the user could not have watched it land.
//!
//! `notice` states the rule this qualifies — success is silent, no window, no
//! sound, no toast — and the qualification is narrow on purpose. A capture that
//! arrives while the panel is on screen still says nothing: the note appears in
//! the list on its own through the store's change event, and a toast on top of
//! that is noise about something the user is already looking at. A capture that
//! arrives while the panel is hidden produces nothing visible at all, which is
//! the whole point of a global capture and also the whole problem with it.
//!
//! # Why this is hand-written WinRT and not a plugin
//!
//! `tauri-plugin-notification`'s desktop backend builds a `notify_rust`
//! notification out of five fields and shows it. There is no action field, no
//! button mapping and no activation callback anywhere in its desktop path —
//! actions exist only under `cfg(target_os = "android")` / `"ios"`. Since the
//! buttons *are* this feature, the plugin is not a candidate.
//!
//! # The two halves Windows makes hard, and why neither bites here
//!
//! **Activation.** `ToastNotification.Activated` delivers both a body click and a
//! foreground action-button click to a *running* process, and
//! `ToastActivatedEventArgs::Arguments()` returns the `launch` string for the
//! first and the button's own `arguments` for the second — so one parser over one
//! string covers both. The `INotificationActivationCallback` COM server everyone
//! remembers as "toasts are hard" exists solely to *relaunch* a process that is no
//! longer running, which task-018 puts out of scope. Nothing in the registry is
//! touched.
//!
//! **The AUMID.** An unpackaged desktop app must own a Start Menu shortcut
//! carrying `System.AppUserModel.ID`, and `CreateToastNotifierWithId` on an
//! unregistered one fails *silently* — `Show()` returns success and no toast ever
//! appears. Tauri's NSIS template already stamps the bundle identifier onto the
//! shortcuts it installs, so an installed build needs no configuration. **A `tauri
//! dev` build owns no such shortcut, so every toast it fires is dropped.** That is
//! expected and must not be mistaken for a defect; this feature is verifiable only
//! from an installed build.

use std::sync::{Arc, Mutex, Once};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use windows::core::{IInspectable, Interface, Ref, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
	ToastActivatedEventArgs, ToastFailedEventArgs, ToastNotification, ToastNotificationManager,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

use crate::diagnostics;
use crate::panel;
use copper_core::store::{self, Landed, SectionRef, SharedStore};

use super::{notice::NoticeController, on_main_thread, CaptureFailure};

/// How much of the captured text the toast carries.
///
/// The first line rather than the first hundred characters of the whole
/// selection: a multi-line capture's first line is what the user would call it,
/// and running two lines together with the newline collapsed reads as neither.
const SNIPPET_CHARS: usize = 100;

/// How many alternative sections get a button.
///
/// Windows caps a toast at five actions in total and silently drops the rest.
/// Four leaves the fifth slot unclaimed rather than spending the whole budget on
/// one feature, and it is already more choices than a notification should be
/// asking someone to read.
const MAX_ACTIONS: usize = 4;

/// How many unattended toasts keep working.
///
/// A `ToastNotification` whose Rust value is dropped loses its `Activated`
/// handler while the toast is still on screen — the classic way to ship a
/// notification whose buttons do nothing — so every live one is held here. The
/// bound is what stops a machine left alone overnight from accumulating them
/// without limit; past it the oldest toast keeps its text and loses its buttons,
/// which is the least bad thing to give up.
const LIVE_TOASTS: usize = 16;

/// The toasts whose handlers are still wanted, each keyed by the note it
/// announces.
///
/// A module static rather than managed state, like `panel`'s pin mirror: nothing
/// outside this module has any business reaching it, and the `Activated` handler
/// that prunes it runs on a WinRT-owned thread with no `AppHandle` of its own to
/// resolve state through.
static LIVE: Mutex<Vec<(String, ToastNotification)>> = Mutex::new(Vec::new());

/// What a toast activation asked for.
///
/// One enum over one string, because Windows hands body clicks and button clicks
/// to the same handler through the same `Arguments()` call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Activation {
	/// The body was clicked: show the user the note.
	Reveal { note: String },
	/// A button was clicked: file the note elsewhere and stay out of the way.
	///
	/// **Names the space as well as the note**, because this one *writes*. A toast
	/// stays live in the Action Center long after the space it was fired for was
	/// switched away from, and a move addressed by note id alone would then be
	/// applied to whatever document happens to be open — or, more usually, refused
	/// for an unknown id with nobody told. The reveal below needs no such thing:
	/// it asks the panel to scroll to a row, and a row that is not in the list is
	/// simply never found.
	Reroute {
		space: String,
		note: String,
		section: String,
	},
}

const REVEAL_VERB: &str = "note:";
const REROUTE_VERB: &str = "move:";

/// Tells the panel which note a toast body click was about.
///
/// The frontend arms its own reveal request against it, which is held until the
/// list has somewhere to scroll — the same request a capture arms, and the reason
/// this is an event rather than a scroll performed from here.
const REVEAL_EVENT: &str = "capture://reveal";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RevealPayload {
	note: String,
}

/// Announces a capture, if the user asked to be told and could not have seen it
/// land.
///
/// Called from `worker::route`, which runs strictly after the store's commit
/// point — so nothing below can block, delay or corrupt the capture it is
/// describing, whatever WinRT does. Every failure is logged and dropped.
pub fn announce(app: &AppHandle, landed: &Landed, snippet: &str) {
	if !landed.notify {
		return;
	}

	let landed = landed.clone();
	let snippet = snippet.to_owned();
	let handle = app.clone();
	// The visibility test belongs on the main thread with the window call it is,
	// and so does everything after it: this is where the app's COM apartment lives
	// and where every other window-touching effect in `capture` is already
	// marshalled.
	on_main_thread(app, "announce a capture", move || {
		let Some(window) = handle.get_webview_window(panel::PANEL_LABEL) else {
			diagnostics::log_error(
				"[copper] capture: the panel window is gone; no capture notification shown",
			);
			return;
		};
		// **Visible, not focused.** A capture can never happen while Copper is the
		// foreground window — `worker::run_cascade` refuses one outright — so
		// "Copper is not in front" is unconditionally true here and would fire a
		// toast every single time. What actually varies is whether the panel is on
		// screen: it is frequently up and unfocused, and there the note simply
		// appears in the list.
		if panel::is_visible(&window) {
			return;
		}
		if let Err(err) = show(&handle, &landed, &snippet) {
			diagnostics::log_error(&format!(
				"[copper] capture: could not show the capture notification: {err}"
			));
		}
	});
}

/// Builds the toast, wires its handlers, and hands it to Windows.
fn show(app: &AppHandle, landed: &Landed, snippet: &str) -> windows::core::Result<()> {
	initialise_apartment();

	let xml = XmlDocument::new()?;
	xml.LoadXml(&HSTRING::from(document(landed, snippet)))?;
	let toast = ToastNotification::CreateToastNotification(&xml)?;

	let activated_app = app.clone();
	toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
		move |_, args: Ref<'_, IInspectable>| {
			// Both the body's `launch` and a button's `arguments` arrive here, in the
			// same string.
			let arguments = args.ok()?.cast::<ToastActivatedEventArgs>()?.Arguments()?;
			activated(&activated_app, &arguments.to_string());
			Ok(())
		},
	))?;

	let failed_note = landed.note.clone();
	toast.Failed(&TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(
		move |_, _| {
			// A toast Windows refused can never be activated, so nothing is served by
			// keeping it alive. `Dismissed` is deliberately *not* wired the same way:
			// a dismissed toast moves into the Action Center and stays clickable
			// there, and retiring it would leave the user a notification whose buttons
			// have quietly stopped working.
			retire(&failed_note);
			Ok(())
		},
	))?;

	retain(&landed.note, &toast);
	// The identifier verbatim: it is what Tauri's installer stamps onto the Start
	// Menu shortcut as `System.AppUserModel.ID`, and reading it from the config
	// keeps the two from drifting apart in a rename.
	let aumid = HSTRING::from(app.config().identifier.as_str());
	if let Err(err) = ToastNotificationManager::CreateToastNotifierWithId(&aumid)?.Show(&toast) {
		retire(&landed.note);
		return Err(err);
	}
	Ok(())
}

/// WinRT activation needs an initialised apartment on the calling thread.
///
/// Belt and braces: tao already calls `OleInitialize` on the event-loop thread for
/// drag-and-drop, and this only ever runs there. Deliberately unbalanced — there
/// is no matching `CoUninitialize`, because the apartment belongs to the process's
/// own event loop and tearing it down is not this module's to do. Every outcome is
/// acceptable, including "already initialised the other way": WinRT activation is
/// happy in either apartment, and a genuine failure surfaces as the `XmlDocument`
/// below refusing to be created, which is already logged.
fn initialise_apartment() {
	static ONCE: Once = Once::new();
	ONCE.call_once(|| {
		// SAFETY: no reserved parameter, and the apartment model is a valid COINIT.
		let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
	});
}

// --- the live-toast registry -------------------------------------------------

fn live() -> std::sync::MutexGuard<'static, Vec<(String, ToastNotification)>> {
	// Poison-tolerant for the reason `capture::lock` gives: the guarded value is a
	// list of COM handles, and a panic elsewhere cannot leave it in a shape that
	// makes reading it dangerous.
	LIVE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn retain(note: &str, toast: &ToastNotification) {
	let mut live = live();
	live.push((note.to_owned(), toast.clone()));
	// One at a time rather than a truncate: this runs once per capture, so the list
	// can only ever be one over.
	if live.len() > LIVE_TOASTS {
		live.remove(0);
	}
}

fn retire(note: &str) {
	live().retain(|(held, _)| held != note);
}

// --- activation --------------------------------------------------------------

/// Routes one activation. Runs on a WinRT-owned thread, never the caller's.
fn activated(app: &AppHandle, arguments: &str) {
	let Some(activation) = parse(arguments) else {
		diagnostics::log_error(&format!(
			"[copper] capture: a capture notification was activated with an argument this build \
			 does not recognise ({arguments:?})"
		));
		return;
	};

	match activation {
		Activation::Reveal { note } => {
			retire(&note);
			// **Armed here, for this note, rather than trusted from the capture.**
			// `append_capture`'s `space-changed` armed a reveal at the moment the note
			// landed, and by now that request may be gone — a reader's own scroll or
			// keypress expires it — or, with two captures waiting, may name the
			// *other* note, since the request is one slot and not a queue. Clicking
			// the first of two toasts has to scroll to the first of two notes.
			if let Err(err) = app.emit(REVEAL_EVENT, RevealPayload { note: note.clone() }) {
				diagnostics::log_error(&format!(
					"[copper] capture: could not emit {REVEAL_EVENT}: {err}"
				));
			}
			let handle = app.clone();
			// `reveal_or_log`, not `reveal_without_activating`: a click on the toast is
			// a deliberate ask for the panel, so it takes the focused path — and that
			// path also tells any live failure-notice episode to give up its claim on
			// the window, without which a timer expiring moments later would hide the
			// panel the user just asked for. It validates the saved position too, so a
			// panel last seen on a monitor since unplugged comes back reachable.
			on_main_thread(app, "reveal the panel for a capture notification", move || {
				panel::reveal_or_log(&handle);
			});
		}
		Activation::Reroute {
			space,
			note,
			section,
		} => {
			retire(&note);
			// No window is touched on this path, which is the whole of "re-route
			// without opening the panel": it is satisfied by not calling into `panel`
			// rather than by asking it for a quieter reveal.
			let Some(store) = app.try_state::<SharedStore>() else {
				diagnostics::log_error(
					"[copper] capture: the store is unavailable; the capture notification could not \
					 move the note",
				);
				return;
			};
			// **Refused rather than applied to whatever is open now**, and refused
			// where the user can see it. Copper does not switch spaces to satisfy a
			// button press — that would move the window and the document out from
			// under whatever they are doing — so the honest answer is to say the
			// offer has expired. Without this the move is attempted, `move_notes`
			// errs on an id the open document has never had, and the only trace is a
			// log line: a button that silently does nothing.
			if store::lock(&store).active_id() != Some(space.as_str()) {
				report(app, &CaptureFailure::SpaceSwitched);
				return;
			}
			if let Err(err) = store::move_notes(&store, std::slice::from_ref(&note), &section) {
				diagnostics::log_error(&format!(
					"[copper] capture: the capture notification could not move the note: {}",
					err.message()
				));
			}
		}
	}
}

/// Puts a failure on the panel's notice surface, the one place capture ever
/// reports anything.
///
/// Through the same controller the capture pipeline uses, so a refusal from here
/// takes part in the same episode: it reveals the panel if it was hidden, clears
/// itself on the same timer, and puts the window back afterwards if it was the
/// reason it came up.
fn report(app: &AppHandle, failure: &CaptureFailure) {
	let Some(notice) = app.try_state::<Arc<NoticeController>>() else {
		diagnostics::log_error(&format!(
			"[copper] capture: no notice controller; {} went unreported",
			failure.cause()
		));
		return;
	};
	notice.show(failure);
}

/// The activation argument scheme, in one place.
///
/// Total and refusing: an argument that is not one of the two shapes is reported
/// rather than guessed at, because guessing wrong here moves the user's note.
///
/// The re-route form carries exactly three colon-separated fields and nothing
/// that could be a fourth — [`actionable`] drops any button whose ids would not
/// survive the round trip, so a trailing field means the argument is not one this
/// build wrote.
fn parse(arguments: &str) -> Option<Activation> {
	if let Some(note) = arguments.strip_prefix(REVEAL_VERB) {
		return (!note.is_empty()).then(|| Activation::Reveal {
			note: note.to_owned(),
		});
	}

	let mut fields = arguments.strip_prefix(REROUTE_VERB)?.split(':');
	let (space, note, section) = (fields.next()?, fields.next()?, fields.next()?);
	if fields.next().is_some() {
		return None;
	}
	(!space.is_empty() && !note.is_empty() && !section.is_empty()).then(|| Activation::Reroute {
		space: space.to_owned(),
		note: note.to_owned(),
		section: section.to_owned(),
	})
}

// --- the document ------------------------------------------------------------

/// The toast payload: a heading naming where the note went, the snippet, and a
/// button per alternative section.
fn document(landed: &Landed, snippet: &str) -> String {
	let mut xml = format!(
		"<toast launch=\"{}\" activationType=\"foreground\">\
		 <visual><binding template=\"ToastGeneric\">\
		 <text>{}</text><text>{}</text>\
		 </binding></visual>",
		escape(&format!("{REVEAL_VERB}{}", landed.note)),
		escape(&heading(&landed.section)),
		escape(snippet),
	);

	let buttons = actionable(landed);
	if !buttons.is_empty() {
		xml.push_str("<actions>");
		for section in buttons {
			xml.push_str(&format!(
				"<action content=\"{}\" arguments=\"{}\" activationType=\"foreground\"/>",
				escape(&section.name),
				escape(&format!(
					"{REROUTE_VERB}{}:{}:{}",
					landed.space, landed.note, section.id
				)),
			));
		}
		xml.push_str("</actions>");
	}

	xml.push_str("</toast>");
	xml
}

/// The sections that get a button: document order, capped, and skipping any id
/// that would not survive the round trip.
///
/// A `.copper` file is hand-editable, so neither a section id nor a space id is
/// guaranteed to be the `sec_########` / `spc_########` this app generates — and
/// a colon inside either would split the re-route argument in the wrong place,
/// filing the note into a section the user did not choose, or into a section of
/// that name in a space they did not mean. Dropping the button is the only answer
/// that cannot move somebody's note by accident.
///
/// A space id that cannot be encoded takes **every** button with it, since none
/// of them could name the document they belong to. The toast itself still fires:
/// announcing the capture is what it is mainly for, and its body click carries no
/// space id to break.
fn actionable(landed: &Landed) -> Vec<&SectionRef> {
	if landed.space.is_empty() || landed.space.contains(':') {
		return Vec::new();
	}
	landed
		.alternatives
		.iter()
		.filter(|section| !section.id.contains(':') && !section.name.is_empty())
		.take(MAX_ACTIONS)
		.collect()
}

/// A sentence rather than the bare section name: "Notes" alone under the app's
/// own name reads as a label with no verb, and the toast's first line is the one
/// the user reads at a glance.
fn heading(section: &SectionRef) -> String {
	if section.name.is_empty() {
		"Captured".to_owned()
	} else {
		format!("Captured to {}", section.name)
	}
}

/// The first line of the capture, clipped, with an ellipsis when there is more.
///
/// The ellipsis is decided against the whole text rather than against the first
/// line, so a short first line followed by ten more still says there is more.
pub fn snippet(text: &str) -> String {
	let text = text.trim();
	let first = text.lines().next().unwrap_or_default().trim_end();
	let kept: String = first.chars().take(SNIPPET_CHARS).collect();
	if kept.len() < text.len() {
		format!("{kept}…")
	} else {
		kept
	}
}

/// XML escaping for text the user wrote.
///
/// The payload is assembled as a string and parsed by `XmlDocument::LoadXml`, so
/// an unescaped `&` in a captured selection is a parse failure and no toast at
/// all. Control characters are not representable in XML 1.0 at any escaping, so
/// they become spaces rather than being dropped — a tab between two words is a
/// word boundary, and deleting it runs them together.
fn escape(raw: &str) -> String {
	let mut out = String::with_capacity(raw.len());
	for character in raw.chars() {
		match character {
			'&' => out.push_str("&amp;"),
			'<' => out.push_str("&lt;"),
			'>' => out.push_str("&gt;"),
			'"' => out.push_str("&quot;"),
			'\'' => out.push_str("&apos;"),
			control if (control as u32) < 0x20 => out.push(' '),
			plain => out.push(plain),
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn section(id: &str, name: &str) -> SectionRef {
		SectionRef {
			id: id.to_owned(),
			name: name.to_owned(),
		}
	}

	fn landed(alternatives: Vec<SectionRef>) -> Landed {
		Landed {
			note: "nte_0000abcd".to_owned(),
			space: "spc_00000001".to_owned(),
			notify: true,
			section: section("sec_11112222", "Inbox"),
			alternatives,
		}
	}

	// --- the argument scheme ---

	#[test]
	fn a_body_click_and_a_button_click_are_told_apart() {
		assert_eq!(
			parse("note:nte_0000abcd"),
			Some(Activation::Reveal {
				note: "nte_0000abcd".to_owned()
			})
		);
		assert_eq!(
			parse("move:spc_00000001:nte_0000abcd:sec_33334444"),
			Some(Activation::Reroute {
				space: "spc_00000001".to_owned(),
				note: "nte_0000abcd".to_owned(),
				section: "sec_33334444".to_owned()
			})
		);
	}

	/// Every argument the toast itself writes parses back to what wrote it. This is
	/// the property that actually matters: the two halves are a scheme, and a change
	/// to one that forgets the other produces a toast whose buttons do nothing.
	#[test]
	fn every_argument_the_document_emits_round_trips() {
		let landed = landed(vec![section("sec_33334444", "Ideas"), section("sec_55556666", "Done")]);
		let xml = document(&landed, "a capture");

		for (attribute, expected) in [
			(
				format!("launch=\"note:{}\"", landed.note),
				Activation::Reveal {
					note: landed.note.clone(),
				},
			),
			(
				format!("arguments=\"move:{}:{}:sec_33334444\"", landed.space, landed.note),
				Activation::Reroute {
					space: landed.space.clone(),
					note: landed.note.clone(),
					section: "sec_33334444".to_owned(),
				},
			),
		] {
			assert!(xml.contains(&attribute), "{attribute} missing from {xml}");
			let value = attribute
				.split_once('"')
				.and_then(|(_, rest)| rest.strip_suffix('"'))
				.unwrap();
			assert_eq!(parse(value), Some(expected));
		}
	}

	#[test]
	fn an_unrecognised_argument_is_refused_rather_than_guessed_at() {
		// Filing somebody's note into a section they did not pick is worse than
		// doing nothing, so every one of these has to be `None`.
		for argument in [
			"",
			"note:",
			"move:",
			"move:nte_1",
			"move:spc_1:nte_1",
			"move:spc_1:nte_1:",
			"move:spc_1::sec_1",
			"move::nte_1:sec_1",
			// The pre-space form. An argument this build cannot have written is
			// refused rather than read as a move in the space that happens to be open.
			"move:nte_1:sec_1",
			// And nothing may follow the section: a fourth field means the ids were
			// not the ones `actionable` vetted.
			"move:spc_1:nte_1:sec_1:extra",
			"open:nte_1",
			"nte_1",
		] {
			assert_eq!(parse(argument), None, "{argument:?} was accepted");
		}
	}

	// --- the document ---

	#[test]
	fn the_heading_names_the_destination_section() {
		let xml = document(&landed(Vec::new()), "a capture");
		assert!(xml.contains("<text>Captured to Inbox</text>"), "{xml}");
		assert!(xml.contains("<text>a capture</text>"), "{xml}");
	}

	#[test]
	fn a_space_with_one_section_gets_no_actions_block_at_all() {
		let xml = document(&landed(Vec::new()), "a capture");
		assert!(!xml.contains("<actions>"), "{xml}");
	}

	#[test]
	fn the_buttons_are_capped_at_windows_ceiling_and_keep_document_order() {
		let alternatives: Vec<SectionRef> = (0..7)
			.map(|index| section(&format!("sec_{index}"), &format!("Section {index}")))
			.collect();
		let xml = document(&landed(alternatives), "a capture");

		let names: Vec<&str> = xml.match_indices("content=\"Section ").map(|(_, m)| m).collect();
		assert_eq!(names.len(), MAX_ACTIONS, "{xml}");
		for index in 0..MAX_ACTIONS {
			assert!(xml.contains(&format!("content=\"Section {index}\"")), "{xml}");
		}
		assert!(!xml.contains("content=\"Section 4\""), "{xml}");
	}

	/// A hand-edited document can carry a section id with a colon in it, and the
	/// re-route argument would then split in the wrong place — filing the note
	/// somewhere nobody chose. The button is dropped instead.
	#[test]
	fn a_section_whose_id_would_break_the_argument_gets_no_button() {
		let xml = document(
			&landed(vec![section("sec:broken", "Broken"), section("sec_ok", "Fine")]),
			"a capture",
		);
		assert!(!xml.contains("Broken"), "{xml}");
		assert!(xml.contains("content=\"Fine\""), "{xml}");
	}

	/// The space id rides on every re-route argument, so one that cannot be
	/// encoded takes every button with it — a move that cannot name its document
	/// is a move that would be applied to whichever one is open.
	#[test]
	fn a_space_whose_id_would_break_the_argument_gets_no_buttons_at_all() {
		let mut hostile = landed(vec![section("sec_33334444", "Ideas")]);
		hostile.space = "spc:broken".to_owned();
		let xml = document(&hostile, "a capture");

		assert!(!xml.contains("<actions>"), "{xml}");
		// The announcement itself is still worth firing.
		assert!(xml.contains("<text>Captured to Inbox</text>"), "{xml}");
	}

	#[test]
	fn a_nameless_section_gets_no_button_either() {
		// A blank button is a button the user cannot choose between.
		let xml = document(&landed(vec![section("sec_blank", "")]), "a capture");
		assert!(!xml.contains("<actions>"), "{xml}");
	}

	/// The failure this prevents is total: an unescaped `&` makes `LoadXml` reject
	/// the payload, so the capture that most needs announcing is the one that
	/// silently announces nothing.
	#[test]
	fn user_text_cannot_break_the_payload() {
		let mut hostile = landed(vec![section("sec_1", "R&D <team>")]);
		hostile.section.name = "\"Quotes\" & <angles>".to_owned();
		let xml = document(&hostile, "a & b < c > d \"e\" 'f'");

		assert!(!xml.contains("R&D"), "an ampersand survived unescaped: {xml}");
		assert!(xml.contains("R&amp;D &lt;team&gt;"), "{xml}");
		assert!(xml.contains("a &amp; b &lt; c &gt; d &quot;e&quot; &apos;f&apos;"), "{xml}");
		// Everything after the header is escaped text, so no stray angle bracket can
		// have opened an element of its own.
		assert_eq!(xml.matches("<text>").count(), 2, "{xml}");
	}

	#[test]
	fn a_control_character_becomes_a_space_rather_than_vanishing() {
		assert_eq!(escape("two\twords"), "two words");
		assert_eq!(escape("a\u{0}b"), "a b");
	}

	// --- the snippet ---

	#[test]
	fn a_short_single_line_capture_is_carried_whole() {
		assert_eq!(snippet("a short note"), "a short note");
	}

	#[test]
	fn a_multi_line_capture_shows_its_first_line_and_says_there_is_more() {
		assert_eq!(snippet("first line\nsecond line"), "first line…");
	}

	#[test]
	fn a_long_line_is_clipped_at_the_documented_length() {
		let long = "x".repeat(SNIPPET_CHARS + 40);
		let clipped = snippet(&long);
		assert_eq!(clipped.chars().count(), SNIPPET_CHARS + 1);
		assert!(clipped.ends_with('…'));
	}

	/// Clipping by bytes rather than by characters panics on a multi-byte boundary,
	/// and a captured selection is exactly where non-ASCII text comes from.
	#[test]
	fn clipping_respects_character_boundaries() {
		let long = "é".repeat(SNIPPET_CHARS + 10);
		let clipped = snippet(&long);
		assert_eq!(clipped.chars().count(), SNIPPET_CHARS + 1);
	}

	#[test]
	fn surrounding_whitespace_is_not_mistaken_for_more_text() {
		// `normalise` has already trimmed by the time a capture reaches here, but a
		// trailing blank line reaching this would otherwise add an ellipsis promising
		// content that is not there.
		assert_eq!(snippet("  a note  \n\n"), "a note");
	}

	#[test]
	fn an_empty_capture_produces_an_empty_snippet_rather_than_an_ellipsis() {
		assert_eq!(snippet(""), "");
		assert_eq!(snippet("   "), "");
	}
}
