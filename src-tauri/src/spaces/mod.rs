//! Space *management*: choosing which space is open, and the OS entry points
//! that choose one for you.
//!
//! The store already owns space documents, `settings.json`, atomic writes and
//! the directory watch. Nothing here reimplements any of that — `open_space_at`
//! is a policy wrapper that resolves a path, classifies it, ends any live
//! `$EDITOR` handoff, and then delegates to `store::open_space`, which loads the
//! document, sets it active, updates `recents` and moves the watch. The watcher
//! is not touched here at all.
//!
//! Three rules hold this layer together:
//!
//! - **Commands take paths, not indices.** `activeSpace` being an index is a
//!   persistence detail of `settings.json`. Everything here identifies a space by
//!   path, so a concurrent recents reorder can never make a request refer to the
//!   wrong space, and the index is derived at write time inside the store.
//! - **Availability is probed, never persisted.** A stale-looking entry is only
//!   ever a live probe result, which is what makes "it comes back when the branch
//!   is checked out again" work with no repair step.
//! - **Activation is serialised.** Ending handoffs and swapping the document are
//!   one transition: if another activation could interleave between them, a
//!   handoff would be recreated against the outgoing document and the cross-space
//!   hazard would return by a different route.

pub mod availability;
pub mod dispatch;
pub mod launch;
pub mod paths;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use availability::{Availability, Executor, ProbeResult, RealFs, UnavailableReason};
use dispatch::{LaunchHost, Request};
use paths::{comparison_key, display_path, same_path};

use crate::store::error::StoreError;
use crate::store::events::{AppSink, EventSink, StoreEvent};
use crate::store::model::Space;
use crate::store::{self, SharedStore};
use crate::{diagnostics, editor, panel};

type Reply<T> = std::result::Result<T, StoreError>;

/// One entry of the switcher.
///
/// `key` is on the wire because the availability event is keyed by it: the
/// frontend patches a single row rather than re-listing the whole menu every
/// time a probe answers.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RecentEntry {
	/// The stored path, and what every command here takes back.
	pub path: String,
	/// The same path with any verbatim prefix removed, for display.
	pub display_path: String,
	pub key: String,
	/// The document's own `name` when it could be read, the file stem otherwise —
	/// so an unavailable entry still shows something recognisable.
	pub name: String,
	pub active: bool,
	pub availability: Availability,
}

/// What an activation did.
///
/// The invariant is strict: `changed: true` always carries the authoritative
/// document, `changed: false` always carries `None`. `false` is what lets the
/// panel keep its scroll position and its selection when the requested space is
/// already open, instead of reloading an identical document and reconciling
/// against it for nothing. And because the outcome carries the document, the
/// frontend must not also pull it — that would be a second read of state it was
/// just handed, and a second chance for the two to disagree.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ActivateOutcome {
	pub changed: bool,
	pub space: Option<Space>,
}

impl ActivateOutcome {
	fn unchanged() -> Self {
		Self {
			changed: false,
			space: None,
		}
	}

	fn changed(space: Space) -> Self {
		Self {
			changed: true,
			space: Some(space),
		}
	}
}

// --- process-wide state --------------------------------------------------------

/// Held across the whole ending-handoffs → open sequence, and across the
/// active-entry check in `remove_recent`.
///
/// Not the store's lock: `editor::end_all` writes through the store, so holding
/// that one across the teardown would deadlock. This one exists purely to make
/// the transition indivisible.
static ACTIVATION: Mutex<()> = Mutex::new(());

static EXECUTOR: OnceLock<Executor> = OnceLock::new();

/// The guard every space transition is serialised behind.
///
/// Public because opening a note in `$EDITOR` has to take it too: creating a
/// handoff and switching space are the two halves of the hazard A27 exists to
/// prevent, and a handoff created between this layer's teardown and the store's
/// document swap would be bound to the outgoing document with nothing left to
/// end it.
pub fn activation() -> MutexGuard<'static, ()> {
	lock(&ACTIVATION)
}

/// Locking for every mutex in this layer, poison-tolerant.
///
/// A panicking worker must not turn every later lock into a second failure: what
/// these mutexes hold is a queue and a cache, both still coherent after a panic
/// in whatever was holding them. Same rule as `store::lock`, and the reason it is
/// stated once rather than in each submodule.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
	mutex.lock().unwrap_or_else(|err| err.into_inner())
}

/// The availability event goes out through the app directly rather than through
/// the store's `EventSink`: nothing about availability is a store change, and
/// giving the store a way to emit it would be the first step towards persisting
/// it. Named for what it carries, so it does not read as a second spelling of
/// the store's `AppSink`, which is imported above and does a different job.
struct AvailabilitySink(AppHandle);

impl availability::ResultSink for AvailabilitySink {
	fn deliver(&self, result: &ProbeResult) {
		// The result *is* the payload. A hand-built object here would be a second
		// copy of the field names the frontend already mirrors, free to drift from
		// the struct that defines them.
		if let Err(err) = self.0.emit(availability::AVAILABILITY_CHANGED, result) {
			diagnostics::log_error(&format!("[copper] could not emit an availability result: {err}"));
		}
	}
}

fn executor(app: &AppHandle) -> &'static Executor {
	EXECUTOR.get_or_init(|| {
		Executor::new(Box::new(RealFs), Box::new(AvailabilitySink(app.clone())))
	})
}

// --- the one open path ---------------------------------------------------------

/// Every entry point — the switcher, the picker, argv, and a forwarded second
/// launch — funnels through here.
pub fn open_space_at(app: &AppHandle, path: &Path) -> Reply<ActivateOutcome> {
	let _serialised = activation();
	let state = app.state::<SharedStore>();

	// A23: re-opening the space that is already open is a reveal and nothing else
	// — no store call, no settings write, no events. Explorer double-clicking the
	// open file is the case this exists for, and it is why the caller reveals
	// rather than this function.
	if is_active(&state, path) {
		return Ok(ActivateOutcome::unchanged());
	}

	let key = comparison_key(path);
	// Classified *before* the handoffs are ended, so a path that was never going
	// to open does not cost the user a live external edit session.
	let (availability, name) = availability::probe(&RealFs, path);
	executor(app).record(&key, availability.clone(), name);
	if let Availability::Unavailable { reason, message } = &availability {
		return Err(refusal(*reason, message));
	}

	let outgoing = leave_current_space(app)?;

	// `?` before the sweep: a failed open leaves the outgoing space active, and
	// its blobs must survive that.
	let doc = store::open_space(&state, path)?;
	sweep_detached(outgoing);
	// The row agrees with what just happened without waiting for another probe.
	executor(app).record(&key, Availability::Available, Some(doc.name.clone()));
	Ok(ActivateOutcome::changed(doc))
}

/// Ends every live `$EDITOR` handoff against the **outgoing** document, and
/// refuses the transition when any of them could not be saved back.
///
/// A27. Handoffs are keyed by `note_id`, which is unique only within one
/// document, so one surviving a switch can rebind to a coincidentally matching
/// id in the new space and write one space's external edit into another. Each is
/// reconciled against the outgoing document first — the temp file may hold work
/// the user has not saved, and switching space is not consent to discard it —
/// and only then stopped.
///
/// **What a refusal here does and does not mean.** By the time `end_all` returns,
/// every session has already ended: they do *not* stay live, and an earlier
/// version of this comment claimed they did. What survives a refusal is the
/// *text* — a handoff whose save could not be read back keeps its temp file, and
/// the message below names it. The transition is refused so the user can recover
/// that text against the space it belongs to, rather than against whichever space
/// they were moving to.
///
/// Must be called with the activation guard held, so nothing can create a handoff
/// between the teardown and the document swap.
/// Everything the outgoing space is owed before another one takes its place.
///
/// Ends every live editor handoff — which can refuse the transition — and then
/// hands back the outgoing space's path and document so the caller can sweep it
/// **after** the swap has actually succeeded.
///
/// The return value is what makes that ordering possible, and the ordering is
/// the point. Sweeping before the swap means a *failed* open leaves the
/// outgoing space still active, its undo stack still alive, and the blobs its
/// snapshots reference already collected — the exact "undo restores a note
/// whose attachments are gone" outcome the whole no-delete-on-mutation rule
/// exists to prevent. Sweeping the detached document afterwards cannot: by then
/// nothing can undo into it.
///
/// **The sweep runs here and at startup, and nowhere else.** Never during a
/// session, for the same reason.
fn leave_current_space(app: &AppHandle) -> Reply<Option<(PathBuf, Space)>> {
	end_handoffs_before_switching(app)?;
	// After `end_all`, so a handoff still writing back cannot have its note's
	// attachments collected out from under it.
	Ok(detach_for_sweep(app))
}

/// The active space's path and document, cloned out and the guard dropped.
///
/// Taken **before** the swap because afterwards the store no longer holds the
/// outgoing document, and used **after** it because until the swap succeeds the
/// outgoing space might still be the one the user is looking at.
fn detach_for_sweep(app: &AppHandle) -> Option<(PathBuf, Space)> {
	let state = app.state::<SharedStore>();
	let guard = store::lock(&state);
	guard
		.active_path()
		.map(Path::to_path_buf)
		.zip(guard.active_space().ok())
}

/// Collects the outgoing space's orphans, once it really is outgoing.
///
/// Best-effort and silent: the user asked to change space, not to tidy a
/// directory, and a failure here must not colour a switch that worked.
fn sweep_detached(outgoing: Option<(PathBuf, Space)>) {
	if let Some((path, doc)) = outgoing {
		crate::attachments::sweep(&path, &doc);
	}
}

fn end_handoffs_before_switching(app: &AppHandle) -> Reply<()> {
	let retained = editor::end_all(app);
	if retained.is_empty() {
		return Ok(());
	}

	let kept: Vec<String> = retained.iter().map(|path| path.display().to_string()).collect();
	Err(StoreError::Io(format!(
		"the space was not switched, because an open editor's text could not be saved back. Every \
		 editor session has ended and any changes that did save were applied; the text that could \
		 not be read back is still in {}",
		kept.join(", ")
	)))
}

/// Whether `path` names the space that is currently open.
///
/// Compared with the lexical key rather than by index: `activeSpace` is an index
/// into a list that promotion reorders, so an in-memory index would go stale the
/// moment anything touched `recents`.
fn is_active(state: &SharedStore, path: &Path) -> bool {
	store::lock(state)
		.active_path()
		.is_some_and(|active| same_path(active, path))
}

/// An unavailable path refused with its cause carried in the message, and a
/// `kind` the frontend can still branch on.
fn refusal(reason: UnavailableReason, message: &str) -> StoreError {
	let message = message.to_string();
	match reason {
		UnavailableReason::Missing => StoreError::NotFound(message),
		UnavailableReason::NotAFile => StoreError::Invalid(message),
		UnavailableReason::Invalid => StoreError::Parse(message),
		UnavailableReason::Unreadable | UnavailableReason::DriveUnavailable => StoreError::Io(message),
	}
}

// --- commands ------------------------------------------------------------------

/// A **pure read** of cached state. It must never start probe work: if listing
/// kicked off probes and probe results in turn told the frontend to re-list, the
/// two would drive each other in a loop with every pass minting a new generation.
#[tauri::command]
pub async fn list_recents(app: AppHandle, state: State<'_, SharedStore>) -> Reply<Vec<RecentEntry>> {
	let (recents, active_key, active_name) = {
		let store = store::lock(&state);
		(
			store.recents().to_vec(),
			// One key for the open document rather than one per row: `same_path`
			// re-derives both sides for every entry, and each row's own key already
			// *is* the lexical identity it would produce. Deriving it here is safe
			// under the lock because `comparison_key` is lexical and never touches
			// the filesystem or the store.
			store.active_path().map(comparison_key),
			// The name only, not the document: `active_space` would clone every note
			// in the space to hand back this one string.
			store.active_name().map(str::to_owned),
		)
	};

	let keys: Vec<String> = recents
		.iter()
		.map(|entry| comparison_key(Path::new(entry)))
		.collect();
	let cached = executor(&app).cached(&keys);

	Ok(recents
		.into_iter()
		.zip(keys)
		.zip(cached)
		.map(|((entry, key), (availability, probed_name))| {
			// A string comparison rather than `same_path`: both sides are already the
			// lexical key, one derived per row above and one derived once for the open
			// document.
			let is_active = active_key.as_deref() == Some(key.as_str());
			let path = Path::new(&entry);
			let display = display_path(path);
			let name = if is_active { active_name.clone() } else { probed_name }
				.unwrap_or_else(|| file_stem(path));
			RecentEntry {
				display_path: display,
				name,
				key,
				active: is_active,
				// The open document is authoritative about itself: the store has it
				// loaded, so no probe is needed to know it can be read.
				availability: if is_active {
					Availability::Available
				} else {
					availability
				},
				path: entry,
			}
		})
		.collect())
}

/// The only thing that starts probes. Called when the menu opens and on an
/// explicit retry.
#[tauri::command]
pub async fn refresh_recents(app: AppHandle, state: State<'_, SharedStore>) -> Reply<()> {
	let recents = store::lock(&state).recents().to_vec();
	executor(&app).submit(availability::snapshot(&recents));
	Ok(())
}

/// What the switcher calls. Going straight to the store's `open_space` would
/// bypass the already-active no-op, the availability classification and the
/// cause presentation — the three things this layer exists to add.
#[tauri::command]
pub async fn activate_space(path: String, app: AppHandle) -> Reply<ActivateOutcome> {
	open_space_at(&app, Path::new(&path))
}

#[tauri::command]
pub async fn pick_and_open_space(app: AppHandle) -> Reply<ActivateOutcome> {
	let window = panel_window(&app)?;

	let picked = blocking_dialog(move || {
		app_dialog(&window)
			.set_title("Open Space")
			.blocking_pick_file()
	})
	.await?;

	// Cancelling is a success with no state change.
	let Some(path) = picked else {
		return Ok(ActivateOutcome::unchanged());
	};
	open_space_at(&app, &path)
}

#[tauri::command]
pub async fn create_space_interactive(app: AppHandle) -> Reply<ActivateOutcome> {
	let window = panel_window(&app)?;
	let directory = store::lock(&app.state::<SharedStore>())
		.spaces_dir()
		.to_path_buf();

	let picked = blocking_dialog(move || {
		app_dialog(&window)
			.set_title("New Space")
			.set_directory(&directory)
			.set_file_name("space.copper")
			.blocking_save_file()
	})
	.await?;

	let Some(picked) = picked else {
		return Ok(ActivateOutcome::unchanged());
	};
	let path = with_copper_extension(picked);

	// The store refuses to create over an existing file, so the policy for that
	// case is this layer's. A user who picked a real space in a save dialog meant
	// to use it — refusing would be obtuse — but a file that is not a space
	// document must never be overwritten.
	if path.exists() {
		let (availability, _) = availability::probe(&RealFs, &path);
		return match availability {
			Availability::Available => open_space_at(&app, &path),
			// The probe's own sentence, not an assertion of our own. "Not a Copper
			// space" is only established for `Invalid`; a file the probe could not
			// read, or one on a drive that just went away, has established nothing of
			// the sort and saying so would send the user to fix the wrong thing.
			other => Err(StoreError::Invalid(format!(
				"{} already exists and was not created. {}",
				display_path(&path),
				other.message().unwrap_or("It could not be read.")
			))),
		};
	}

	let name = file_stem(&path);
	let _serialised = activation();
	// Creating is a switch like any other, so the outgoing space's editor sessions
	// are ended against the document they belong to first — under the same guard,
	// so nothing can open a handoff between the teardown and the swap. Without
	// this, `create_space` replaces the active document while a handoff keyed by
	// `note_id` is still live, which is the exact cross-space write A27 exists to
	// prevent.
	let outgoing = leave_current_space(&app)?;
	// `create_space` opens what it created, updates `recents` and emits its one
	// `settings-changed`. Opening again would be a redundant second load and a
	// second recents touch. If the create succeeds and the open half then fails,
	// the store leaves the file in place and the error says so — never a
	// half-activated state.
	let doc = store::create_space(&app.state::<SharedStore>(), &path, &name)?;
	sweep_detached(outgoing);
	executor(&app).record(
		&comparison_key(&path),
		Availability::Available,
		Some(doc.name.clone()),
	);
	Ok(ActivateOutcome::changed(doc))
}

/// Explicit user action only — never called automatically, and never on the
/// active entry.
///
/// A26's refusal is enforced here rather than in the store. `Store::remove_recent`
/// deliberately has no opinion: it removes the entry and re-points `activeSpace`
/// at the still-open space. That is a reasonable primitive; the policy that the
/// active entry cannot be forgotten is this layer's, because it is what keeps
/// removal from having to invent a replacement active space.
#[tauri::command]
pub async fn remove_recent(path: String, state: State<'_, SharedStore>, app: AppHandle) -> Reply<()> {
	let path = PathBuf::from(path);
	// Held across the check and the removal, so an activation cannot slip between
	// them and make this remove the entry that just became active.
	let _serialised = activation();

	if is_active(&state, &path) {
		return Err(StoreError::Invalid(
			"this is the space you have open. Switch to another space first.".into(),
		));
	}

	store::remove_recent(&state, &path)?;
	// A removed and later re-added path must be probed afresh rather than
	// answered from a snapshot it was not in.
	executor(&app).forget(&comparison_key(&path));
	Ok(())
}

// --- dialogs -------------------------------------------------------------------

/// The panel, or the reason there is nothing to parent a dialog to.
///
/// Both dialog commands need it and neither can proceed without it, so the
/// lookup and its wording live in one place. `panel.rs` has its own lookup, but
/// it logs the miss rather than returning it — which is right for the tray and
/// wrong for a command someone is waiting on.
fn panel_window(app: &AppHandle) -> Reply<WebviewWindow> {
	app.get_webview_window(panel::PANEL_LABEL)
		.ok_or_else(|| StoreError::Unavailable("the panel window is not available".into()))
}

/// `set_parent` is not optional. The panel is `alwaysOnTop`, so a dialog with no
/// owner window opens *behind* it and reads as a hang — the app appears frozen
/// with no visible dialog. An owned dialog is always drawn above its owner.
fn parented_dialog(window: &WebviewWindow) -> tauri_plugin_dialog::FileDialogBuilder<tauri::Wry> {
	window.dialog().file().set_parent(window)
}

fn app_dialog(window: &WebviewWindow) -> tauri_plugin_dialog::FileDialogBuilder<tauri::Wry> {
	parented_dialog(window).add_filter("Copper Space", &["copper"])
}

/// Task-011's paperclip picker, sharing this layer's dialog plumbing rather
/// than growing a second copy of it.
///
/// It lives here, next to `pick_and_open_space`, because `set_parent` and the
/// `spawn_blocking` wrapper are the two things every dialog in this app has to
/// get right, and a second module opening dialogs is a second place to get them
/// wrong. **No extension filter**: an attachment can be anything, and the type
/// that matters is sniffed from the bytes at ingest rather than taken from a
/// dialog's idea of it.
pub async fn pick_attachment_files(app: &AppHandle) -> Reply<Vec<PathBuf>> {
	let window = panel_window(app)?;

	let picked = tauri::async_runtime::spawn_blocking(move || {
		parented_dialog(&window)
			.set_title("Attach Files")
			.blocking_pick_files()
	})
	.await
	.map_err(|err| StoreError::Io(format!("the file dialog could not be opened: {err}")))?;

	picked
		.unwrap_or_default()
		.into_iter()
		.map(|location| {
			location
				.into_path()
				.map_err(|err| StoreError::Invalid(format!("that location is not a file path: {err}")))
		})
		.collect()
}

/// The `blocking_*` dialog calls are documented as not for the main thread, and
/// an `async fn` command alone is not enough: it gets the call off the main
/// thread but still parks an async-runtime worker for as long as the dialog
/// stays open.
/// The picked location is converted to a path here rather than at the call
/// sites: `FilePath` also models a URL, which the desktop file dialogs never
/// produce, and letting that shape travel any further would mean every caller
/// carrying a branch for a case that cannot happen.
async fn blocking_dialog(
	open: impl FnOnce() -> Option<tauri_plugin_dialog::FilePath> + Send + 'static,
) -> Reply<Option<PathBuf>> {
	let picked = tauri::async_runtime::spawn_blocking(open)
		.await
		.map_err(|err| StoreError::Io(format!("the file dialog could not be opened: {err}")))?;

	picked
		.map(|location| {
			location
				.into_path()
				.map_err(|err| StoreError::Invalid(format!("that location is not a file path: {err}")))
		})
		.transpose()
}

/// A typed name gets `.copper` **appended**, never substituted.
///
/// `notes.txt` becomes `notes.txt.copper`: that keeps what the user typed and
/// still produces a file the association opens, whereas silently rewriting
/// someone's extension is the kind of quiet surprise that makes a save dialog
/// untrustworthy.
fn with_copper_extension(path: PathBuf) -> PathBuf {
	if path
		.extension()
		.is_some_and(|ext| ext.eq_ignore_ascii_case("copper"))
	{
		return path;
	}
	let mut name = path.file_name().unwrap_or_default().to_os_string();
	name.push(".copper");
	path.with_file_name(name)
}

fn file_stem(path: &Path) -> String {
	path.file_stem()
		.map(|stem| stem.to_string_lossy().into_owned())
		.unwrap_or_else(|| display_path(path))
}

// --- launch --------------------------------------------------------------------

/// The dispatcher's hands: opening goes through the same policy wrapper the
/// switcher uses, and the reveal goes through task-002's `panel::reveal`.
struct AppLaunchHost(AppHandle);

impl LaunchHost for AppLaunchHost {
	fn open(&self, path: &Path) {
		if let Err(err) = open_space_at(&self.0, path) {
			diagnostics::log_error(&format!("[copper] could not open {}: {err}", path.display()));
			// A forwarded launch happens with the webview already listening, so an
			// event is the right channel here — unlike the cold path below. Through
			// the store's own sink rather than a second emit written by hand: it
			// already owns this payload's shape and the rule that a failed emit is
			// logged rather than propagated, because the failure it announces has
			// already happened.
			AppSink::new(self.0.clone()).emit(&StoreEvent::error(&err));
		}
	}

	/// Marshalled onto the main thread rather than called where it is decided.
	///
	/// `panel.rs` states the rule for the whole module — window operations happen
	/// on the thread that owns the window — and this is the first caller that is
	/// not already on it: the dispatcher's worker is an ordinary background
	/// thread, deliberately, so that the single-instance callback can hand over
	/// and return without stalling the message loop. Tauri's own window methods
	/// post to the event loop and would mostly survive the violation, but
	/// `reveal_or_log` also reaches into the capture notice controller, and
	/// "mostly survives" is not a property worth depending on for the one call
	/// that decides whether the user sees their window.
	fn present(&self) {
		let app = self.0.clone();
		if let Err(err) = self.0.run_on_main_thread(move || panel::reveal_or_log(&app)) {
			diagnostics::log_error(&format!("[copper] could not reach the main thread to reveal: {err}"));
		}
	}
}

/// Reads `std::env::args_os`, opens the space it names, and returns the
/// presentation request to queue.
///
/// **The open happens here, synchronously**, and that is the whole point: `setup`
/// calls this before `start_capture`, so a double-tap in the moments after an
/// Explorer launch cannot append to whatever space was previously active. Merely
/// submitting the request to the dispatcher would order nothing, because the
/// dispatcher is asynchronous by design.
///
/// "Completing" means the store has applied the open and nothing more. It must
/// not mean a main-thread round trip or any acknowledgement from the webview:
/// `setup` runs before the Windows message pump resumes, so blocking it on
/// anything needing a message dispatched is a deadlock rather than a delay.
/// Reveal and frontend notification are deliberately outside it.
///
/// `args_os` rather than `args`, which panics on an argument that is not valid
/// Unicode — and this runs in `setup`, where a panic takes the process down with
/// no window ever shown.
pub fn apply_cold_launch(app: &AppHandle) -> Request {
	let args: Vec<String> = std::env::args_os()
		.map(|arg| arg.to_string_lossy().into_owned())
		.collect();
	let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

	let Some(path) = launch::space_path_from_args(&args, &cwd) else {
		return Request::cold(None);
	};

	if let Err(err) = open_space_at(app, &path) {
		// A17. On a cold launch nothing is listening and Tauri has no replay, so an
		// emit here would be dropped: it is recorded as state and surfaces through
		// the frontend's mount-time `get_status` pull. The path is not added to
		// `recents` — the store never touched it, because the open failed.
		store::lock(&app.state::<SharedStore>()).push_startup_notice(format!(
			"{} could not be opened: {}",
			display_path(&path),
			err.message()
		));
	}

	// Whether the open succeeded or not, the panel is revealed with the
	// explanation: the user double-clicked a file and is owed a window. No `path`
	// on it — the open happened above, synchronously, and re-queueing the path
	// would open it a second time through the dispatcher.
	Request {
		path: None,
		reveal: true,
	}
}

/// Queues the cold launch's presentation and opens the readiness gate.
/// Everything queued before this — including a forwarded launch that arrived
/// during startup — drains afterwards, in arrival order, with the cold request
/// first.
pub fn start_dispatcher(app: &AppHandle, cold: Request) {
	dispatch::submit(cold);
	// Initialised here so the first menu open does not pay for it, and so the
	// availability sink exists before anything can produce a result.
	let _ = executor(app);
	// The startup half of the sweep policy. `apply_cold_launch` has already
	// opened whatever the command line named, so the space being collected is the
	// one the user is about to look at, and nothing has had a chance to add a
	// note — so no blob it could collect is one this session created.
	//
	// **On its own thread, off the `setup()` path.** Nothing needs it finished
	// before the panel exists, and enumerating an assets directory on a network
	// share can take seconds — which would sit directly between launch and the
	// first capture being possible. Detached deliberately: there is nothing to
	// join, and a failure is already best-effort.
	let handle = app.clone();
	std::thread::spawn(move || crate::attachments::commands::sweep_active_space(&handle));
	dispatch::start(Arc::new(AppLaunchHost(app.clone())));
}

/// The single-instance callback's whole body. It runs on the main thread, inside
/// the message loop tao is pumping, so it hands over and returns rather than
/// doing any work inline.
pub fn forwarded_launch(argv: &[String], cwd: &str) {
	let path = launch::space_path_from_args(argv, Path::new(cwd));
	dispatch::submit(Request::forwarded(path));
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_missing_extension_is_appended_and_a_present_one_is_kept() {
		assert_eq!(
			with_copper_extension(PathBuf::from(r"D:\x\notes")),
			PathBuf::from(r"D:\x\notes.copper")
		);
		assert_eq!(
			with_copper_extension(PathBuf::from(r"D:\x\notes.copper")),
			PathBuf::from(r"D:\x\notes.copper")
		);
		assert_eq!(
			with_copper_extension(PathBuf::from(r"D:\x\notes.COPPER")),
			PathBuf::from(r"D:\x\notes.COPPER")
		);
	}

	/// Appended rather than substituted: rewriting what someone typed is worse
	/// than a double extension.
	#[test]
	fn a_different_extension_is_kept_and_copper_added_after_it() {
		assert_eq!(
			with_copper_extension(PathBuf::from(r"D:\x\notes.txt")),
			PathBuf::from(r"D:\x\notes.txt.copper")
		);
	}

	#[test]
	fn each_cause_refuses_with_a_kind_the_frontend_can_branch_on() {
		for (reason, kind) in [
			(UnavailableReason::Missing, "not-found"),
			(UnavailableReason::NotAFile, "invalid"),
			(UnavailableReason::Invalid, "parse"),
			(UnavailableReason::Unreadable, "io"),
			(UnavailableReason::DriveUnavailable, "io"),
		] {
			let availability = Availability::unavailable(reason);
			let message = availability.message().unwrap();
			let err = refusal(reason, message);
			assert_eq!(err.kind(), kind);
			// The cause sentence survives to the user rather than being replaced by a
			// generic "could not open".
			assert_eq!(err.message(), message);
		}
	}

	#[test]
	fn a_file_stem_falls_back_to_the_whole_path() {
		assert_eq!(file_stem(Path::new(r"D:\x\work.copper")), "work");
		assert_eq!(file_stem(Path::new(r"C:\")), r"C:\");
	}
}
