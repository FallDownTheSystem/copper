//! The second strategy: synthesize `Ctrl+C`, watch the clipboard, put the user's
//! clipboard back.
//!
//! This is not a passive read and the design says so plainly. Synthesizing
//! `Ctrl+C` runs whatever the target application binds to that chord: with an
//! empty selection VS Code and Cursor copy the current line, and Windows Terminal
//! interrupts the running command. The cascade reaches here precisely when UI
//! Automation found nothing, which is when those behaviours are most likely — so
//! it is not a rare corner. R-Q2 resolved to accept it rather than terminate the
//! cascade on a trusted-empty UIA answer.
//!
//! The step order is load-bearing; each step avoids one specific real failure.

use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::{
	GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
	KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
};

use crate::diagnostics;
use crate::win32::clipboard::{self, ClipboardError, Snapshot};
use crate::win32::foreground::Target;
use crate::win32::keys::{
	VK_C, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
	VK_RWIN,
};
use crate::win32::EXTRA_INFO_SIGNATURE;

use super::{
	normalise, Evidence, StrategyResult, CLIPBOARD_POLL_INTERVAL, CLIPBOARD_POLL_TIMEOUT,
	MODIFIER_RELEASE_TIMEOUT,
};

/// Every modifier that could turn the injected `Ctrl+C` into a different chord.
///
/// Family-agnostic as written, and the invariant Phase 7 must preserve is that
/// the trigger key itself is always in this list — the trigger fires on a key-up
/// and the fallback injects a chord, so a trigger key missing from here would let
/// a still-held trigger corrupt the injection.
const WATCHED_MODIFIERS: [u32; 8] = [
	VK_LSHIFT, VK_RSHIFT, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN,
];

const MODIFIER_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Runs the fallback against the sampled target.
pub fn try_clipboard(target: Target) -> StrategyResult {
	let mut evidence = Evidence::default();

	// The trigger fires on a key-up, so nothing is held by construction — but the
	// user can genuinely be holding something else, and `SendInput` is documented
	// not to reset keyboard state. A physically-held Shift at injection time turns
	// the synthesized Ctrl+C into Ctrl+Shift+C, a different and often-bound
	// shortcut. Copper waits and then fails rather than injecting key-ups for keys
	// the user is holding: that would leave the logical keyboard disagreeing with
	// the physical one, would have to be unwound on every exit path including a
	// partial insert, and a synthetic Alt or Win release can itself trigger shell
	// behaviour — which would break the guarantee that focus never moves.
	if !wait_for_modifier_release() {
		evidence.modifier_held = true;
		return StrategyResult::nothing(evidence);
	}

	// Revalidate before injecting. The modifier wait can add 300 ms, during which
	// focus can move — task-001 names a Sticky Keys confirmation dialog as the
	// concrete case. Injecting into a window that is no longer the sampled one
	// sends a keystroke to the wrong application.
	if !target.still_current() {
		evidence.foreground_changed = true;
		return StrategyResult::nothing(evidence);
	}

	// Taken **after** the waits, not before them. The snapshot is the bytes that
	// will be put back and its sequence token is what says they are still the
	// current clipboard; a snapshot taken before a 300 ms modifier wait can be
	// stale by the time it matters, and restoring it would destroy whatever the
	// user copied in between. Nothing has been injected at this point either way,
	// so a failure here still costs a capture and nothing else.
	let snapshot = match clipboard::snapshot() {
		Ok(snapshot) => snapshot,
		Err(ClipboardError::Busy { .. }) => {
			evidence.clipboard_never_opened = true;
			return StrategyResult::nothing(evidence);
		}
		Err(err) => {
			diagnostics::log_error(&format!("[copper] capture: clipboard snapshot failed: {err}"));
			return StrategyResult::nothing(evidence);
		}
	};

	let before = snapshot.sequence();
	if before == 0 {
		// No WINSTA_ACCESSCLIPBOARD on this window station: polling cannot work,
		// so this is not a baseline that will never change — it is no baseline.
		evidence.clipboard_never_opened = true;
		return StrategyResult::nothing(evidence);
	}

	// The last check before anything is injected. Between the snapshot's session
	// closing and this line the clipboard could have moved again; if it has, the
	// bytes in hand are a stale copy and every later step — the poll baseline, the
	// restore — would be reasoning about a clipboard that no longer exists.
	// Abandoning without injecting is the safe direction: it costs one capture,
	// where continuing would cost the user whatever they just copied.
	if clipboard::sequence_number() != before {
		diagnostics::log(
			"[copper] capture: the clipboard changed while the fallback was preparing; \
			 abandoning without injecting rather than working from a stale snapshot",
		);
		// Nothing is recorded against this path: the clipboard plainly did move, so
		// the unchanged flag would be a lie to whoever extends the precedence rule
		// next, and nothing was injected — the cause belongs to whatever the rest of
		// the cascade saw.
		return StrategyResult::nothing(evidence);
	}

	let injected_at = Instant::now();
	// A *short* insert is not the same as no insert. If Ctrl-down and C-down went
	// in but the key-ups did not, the target may already have copied — so the
	// clipboard still has to be polled and put back. Returning early here would
	// leave the target's copy on the user's clipboard with the snapshot never
	// restored, which task-001's review caught as a real data-loss path.
	if !send_ctrl_c() {
		evidence.send_input_failed = true;
	}

	let observed = poll_for_change(target, before, injected_at);

	let Some(Observation { foreign_owner }) = observed else {
		evidence.clipboard_unchanged = true;
		return StrategyResult::nothing(evidence);
	};

	// The sequence comes back from inside the read's own session. Taken AFTER the
	// read rather than before it, because reading a delayed-rendered format makes
	// the owning application call `SetClipboardData`, which bumps the sequence —
	// so expecting the pre-read value would withhold the restore every time Copper
	// captures from an application that uses delayed rendering, which is most of
	// the interesting ones. Taken *inside* that session rather than after it,
	// because a copy landing in the gap would raise the live sequence, the
	// restore's own in-session check would then compare equal, and the user's new
	// content would be destroyed by the check meant to protect it.
	let (read, expected) = match clipboard::read_text() {
		Ok((text, sequence)) => (Ok(text), sequence),
		// No sequence to trust, so no restore is attempted below.
		Err(err) => (Err(err), 0),
	};
	let foreign_now = owner_is_foreign(target);

	if expected != 0 {
		restore(&snapshot, expected, foreign_owner || foreign_now, &mut evidence);
	}

	match read {
		Ok(Some(text)) => {
			let normalised = normalise(&text);
			if normalised.is_empty() {
				// The sequence moved and text came back, but there was nothing in
				// it. That is a selection of whitespace, not an untextual one.
				evidence.empty_after_normalisation = true;
				StrategyResult::nothing(evidence)
			} else {
				StrategyResult::captured(normalised, evidence)
			}
		}
		Ok(None) => {
			// The sequence moved but `CF_UNICODETEXT` is absent — a file in
			// Explorer puts `CF_HDROP` on the clipboard and does exactly this.
			evidence.clipboard_changed_but_untextual = true;
			StrategyResult::nothing(evidence)
		}
		Err(ClipboardError::Busy { .. }) => {
			evidence.clipboard_never_opened = true;
			StrategyResult::nothing(evidence)
		}
		Err(err) => {
			diagnostics::log_error(&format!("[copper] capture: clipboard read failed: {err}"));
			evidence.clipboard_changed_but_untextual = true;
			StrategyResult::nothing(evidence)
		}
	}
}

/// What the poll saw.
struct Observation {
	/// The write came from a process other than the target — almost certainly
	/// something the user copied themselves.
	foreign_owner: bool,
}

/// Polls until the sequence number moves, or the budget runs out.
///
/// "The sequence number moved" is not on its own a sufficient discriminator:
/// clipboard managers, Office and browsers with clipboard listeners all write
/// during the polling window. So the owner process is checked too — a **soft**
/// signal for the capture, since applications set the clipboard with no owner
/// window or through OLE and a mismatch must never by itself discard a good
/// capture, but a **hard** signal for the restore, because content somebody else
/// just wrote is not Copper's to overwrite.
/// Whether whoever owns the clipboard right now is somebody other than the
/// capture target.
///
/// Sampled fresh at each call, never cached: the whole point is that it can
/// change between the poll and the read.
fn owner_is_foreign(target: Target) -> bool {
	clipboard::owner_pid().is_some_and(|pid| pid != target.pid)
}

fn poll_for_change(target: Target, before: u32, injected_at: Instant) -> Option<Observation> {
	let mut observed: Option<Observation> = None;
	loop {
		if clipboard::sequence_number() != before {
			let foreign_owner = owner_is_foreign(target);
			observed = Some(Observation { foreign_owner });
			if !foreign_owner {
				break;
			}
			// Keep polling: the target's own write may still be coming, and the
			// last observation is the one that counts.
		}
		if injected_at.elapsed() >= CLIPBOARD_POLL_TIMEOUT {
			break;
		}
		thread::sleep(CLIPBOARD_POLL_INTERVAL);
	}
	observed
}

/// Puts the snapshot back, unless any of the conditions that gate it says not to.
///
/// A skipped restore is recorded rather than silent, and an *attempted* restore
/// that fails is the one outcome in this task that destroys user data rather than
/// merely failing to capture — so it is never swallowed.
fn restore(snapshot: &Snapshot, expected: u32, foreign: bool, evidence: &mut Evidence) {
	if foreign {
		diagnostics::log(
			"[copper] capture: the clipboard is owned by another process; withholding the restore \
			 rather than overwriting what is almost certainly something the user copied",
		);
		return;
	}
	// Checked before emptiness, not after. A clipboard holding nothing but an
	// image is both lossy *and* empty of restorable entries, and reporting that as
	// "nothing to put back" would hide the fact that Copper is walking away from
	// real content it cannot reproduce. When Copper cannot reproduce the original
	// faithfully, leaving the captured text on the clipboard is better than a
	// lossy restore that silently destroys the richer content the user had.
	if snapshot.is_lossy() {
		diagnostics::log(
			"[copper] capture: the clipboard held a format Copper cannot reproduce; withholding \
			 the restore rather than performing a lossy one",
		);
		return;
	}
	if snapshot.is_empty() {
		return;
	}

	match clipboard::restore(snapshot, expected) {
		Ok(()) => {}
		Err(ClipboardError::Superseded { .. }) => diagnostics::log(
			"[copper] capture: the clipboard changed again before the restore could take effect; \
			 restore withheld",
		),
		Err(err) => {
			evidence.clipboard_restore_failed = true;
			diagnostics::log_error(&format!(
				"[copper] capture: CLIPBOARD RESTORE FAILED — the user's previous clipboard \
				 contents are gone: {err}"
			));
		}
	}
}

// --- input -------------------------------------------------------------------

fn is_down(vk: u32) -> bool {
	// SAFETY: no preconditions. The high bit is the documented "currently down".
	(unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

/// Waits up to `MODIFIER_RELEASE_TIMEOUT` for every watched modifier to come up.
///
/// Returns false if anything is still down when the budget runs out. Defensive
/// and rarely fires: the natural gesture is press-release-press-release and the
/// trigger is the second key-up, so nothing is held by then. It exists for a
/// second tap that turns into a hold, and for a stuck Alt or Win from an earlier
/// chord.
fn wait_for_modifier_release() -> bool {
	let started = Instant::now();
	loop {
		if !WATCHED_MODIFIERS.iter().copied().any(is_down) {
			return true;
		}
		if started.elapsed() >= MODIFIER_RELEASE_TIMEOUT {
			return false;
		}
		thread::sleep(MODIFIER_POLL_INTERVAL);
	}
}

fn key_event(vk: u32, up: bool) -> INPUT {
	// SAFETY: no preconditions; an unmapped key yields scan code 0, which is
	// acceptable for the virtual-key-driven injection below.
	let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) } as u16;
	INPUT {
		r#type: INPUT_KEYBOARD,
		Anonymous: INPUT_0 {
			ki: KEYBDINPUT {
				wVk: VIRTUAL_KEY(vk as u16),
				wScan: scan,
				dwFlags: if up {
					KEYEVENTF_KEYUP
				} else {
					KEYBD_EVENT_FLAGS(0)
				},
				time: 0,
				// The tag is what lets the hook discard Copper's own injection
				// without discarding everybody else's.
				dwExtraInfo: EXTRA_INFO_SIGNATURE,
			},
		},
	}
}

/// Synthesizes `Ctrl+C` into the foreground window. Returns false on a short
/// insert.
///
/// A short return is not merely an injection failure, it is a **system-wide
/// hazard**: if the `VK_CONTROL` key-down was inserted and the matching key-up
/// was not, Ctrl is left logically stuck down for every application on the
/// desktop until something else releases it, and the user's next click is
/// silently modified. Re-sending the outstanding key-ups is therefore a recovery
/// obligation, not a diagnostic one — and it is the strongest argument for
/// waiting modifiers out rather than forcing them: the fewer synthetic modifier
/// transitions Copper makes, the fewer states it can strand.
fn send_ctrl_c() -> bool {
	let sequence = [
		key_event(VK_CONTROL, false),
		key_event(VK_C, false),
		key_event(VK_C, true),
		key_event(VK_CONTROL, true),
	];
	// SAFETY: `sequence` outlives the call and the size argument matches INPUT.
	let inserted = unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) } as usize;
	if inserted == sequence.len() {
		return true;
	}

	// UIPI blocks this against a higher-integrity target, returning 0 — and the
	// docs state explicitly that neither the return value nor GetLastError
	// identifies UIPI as the cause, which is why elevation is detected by token
	// probe rather than inferred here.
	let outstanding = outstanding_key_ups(inserted);
	if outstanding.is_empty() {
		diagnostics::log_error(&format!(
			"[copper] capture: SendInput inserted {inserted} of {} events; no key is left down",
			sequence.len()
		));
		return false;
	}

	let recovery: Vec<INPUT> = outstanding
		.iter()
		.map(|vk| key_event(*vk, true))
		.collect();
	// SAFETY: `recovery` outlives the call and the size argument matches INPUT.
	let recovered = unsafe { SendInput(&recovery, std::mem::size_of::<INPUT>() as i32) } as usize;
	if recovered == recovery.len() {
		diagnostics::log_error(&format!(
			"[copper] capture: SendInput inserted {inserted} of {} events; {recovered} recovery \
			 key-ups sent, so no modifier is left down",
			sequence.len()
		));
		return false;
	}

	// The recovery itself came up short, which is the state this whole path exists
	// to prevent: a key Copper pressed is still logically down for every
	// application on the desktop, and the user's next click or keystroke is
	// silently modified until something else releases it. There is nothing further
	// Copper can do about it — a second SendInput would fail the same way — so it
	// is reported as loudly as this layer can report anything.
	diagnostics::log_error(&format!(
		"[copper] capture: RECOVERY KEY-UPS FAILED — SendInput inserted {recovered} of {} \
		 recovery events after a short insert of {inserted}. A modifier Copper pressed may be \
		 stuck down system-wide until another key event releases it.",
		recovery.len()
	));
	false
}

/// Which keys the injection left logically **down**, given how many of its four
/// events actually went in.
///
/// The sequence is Ctrl-down, C-down, C-up, Ctrl-up, so a short insert strands
/// exactly those keys whose down went in and whose up did not. Derived from the
/// count rather than assumed: sending an unconditional pair would press-and-
/// release nothing on a zero insert and could send a stray C-up on a partial one.
/// Returned in reverse press order, which is how a person releases keys.
fn outstanding_key_ups(inserted: usize) -> Vec<u32> {
	let mut down = Vec::new();
	// Ctrl-down is event 0, and its matching up is event 3.
	if (1..=3).contains(&inserted) {
		down.push(VK_CONTROL);
	}
	// C-down is event 1, and its matching up is event 2, so C is left down only
	// when exactly those two went in.
	if inserted == 2 {
		down.push(VK_C);
	}
	down.reverse();
	down
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_trigger_key_is_in_the_modifier_wait_list() {
		// The invariant Phase 7 must preserve when the trigger becomes rebindable:
		// the trigger fires on a key-up and the fallback injects a chord, so a
		// trigger key missing from this list could corrupt the injection.
		let trigger = super::super::CAPTURE_TRIGGER;
		assert!(WATCHED_MODIFIERS.contains(&trigger.left.unwrap()));
		assert!(WATCHED_MODIFIERS.contains(&trigger.right.unwrap()));
	}

	#[test]
	fn the_injected_chord_carries_coppers_tag() {
		// Without the tag the hook sees Copper's own Ctrl+C as user input.
		for event in [key_event(VK_CONTROL, false), key_event(VK_C, true)] {
			// SAFETY: the union is written as a KEYBDINPUT by `key_event`.
			let keyboard = unsafe { event.Anonymous.ki };
			assert_eq!(keyboard.dwExtraInfo, EXTRA_INFO_SIGNATURE);
		}
	}

	#[test]
	fn key_up_events_are_flagged_as_such() {
		// SAFETY: the union is written as a KEYBDINPUT by `key_event`.
		let down = unsafe { key_event(VK_C, false).Anonymous.ki };
		// SAFETY: as above.
		let up = unsafe { key_event(VK_C, true).Anonymous.ki };
		assert_eq!(down.dwFlags, KEYBD_EVENT_FLAGS(0));
		assert_eq!(up.dwFlags, KEYEVENTF_KEYUP);
	}

	#[test]
	fn an_empty_snapshot_is_not_restored() {
		let mut evidence = Evidence::default();
		restore(&Snapshot::default(), 0, false, &mut evidence);
		assert!(!evidence.clipboard_restore_failed);
	}

	#[test]
	fn a_complete_insert_leaves_nothing_to_recover() {
		assert!(outstanding_key_ups(4).is_empty());
	}

	#[test]
	fn a_zero_insert_leaves_nothing_to_recover() {
		// UIPI refusing the whole batch is the common case. Sending key-ups here
		// would release keys the user is holding rather than keys Copper pressed.
		assert!(outstanding_key_ups(0).is_empty());
	}

	#[test]
	fn a_short_insert_releases_exactly_what_it_pressed() {
		// One event in: Ctrl is down with no matching up.
		assert_eq!(outstanding_key_ups(1), vec![VK_CONTROL]);
		// Two in: Ctrl and C are both down. Released in reverse press order.
		assert_eq!(outstanding_key_ups(2), vec![VK_C, VK_CONTROL]);
		// Three in: C has already been released, Ctrl has not.
		assert_eq!(outstanding_key_ups(3), vec![VK_CONTROL]);
	}

	#[test]
	fn no_short_insert_ever_strands_control() {
		// Ctrl stuck down system-wide is the failure this path exists to prevent,
		// and it is the one that presents as the machine breaking rather than as a
		// failed capture. Every partial insert must release it.
		for inserted in 1..4 {
			assert!(
				outstanding_key_ups(inserted).contains(&VK_CONTROL),
				"an insert of {inserted} events left Ctrl down"
			);
		}
	}
}
