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
//! A healthy cycle costs [`PROBE_INTERVAL`] plus [`PROBE_GRACE`] — about
//! seventeen seconds, not the fifteen the interval alone suggests — so
//! [`MISS_THRESHOLD`] consecutive misses put detection between roughly
//! thirty-six and fifty-one seconds after the hook actually died.
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

/// How long the watchdog waits between probe cycles.
///
/// Deliberately slack. The hook is either installed or it is not, and nothing
/// about the answer changes between one second and the next; probing hard would
/// buy a few seconds of detection latency at the cost of a synthetic keystroke
/// through the whole system's hook chain every time.
///
/// Not the cycle length: a cycle that actually probes also waits
/// [`PROBE_GRACE`], so the healthy cadence is the two added together, about
/// seventeen seconds.
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
/// pressed in that window is genuinely not seen.
///
/// Three seventeen-second cycles puts detection between roughly thirty-six and
/// fifty-one seconds after the hook dies, depending on where in a cycle it went.
/// That is the right side of the trade for a failure that is otherwise permanent
/// and silent.
const MISS_THRESHOLD: u32 = 3;

/// How many cycles in a row may fail to inject a probe at all before the
/// watchdog admits it cannot vouch for the hook.
///
/// A cycle that cannot inject skips the grace wait, so eight of them is about two
/// minutes. Much longer than [`MISS_THRESHOLD`] on purpose: this is not evidence
/// of anything being wrong with the hook, only evidence that the watchdog has
/// gone blind, and the response is correspondingly weaker — the insurance chord
/// goes up, nothing is reinstalled.
const UNPROBEABLE_THRESHOLD: u32 = 8;

/// The two thresholds only mean anything relative to each other: being blind is a
/// weaker claim than being broken, so it must take longer to make. Asserted at
/// compile time because a later tuning pass that inverted them would produce a
/// watchdog that gave up before it had tried.
const _: () = assert!(UNPROBEABLE_THRESHOLD > MISS_THRESHOLD);

// --- the pure rule -----------------------------------------------------------
// No OS call below this line, so the whole decision is unit-testable on its own.

/// What one cycle observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Observation {
	/// The probe came back through the callback.
	Seen,
	/// The probe went out and did not come back — or there was no hook to send it
	/// to, which is the same evidence arrived at without spending a keystroke.
	Missing,
	/// The probe could not be put into the input stream at all.
	Unsendable,
}

/// What the loop should do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
	/// Nothing. Either all is well, or not enough has gone wrong yet.
	Wait,
	/// The hook answered again after an outage.
	Recovered,
	/// The hook is gone and must be put back.
	Reinstall { notify: bool },
	/// The probe cannot be injected, so the hook can no longer be vouched for.
	/// **Not** a reinstall: nothing here is evidence about the hook itself.
	Unprobeable { notify: bool },
}

/// The whole decision: two independent run-lengths and one latch.
#[derive(Debug)]
struct Health {
	misses: u32,
	unsendable: u32,
	/// Whether the rule is inside an outage it has already announced.
	///
	/// The latch does two jobs. It is what makes a probe that comes back produce
	/// [`Action::Recovered`] exactly once rather than on every healthy cycle, and
	/// it is what `notify` derives from: only the *first* threshold crossing of
	/// an outage carries `notify: true`, so a hook that cannot be reinstalled
	/// does not re-announce itself every fifty seconds for as long as the app
	/// runs. Cleared only by a probe that actually comes back, so a second
	/// announcement means a genuine second outage rather than the same one still
	/// going. (Whether an announcement becomes a user-facing *notice* is the
	/// loop's decision now, not this rule's — see `notified` in [`watch`].)
	reported: bool,
	miss_threshold: u32,
	unsendable_threshold: u32,
}

impl Health {
	fn new(miss_threshold: u32, unsendable_threshold: u32) -> Self {
		Self {
			misses: 0,
			unsendable: 0,
			reported: false,
			miss_threshold,
			unsendable_threshold,
		}
	}

	/// Folds one cycle's observation in and says what follows from it.
	///
	/// Both run-lengths reset when they fire, which is what stops a response that
	/// did not work from firing again on the very next cycle: the next attempt is
	/// a fresh threshold away.
	fn record(&mut self, observation: Observation) -> Action {
		match observation {
			Observation::Seen => {
				self.misses = 0;
				self.unsendable = 0;
				if !self.reported {
					return Action::Wait;
				}
				// The only path that clears the latch, and the only thing that earns
				// the right to raise a second notice later.
				self.reported = false;
				Action::Recovered
			}
			Observation::Missing => {
				self.unsendable = 0;
				self.misses += 1;
				if self.misses < self.miss_threshold {
					return Action::Wait;
				}
				self.misses = 0;
				Action::Reinstall {
					notify: !std::mem::replace(&mut self.reported, true),
				}
			}
			Observation::Unsendable => {
				self.misses = 0;
				self.unsendable += 1;
				if self.unsendable < self.unsendable_threshold {
					return Action::Wait;
				}
				self.unsendable = 0;
				Action::Unprobeable {
					notify: !std::mem::replace(&mut self.reported, true),
				}
			}
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
	///
	/// A stop that fails is recorded rather than shrugged off, and the replacement
	/// goes in either way. `HookHandle::stop` counts the orphan process-wide, which
	/// is what `CaptureHandle::shutdown` reads before deciding whether joining the
	/// worker can complete — the orphan still owns a trigger sender, and no local
	/// bookkeeping here would reach the code that has to know.
	fn revive(&self) -> bool {
		let mut slot = super::lock(&self.hook);
		if let Some(mut previous) = slot.take() {
			if !previous.stop() {
				diagnostics::log_error(
					"[copper] capture: the outgoing hook thread would not accept WM_QUIT and has been \
					 detached; installing its replacement anyway, since the orphan exists either way",
				);
			}
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

/// How much of the probe made it into the input stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Injected {
	/// Nothing went in. No probe was ever in flight, and nothing is stranded.
	Nothing,
	/// The key-down went in and its key-up did not — so the probe *is* in flight,
	/// and a key was left held until the recovery below dealt with it.
	Partial,
	Complete,
}

fn probe_event(up: bool) -> INPUT {
	// SAFETY: no preconditions; an unmapped key yields scan code 0, which is
	// acceptable for a virtual-key-driven injection.
	let scan = unsafe { MapVirtualKeyW(VK_F24, MAPVK_VK_TO_VSC) } as u16;
	INPUT {
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
	}
}

/// Injects the one keystroke the callback is meant to swallow.
///
/// Down **and** up in a single call rather than a lone key-down. On the one path
/// that matters — the hook is gone, which is exactly what this is looking for —
/// nothing swallows the event, and a key-down with no matching key-up would leave
/// F24 logically held for every application on the desktop until something else
/// released it.
///
/// A short insert is therefore a recovery obligation, not a diagnostic one, and
/// it is the same obligation `clipboard_fallback::send_ctrl_c` carries for its
/// `Ctrl`: an insert of exactly one means the down went in alone, and the
/// matching up has to be sent separately or the hazard is real. Reporting the
/// short insert and moving on would leave a key stuck down system-wide, which is
/// a far worse outcome than the missed probe it was trying to report.
fn send_probe() -> Injected {
	let sequence = [probe_event(false), probe_event(true)];
	// SAFETY: `sequence` outlives the call and the size argument matches INPUT.
	let inserted = unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) } as usize;
	if inserted == sequence.len() {
		return Injected::Complete;
	}
	if inserted == 0 {
		// UIPI and a locked desktop both land here, returning zero with nothing to
		// distinguish them by. Nothing went in, so nothing is stuck.
		diagnostics::log_error(
			"[copper] capture: the liveness probe could not be injected; no key is left down",
		);
		return Injected::Nothing;
	}

	let recovery = [probe_event(true)];
	// SAFETY: `recovery` outlives the call and the size argument matches INPUT.
	let recovered = unsafe { SendInput(&recovery, std::mem::size_of::<INPUT>() as i32) } as usize;
	if recovered == recovery.len() {
		diagnostics::log_error(
			"[copper] capture: the liveness probe inserted its key-down but not its key-up; the \
			 recovery key-up went in, so no key is left down",
		);
	} else {
		// Nothing further can be done — a third `SendInput` would fail the same way
		// — so it is reported as loudly as this layer reports anything.
		diagnostics::log_error(
			"[copper] capture: RECOVERY KEY-UP FAILED — the liveness probe's F24 key-down was \
			 inserted, its key-up was not, and the recovery key-up was refused too. F24 may be \
			 stuck down system-wide until another key event releases it.",
		);
	}
	Injected::Partial
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

/// Hands the insurance-chord decision to a thread of its own.
///
/// Off the watchdog thread because registering a shortcut blocks on the main
/// thread, and the main thread is what joins the watchdog at teardown — doing it
/// inline is the deadlock this module's header note is about.
///
/// Gated on the teardown flag rather than fired blindly. A thread that reached
/// the registry lock during exit would either register a chord the process is
/// about to drop, or hold the lock across `shortcuts::shutdown`'s `try_lock` and
/// make it skip every retirement it was there to do. `revisit_fallback` checks
/// the same flag again once it holds the lock, which is what closes the gap
/// between this check and that acquisition.
fn revisit_fallback_off_thread(app: &AppHandle) {
	if crate::shutting_down() {
		return;
	}
	let app = app.clone();
	thread::spawn(move || shortcuts::revisit_fallback(&app));
}

fn watch(
	app: &AppHandle,
	stop: &Receiver<()>,
	reviver: &Reviver,
	notice: &Arc<NoticeController>,
) {
	let mut health = Health::new(MISS_THRESHOLD, UNPROBEABLE_THRESHOLD);
	// Whether the *current* outage has produced a notice. The pure rule's own
	// latch marks the first threshold crossing, but "first crossing" and "worth
	// telling" stopped being the same question when repaired outages stopped
	// being told: a crossing whose reinstall succeeds is silent, and if the hook
	// dies again before a probe has answered, the rule's `notify` is already
	// spent — this flag is what lets the failing second reinstall still speak.
	// Cleared where the rule's latch is cleared, on a probe that comes back.
	let mut notified = false;

	while wait(stop, PROBE_INTERVAL) {
		let observation = if !hook::alive() {
			// Nothing to probe, and nothing may be injected either: with no callback
			// to swallow it, the F24 would land in whatever the user is typing. A hook
			// that is already down is itself the miss, so the same threshold that
			// paces detection also paces the reinstall attempts — rather than a fresh
			// one every cycle for as long as the app runs.
			Observation::Missing
		} else {
			let before = hook::probe_stamp();
			match send_probe() {
				// Nothing reached the input stream, so there is no probe to wait for.
				// This is evidence about the desktop, not about the hook.
				Injected::Nothing => Observation::Unsendable,
				// A partial insert still put the key-**down** into the stream, which is
				// all a probe is: if the callback is alive it has already stamped. The
				// recovery key-up carries the same tag and would stamp again.
				Injected::Partial | Injected::Complete => {
					if !wait(stop, PROBE_GRACE) {
						return;
					}
					if hook::probe_stamp() == before {
						Observation::Missing
					} else {
						Observation::Seen
					}
				}
			}
		};

		match health.record(observation) {
			Action::Wait => {}
			Action::Recovered => {
				diagnostics::log("[copper] capture: the keyboard hook is answering its probe again");
				super::set_probe_blocked(false);
				notified = false;
				// The insurance chord stood in for a hook that is back; retiring it is
				// the same call that put it up.
				revisit_fallback_off_thread(app);
			}
			Action::Reinstall { notify } => {
				diagnostics::log_error(&format!(
					"[copper] capture: the keyboard hook missed {MISS_THRESHOLD} liveness probes in a \
					 row; reinstalling it"
				));
				// A reinstall settles the question either way, so any earlier doubt
				// about the probe no longer describes anything.
				super::set_probe_blocked(false);
				let recovered = reviver.revive();
				revisit_fallback_off_thread(app);
				// Only a reinstall that FAILED is worth a notice. A hook that died and
				// went straight back is a condition that has already resolved — and
				// Windows removes hooks on its own schedule, under load, with the user
				// nowhere near a capture, so the panel appearing to announce the repair
				// read as Copper interrupting for nothing. The outage is in the log
				// either way.
				//
				// `notified` rather than the rule's `notify`, which is left unread on
				// purpose: the rule marks the first threshold crossing, but a crossing
				// whose reinstall succeeded said nothing, and the notice must still be
				// available to the crossing after it that fails.
				let _ = notify;
				if !recovered && !notified {
					notified = true;
					notice.show(&CaptureFailure::HookLost);
				}
			}
			Action::Unprobeable { notify } => {
				diagnostics::log_error(&format!(
					"[copper] capture: the liveness probe has not been injectable for \
					 {UNPROBEABLE_THRESHOLD} cycles; the hook cannot be vouched for, so the insurance \
					 chord goes up. It is **not** reinstalled — nothing here is evidence about the \
					 hook itself"
				));
				super::set_probe_blocked(true);
				revisit_fallback_off_thread(app);
				// No notice, deliberately. Blindness is not evidence the hook is
				// broken, and the *ordinary* way to become blind is the lock screen or
				// a UAC prompt: `SendInput` is refused on a secure desktop, so two
				// minutes locked used to greet the returning user with a panel
				// reporting a shortcut failure that never happened. The insurance
				// chord goes up silently and comes down on the first probe that
				// answers; the settings view names the standing condition for anyone
				// who goes looking. `notify` stays in the rule unread — the tests pin
				// the first-crossing semantics it still expresses.
				let _ = notify;
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use Observation::{Missing, Seen, Unsendable};

	/// The shipped thresholds, so the tests exercise the numbers that ship.
	fn health() -> Health {
		Health::new(MISS_THRESHOLD, UNPROBEABLE_THRESHOLD)
	}

	/// Feeds `observation` `times` over and returns the last action, asserting
	/// every earlier one was `Wait`.
	fn run(health: &mut Health, observation: Observation, times: u32) -> Action {
		let mut last = Action::Wait;
		for turn in 1..=times {
			last = health.record(observation);
			if turn < times {
				assert_eq!(last, Action::Wait, "{observation:?} acted early on turn {turn}");
			}
		}
		last
	}

	#[test]
	fn a_healthy_hook_never_acts() {
		let mut health = health();
		for _ in 0..50 {
			assert_eq!(health.record(Seen), Action::Wait);
		}
	}

	#[test]
	fn one_miss_is_not_evidence_and_the_threshold_is_consecutive() {
		let mut health = health();
		assert_eq!(health.record(Missing), Action::Wait);
		assert_eq!(health.record(Missing), Action::Wait);
		// A probe that comes back says the hook is there, whatever came before it.
		assert_eq!(health.record(Seen), Action::Wait);
		assert_eq!(run(&mut health, Missing, MISS_THRESHOLD), Action::Reinstall { notify: true });
	}

	#[test]
	fn a_reinstall_that_did_not_take_waits_a_fresh_threshold() {
		// The churn this exists to prevent: without the run-length reset, every
		// cycle after the first verdict would be the threshold-th miss and would
		// reinstall again.
		let mut health = health();
		assert_eq!(run(&mut health, Missing, MISS_THRESHOLD), Action::Reinstall { notify: true });
		assert_eq!(run(&mut health, Missing, MISS_THRESHOLD), Action::Reinstall { notify: false });
	}

	#[test]
	fn an_outage_that_never_recovers_is_reported_exactly_once() {
		// The whole point of the latch. Every `notify: true` reveals the panel and
		// plays the failure sound, so an unrecoverable hook without this would do
		// both roughly every fifty seconds for as long as the app runs.
		let mut health = health();
		assert_eq!(run(&mut health, Missing, MISS_THRESHOLD), Action::Reinstall { notify: true });
		for _ in 0..10 {
			assert_eq!(
				run(&mut health, Missing, MISS_THRESHOLD),
				Action::Reinstall { notify: false }
			);
		}
	}

	#[test]
	fn only_a_confirmed_recovery_re_arms_the_notice() {
		let mut health = health();
		assert_eq!(run(&mut health, Missing, MISS_THRESHOLD), Action::Reinstall { notify: true });
		// A reinstall alone proves nothing; the probe coming back is what does.
		assert_eq!(health.record(Seen), Action::Recovered);
		// And a recovery is announced once, not on every subsequent healthy cycle.
		assert_eq!(health.record(Seen), Action::Wait);
		// A genuinely new outage is a genuinely new notice.
		assert_eq!(run(&mut health, Missing, MISS_THRESHOLD), Action::Reinstall { notify: true });
	}

	#[test]
	fn a_blocked_probe_never_reinstalls() {
		// A probe that could not be injected says nothing about the hook, and
		// reinstall churn on a guess is worse than no reinstall at all.
		let mut health = health();
		let action = run(&mut health, Unsendable, UNPROBEABLE_THRESHOLD);
		assert_eq!(action, Action::Unprobeable { notify: true });
	}

	#[test]
	fn the_blocked_probe_threshold_is_far_slacker_than_the_miss_threshold() {
		// Being blind is a weaker claim than being broken, so it takes longer to
		// make — see the compile-time assertion next to the constants.
		let mut health = health();
		assert_eq!(run(&mut health, Unsendable, MISS_THRESHOLD), Action::Wait);
	}

	#[test]
	fn a_blocked_probe_shares_the_one_outage_latch() {
		let mut health = health();
		assert_eq!(
			run(&mut health, Unsendable, UNPROBEABLE_THRESHOLD),
			Action::Unprobeable { notify: true }
		);
		// The hook then goes for real while the outage is still open: acted on,
		// because the response differs, but not reported twice.
		assert_eq!(run(&mut health, Missing, MISS_THRESHOLD), Action::Reinstall { notify: false });
		// One recovery closes whichever outage was open.
		assert_eq!(health.record(Seen), Action::Recovered);
	}

	#[test]
	fn the_two_run_lengths_do_not_contaminate_each_other() {
		let mut health = health();
		// Misses that are interrupted by a cycle the probe could not go out on are
		// not consecutive misses: the interruption gathered no evidence either way.
		assert_eq!(health.record(Missing), Action::Wait);
		assert_eq!(health.record(Missing), Action::Wait);
		assert_eq!(health.record(Unsendable), Action::Wait);
		assert_eq!(health.record(Missing), Action::Wait);
		assert_eq!(health.record(Missing), Action::Wait);
		// Still one short, because the run restarted at the interruption.
		assert_eq!(health.record(Missing), Action::Reinstall { notify: true });
	}
}
