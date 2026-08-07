//! Proof that the keyboard hook is still there, and the recovery when it is not.
//!
//! Windows removes a `WH_KEYBOARD_LL` hook whose callback keeps missing
//! `LowLevelHooksTimeout` and gives the application **no way to detect that it
//! happened** — no error, no message, no return code. Before it removes the hook
//! it silently discards the offending keystrokes machine-wide. Everything above
//! this module was written around that fact; this module is the part that
//! notices.
//!
//! The probe is a single inert keystroke tagged with [`PROBE_SIGNATURE`], which
//! the callback swallows. A hook that is still installed therefore stamps
//! [`hook::probe_stamp`] within a few milliseconds, and a hook Windows has taken
//! away never sees the event at all. That is the whole test: not "is the callback
//! fast", which was never the problem, but "is the callback still being called".
//!
//! Its shape follows the notice timer's — one long-lived thread, a channel whose
//! disconnection is the shutdown signal, a join at teardown — for the reason
//! stated there: a thread per probe would be a topology nobody designed. Nothing
//! on this thread ever blocks on the main thread, which is what makes joining it
//! from the main thread's teardown safe; the two things that would, the fallback
//! re-registration and the notice, are handed off instead.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tauri::AppHandle;
use windows::Win32::UI::Input::KeyboardAndMouse::{
	MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
	KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
};

use crate::win32::keys::VK_F24;
use crate::win32::PROBE_SIGNATURE;
use crate::{diagnostics, shortcuts};

use super::hook;
use super::notice::NoticeController;
use super::CaptureFailure;

/// How often the hook is asked to prove it is still there.
///
/// Deliberately slack. The hook is either installed or it is not, and nothing
/// about the answer changes between one second and the next; probing hard would
/// buy a few seconds of detection latency at the cost of a synthetic keystroke
/// through the whole system's hook chain every time.
const PROBE_INTERVAL: Duration = Duration::from_secs(15);

/// How long a probe has to make it back through the callback.
///
/// Generous against the round trip it measures — `SendInput` to hook callback is
/// sub-millisecond — because the thing being ruled out is the machine being too
/// busy to schedule anything, and a grace window that assumed otherwise would
/// report the failure it was meant to distinguish.
const PROBE_GRACE: Duration = Duration::from_secs(2);

/// How many probes in a row must go missing before the hook is treated as gone.
///
/// One miss is not evidence. The same contention that makes the hook thread miss
/// its timeout makes this thread late reading the answer, so a threshold of one
/// would tear down and reinstall a perfectly healthy hook every time the machine
/// got busy — and a reinstall is not free: it takes the hook down, and anything
/// pressed in that window is genuinely not seen. Three costs roughly
/// three-quarters of a minute before capture comes back, which is the right side
/// of that trade for a failure that is otherwise permanent and silent.
const MISS_THRESHOLD: u32 = 3;

// --- the pure rule -----------------------------------------------------------
// No OS call below this line, so the counting is unit-testable on its own.

/// What one probe outcome means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
	/// The probe came back.
	Healthy,
	/// It did not, and that is not yet enough to act on.
	Missed,
	/// Enough of them in a row that the hook is no longer there.
	Gone,
}

/// The consecutive-miss counter behind [`MISS_THRESHOLD`].
#[derive(Debug)]
struct Misses {
	consecutive: u32,
	threshold: u32,
}

impl Misses {
	fn new(threshold: u32) -> Self {
		Self {
			consecutive: 0,
			threshold,
		}
	}

	/// Records one probe outcome.
	///
	/// The count is cleared on [`Verdict::Gone`] as well as on a success, which is
	/// what stops a reinstall that did not take from reinstalling again on every
	/// subsequent probe: the next attempt is a fresh threshold away.
	fn record(&mut self, seen: bool) -> Verdict {
		if seen {
			self.consecutive = 0;
			return Verdict::Healthy;
		}
		self.consecutive += 1;
		if self.consecutive < self.threshold {
			Verdict::Missed
		} else {
			self.consecutive = 0;
			Verdict::Gone
		}
	}
}

// --- reinstalling ------------------------------------------------------------

/// Everything needed to put a hook back, without reaching for the pipeline's own
/// lock.
///
/// The hook slot is shared with [`super::CaptureHandle`] rather than owned here,
/// so shutdown and the watchdog cannot each hold a handle to the same thread.
/// Holding the pieces directly instead of resolving `CaptureState` is what keeps
/// this thread off the lock the main thread's teardown takes while joining it.
struct Reviver {
	hook: Arc<Mutex<Option<hook::HookHandle>>>,
	tx: Sender<hook::Trigger>,
	in_flight: Arc<AtomicBool>,
	armed: Arc<AtomicBool>,
}

impl Reviver {
	/// Puts a fresh hook in place of whatever is there. Returns whether capture's
	/// double-tap trigger is working again.
	///
	/// The old handle comes down **first**. Installing over it would leave two hook
	/// threads and two `HHOOK`s, both live and both delivering every event, and the
	/// handle for the first would be dropped — taking its thread with it at some
	/// later and entirely unrelated moment.
	fn revive(&self) -> bool {
		let mut slot = super::lock(&self.hook);
		if let Some(mut previous) = slot.take() {
			previous.stop();
		}
		match hook::install(
			self.tx.clone(),
			Arc::clone(&self.in_flight),
			Arc::clone(&self.armed),
		) {
			Ok(handle) => {
				*slot = Some(handle);
				diagnostics::log("[copper] capture: the keyboard hook was reinstalled");
				true
			}
			Err(err) => {
				diagnostics::log_error(&format!(
					"[copper] capture: the keyboard hook could not be reinstalled ({err}); capture \
					 falls back to a conventional chord"
				));
				false
			}
		}
	}
}

// --- the probe ---------------------------------------------------------------

/// Injects the one keystroke the callback is meant to swallow.
///
/// Down **and** up in a single call rather than a lone key-down. On the one path
/// that matters — the hook is gone, which is exactly what this is looking for —
/// nothing swallows the event, and a key-down with no matching key-up would leave
/// F24 logically held for every application on the desktop. `clipboard_fallback`
/// treats that hazard as a recovery obligation for the same reason.
fn send_probe() -> bool {
	// SAFETY: no preconditions; an unmapped key yields scan code 0, which is
	// acceptable for a virtual-key-driven injection.
	let scan = unsafe { MapVirtualKeyW(VK_F24, MAPVK_VK_TO_VSC) } as u16;
	let event = |up: bool| INPUT {
		r#type: INPUT_KEYBOARD,
		Anonymous: INPUT_0 {
			ki: KEYBDINPUT {
				wVk: VIRTUAL_KEY(VK_F24 as u16),
				wScan: scan,
				dwFlags: if up {
					KEYEVENTF_KEYUP
				} else {
					KEYBD_EVENT_FLAGS(0)
				},
				time: 0,
				// The tag is the whole mechanism: it is what the callback matches to
				// know this event is Copper asking whether it is still alive.
				dwExtraInfo: PROBE_SIGNATURE,
			},
		},
	};

	let sequence = [event(false), event(true)];
	// SAFETY: `sequence` outlives the call and the size argument matches INPUT.
	let inserted = unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) } as usize;
	inserted == sequence.len()
}

// --- the thread --------------------------------------------------------------

/// Owns the watchdog thread.
pub struct Watchdog {
	/// Dropping the sender is the shutdown signal; the thread's `recv_timeout`
	/// reports the channel disconnected and returns from whichever wait it is in.
	stop: Mutex<Option<Sender<()>>>,
	/// Held so shutdown can wait for the thread to actually be gone. It also owns a
	/// clone of the trigger sender, and the worker's receive loop ends only once
	/// every sender has been dropped — so this is not merely tidy, it is what keeps
	/// the worker's join from blocking forever.
	thread: Mutex<Option<JoinHandle<()>>>,
}

impl Watchdog {
	pub fn start(
		app: AppHandle,
		hook: Arc<Mutex<Option<hook::HookHandle>>>,
		tx: Sender<hook::Trigger>,
		in_flight: Arc<AtomicBool>,
		armed: Arc<AtomicBool>,
		notice: Arc<NoticeController>,
	) -> Self {
		let (stop_tx, stop_rx) = mpsc::channel::<()>();
		let reviver = Reviver {
			hook,
			tx,
			in_flight,
			armed,
		};

		let spawned = thread::Builder::new()
			.name("copper-hook-watchdog".to_owned())
			.spawn(move || watch(&app, &stop_rx, &reviver, &notice));

		let (stop, thread) = match spawned {
			Ok(handle) => (Some(stop_tx), Some(handle)),
			Err(err) => {
				// Capture still works; it just loses the ability to notice that it has
				// stopped, which is the state everything before this task was in.
				diagnostics::log_error(&format!(
					"[copper] capture: could not start the hook watchdog ({err}); a hook Windows \
					 removes will go unnoticed"
				));
				(None, None)
			}
		};

		Self {
			stop: Mutex::new(stop),
			thread: Mutex::new(thread),
		}
	}

	/// Stops the thread and waits for it to be gone. Idempotent.
	pub fn shutdown(&self) {
		super::lock(&self.stop).take();
		let handle = super::lock(&self.thread).take();
		if let Some(handle) = handle {
			let _ = handle.join();
		}
	}
}

/// Sleeps for `how_long`. Returns whether the loop should keep going.
fn wait(stop: &Receiver<()>, how_long: Duration) -> bool {
	match stop.recv_timeout(how_long) {
		Err(RecvTimeoutError::Timeout) => true,
		// Nothing is ever sent on this channel — the sender being dropped is the
		// signal — but treating a value as a stop keeps the match total and the
		// meaning obvious.
		_ => false,
	}
}

fn watch(
	app: &AppHandle,
	stop: &Receiver<()>,
	reviver: &Reviver,
	notice: &Arc<NoticeController>,
) {
	let mut misses = Misses::new(MISS_THRESHOLD);

	while wait(stop, PROBE_INTERVAL) {
		let seen = if hook::alive() {
			let before = hook::probe_stamp();
			if !send_probe() {
				// `SendInput` came up short, so no probe was ever in flight. That says
				// something about the input desktop, not about the hook, and counting it
				// as a miss would reinstall a healthy hook on the strength of evidence
				// that was never gathered.
				continue;
			}
			if !wait(stop, PROBE_GRACE) {
				return;
			}
			hook::probe_stamp() != before
		} else {
			// Nothing to probe, and nothing may be injected either: with no callback to
			// swallow it, the F24 would land in whatever the user is typing. A hook
			// that is already down is itself the miss, so the same threshold that
			// paces detection also paces the retries — rather than a fresh install
			// attempt every fifteen seconds for as long as the app runs.
			false
		};

		match misses.record(seen) {
			Verdict::Healthy | Verdict::Missed => {}
			Verdict::Gone => {
				diagnostics::log_error(&format!(
					"[copper] capture: the keyboard hook missed {MISS_THRESHOLD} liveness probes in a \
					 row; reinstalling it"
				));
				let recovered = reviver.revive();

				// Off this thread, both of them, and for the same reason. Registering a
				// shortcut blocks on the main thread, and the main thread is what joins
				// this one at teardown — so doing it here is the deadlock the whole
				// module note is about. `show` only queues, but it takes the same route
				// for consistency with the ordering it and the fallback need anyway.
				let fallback_app = app.clone();
				thread::spawn(move || shortcuts::revisit_fallback(&fallback_app));
				notice.show(&CaptureFailure::HookLost { recovered });
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn one_miss_is_not_evidence() {
		let mut misses = Misses::new(3);
		assert_eq!(misses.record(false), Verdict::Missed);
		assert_eq!(misses.record(false), Verdict::Missed);
	}

	#[test]
	fn the_threshold_is_consecutive_misses_not_a_total() {
		let mut misses = Misses::new(3);
		assert_eq!(misses.record(false), Verdict::Missed);
		assert_eq!(misses.record(false), Verdict::Missed);
		// A probe that comes back says the hook is there, whatever came before it.
		assert_eq!(misses.record(true), Verdict::Healthy);
		assert_eq!(misses.record(false), Verdict::Missed);
		assert_eq!(misses.record(false), Verdict::Missed);
		assert_eq!(misses.record(false), Verdict::Gone);
	}

	#[test]
	fn three_in_a_row_is_the_verdict() {
		let mut misses = Misses::new(MISS_THRESHOLD);
		for _ in 1..MISS_THRESHOLD {
			assert_eq!(misses.record(false), Verdict::Missed);
		}
		assert_eq!(misses.record(false), Verdict::Gone);
	}

	#[test]
	fn a_reinstall_that_did_not_take_waits_a_fresh_threshold() {
		// The churn this exists to prevent: without the reset, every probe after the
		// first verdict would be the threshold-th miss and would reinstall again.
		let mut misses = Misses::new(3);
		assert_eq!(misses.record(false), Verdict::Missed);
		assert_eq!(misses.record(false), Verdict::Missed);
		assert_eq!(misses.record(false), Verdict::Gone);

		assert_eq!(misses.record(false), Verdict::Missed);
		assert_eq!(misses.record(false), Verdict::Missed);
		assert_eq!(misses.record(false), Verdict::Gone);
	}

	#[test]
	fn a_healthy_hook_never_reaches_a_verdict() {
		let mut misses = Misses::new(MISS_THRESHOLD);
		for _ in 0..50 {
			assert_eq!(misses.record(true), Verdict::Healthy);
		}
	}
}
