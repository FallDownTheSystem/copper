//! The worker thread: one capture at a time, from trigger to note.
//!
//! It owns the cascade, the clipboard sessions and their owner windows, the
//! normalisation and the store append. It initialises **no** COM: it creates
//! message-only windows for clipboard writes, and a UI Automation client thread
//! must own no windows. That is the whole reason the UIA thread is separate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread::JoinHandle;

use tauri::{AppHandle, Manager};

// Only the debug-build accounting below logs. The capture path is silent on
// success by design, and its real failures speak through the notice.
#[cfg(debug_assertions)]
use crate::diagnostics;
use crate::store::{self, SharedStore};
use crate::win32::foreground::{our_pid, Target};
use crate::win32::integrity::{target_integrity, uiaccess_active};

use super::clipboard_fallback::try_clipboard;
use super::hook::Trigger;
use super::notice::NoticeController;
use super::uia::UiaService;
use super::{
	resolve, CaptureFailure, CaptureOutcome, CaptureStrategy, Evidence, CAPTURE_CASCADE,
	MAX_CAPTURE_CHARS, UIA_TIMEOUT,
};

/// Clears the in-flight gate on **every** exit path, including an early return
/// and a panic. Without this a single wedged capture disables the trigger for
/// the rest of the session, with no symptom beyond nothing happening.
struct InFlightGate<'a>(&'a AtomicBool);

impl Drop for InFlightGate<'_> {
	fn drop(&mut self) {
		self.0.store(false, Ordering::SeqCst);
	}
}

pub fn spawn(
	app: AppHandle,
	triggers: Receiver<Trigger>,
	in_flight: Arc<AtomicBool>,
	notice: Arc<NoticeController>,
) -> std::io::Result<JoinHandle<()>> {
	std::thread::Builder::new()
		.name("copper-capture".to_owned())
		.spawn(move || {
			let mut uia = UiaService::new();
			// Pay for COM and the automation object now rather than inside the
			// first capture, where it would not be covered by the read budget.
			uia.warm_up();

			while let Ok(trigger) = triggers.recv() {
				let _gate = InFlightGate(&in_flight);
				let before = Target::current();
				let outcome = capture_once(&app, &mut uia);
				check_focus_did_not_move(before);
				report(&outcome, trigger);
				route(&notice, outcome);
			}
		})
}

/// One capture attempt, start to finish.
fn capture_once(app: &AppHandle, uia: &mut UiaService) -> CaptureOutcome {
	let Some(target) = Target::current() else {
		return CaptureOutcome::Failed(CaptureFailure::NoForegroundWindow);
	};

	// Copper capturing from Copper is noise: the user is typing in the composer,
	// and neither a note nor a notice flashed at them is wanted. Compared on
	// process rather than window handle, which covers the panel and anything else
	// Copper ever puts on screen.
	if target.pid == our_pid() {
		return CaptureOutcome::Ignored;
	}

	let mut evidence = Evidence::default();
	let mut captured: Option<String> = None;

	for strategy in CAPTURE_CASCADE {
		// Revalidated before each strategy: the gesture fires on a key-up and the
		// modifier wait can add 300 ms, during which focus can move.
		if !target.still_current() {
			evidence.foreground_changed = true;
			break;
		}

		let result = match strategy {
			CaptureStrategy::UiAutomation => uia.read(target, UIA_TIMEOUT),
			CaptureStrategy::ClipboardFallback => try_clipboard(target),
		};
		evidence.merge(result.evidence);

		// Each strategy normalises before reporting, so "first non-empty wins" is
		// applied to normalised text.
		if let Some(text) = result.text {
			#[cfg(debug_assertions)]
			diagnostics::log(&format!(
				"[copper] capture: {} chars via {strategy:?}",
				text.chars().count()
			));
			captured = Some(text);
			break;
		}
		if result.terminal {
			break;
		}
	}

	let Some(text) = captured else {
		// A failed restore outranks whatever the precedence rule would say: it is
		// the one outcome that destroys data the user already had, rather than
		// merely failing to produce a note.
		if evidence.clipboard_restore_failed {
			return CaptureOutcome::Failed(CaptureFailure::ClipboardRestoreFailed);
		}
		// The elevation probe runs only here, after the cascade has already
		// failed, so the success path pays nothing for it.
		evidence.integrity = target_integrity(target.pid);
		evidence.uiaccess = uiaccess_active();
		return CaptureOutcome::Failed(resolve(&evidence));
	};

	// Re-checked after reading and before persisting. Saving text read from a
	// different application than the user was looking at is a silent mis-capture,
	// which is worse than no capture at all.
	if !target.still_current() {
		return CaptureOutcome::Failed(CaptureFailure::ForegroundChanged);
	}

	let chars = text.chars().count();
	if chars > MAX_CAPTURE_CHARS {
		// Refused rather than truncated: a truncation is a silent partial loss.
		return CaptureOutcome::Failed(CaptureFailure::TooLarge { chars });
	}

	match save(app, &text) {
		// The note was written and the clipboard was not put back. The one
		// successful outcome that still shows a notice: R8's silence on success is
		// explicitly qualified by "unless the user's clipboard was destroyed on
		// the way".
		CaptureOutcome::Captured if evidence.clipboard_restore_failed => {
			CaptureOutcome::CapturedWithClipboardLoss
		}
		outcome => outcome,
	}
}

/// Appends through task-003's Rust-side entry point, which pushes the undo
/// snapshot, writes atomically and emits `space-changed` with reason `capture` —
/// so an already-open panel updates with no extra work here.
fn save(app: &AppHandle, text: &str) -> CaptureOutcome {
	let Some(store) = app.try_state::<SharedStore>() else {
		// Task-003 guarantees a valid destination always exists once bootstrap has
		// run, and capture is not armed until it has. This is defensive, not
		// expected.
		return CaptureOutcome::Failed(CaptureFailure::NotSaved {
			kind: "unavailable",
		});
	};

	match store::append_capture(&store, text) {
		Ok(_) => CaptureOutcome::Captured,
		Err(err) => CaptureOutcome::Failed(CaptureFailure::NotSaved { kind: err.kind() }),
	}
}

/// Routes the outcome. Success does nothing whatsoever — no window, no sound, no
/// event. If the panel happens to be visible the new note simply appears, via the
/// store's own change event.
fn route(notice: &NoticeController, outcome: CaptureOutcome) {
	match outcome {
		CaptureOutcome::Captured | CaptureOutcome::Ignored => {}
		CaptureOutcome::CapturedWithClipboardLoss => {
			notice.show(&CaptureFailure::ClipboardRestoreFailed);
		}
		CaptureOutcome::Failed(failure) => notice.show(&failure),
	}
}

/// Debug-only proof of the rule that nothing on this path moves focus.
///
/// The task's grep assertions cover it statically: none of the four
/// focus-moving Win32 entry points appears anywhere under `capture/` or
/// `win32/`, and neither does a call to the panel module's focused reveal. But a
/// no-activate reveal that quietly activates anyway would pass every one of
/// them, so this catches that empirically. A genuine user switch mid-capture
/// also trips it, which is why it is a loud debug line and not an assertion.
#[cfg(debug_assertions)]
fn check_focus_did_not_move(before: Option<Target>) {
	let after = Target::current();
	if before != after {
		diagnostics::log_error(&format!(
			"[copper] capture: the foreground window changed across a capture \
			 ({before:?} → {after:?}) — either the user switched windows or something on this \
			 path moved focus"
		));
	}
}

#[cfg(not(debug_assertions))]
fn check_focus_did_not_move(_before: Option<Target>) {}

/// Debug-only accounting. Without it, "the hook fired while Copper was focused"
/// and "the hook never fired at all" are indistinguishable, and the manual
/// verification matrix cannot tell which strategy served which application.
#[cfg(debug_assertions)]
fn report(outcome: &CaptureOutcome, trigger: Trigger) {
	use std::sync::atomic::AtomicU64;

	static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
	let attempt = ATTEMPTS.fetch_add(1, Ordering::Relaxed) + 1;
	diagnostics::log(&format!(
		"[copper] capture: attempt {attempt} took {} ms → {outcome:?}",
		trigger.at.elapsed().as_millis()
	));
}

#[cfg(not(debug_assertions))]
fn report(_outcome: &CaptureOutcome, _trigger: Trigger) {}
