//! The failure notice: the only thing capture ever shows.
//!
//! Success is silent — no window, no sound, no toast, no tray balloon. The one
//! exception is a capture that succeeded but destroyed the user's clipboard on
//! the way, because an unnoticed loss is worse than an unconfirmed success and
//! that is the whole argument for this surface existing.
//!
//! # The unit of state is an *episode*, not a notice
//!
//! Recomputing "was the panel visible?" per failure is wrong in a way that is
//! easy to miss. Failure 1 finds the panel hidden, records "hide it afterwards",
//! and reveals it. Failure 2 arrives 200 ms later, finds the panel visible —
//! *because failure 1 revealed it* — and records "leave it visible". The panel
//! then stays up forever. So `owns_visibility` is decided **once**, when an
//! episode begins, and overlapping failures inherit it while restarting the
//! timer.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::diagnostics;
use crate::panel;

use super::{on_main_thread, CaptureFailure, FAILURE_NOTICE_DURATION};

const FAILED_EVENT: &str = "capture://failed";
const CLEARED_EVENT: &str = "capture://cleared";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FailedPayload {
	cause: &'static str,
	message: String,
	generation: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ClearedPayload {
	generation: u64,
}

// --- the pure rule -----------------------------------------------------------

/// What an expiry means for the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expiry {
	/// A newer failure has since arrived; this timer must do nothing at all.
	Stale,
	Clear { hide: bool },
}

/// One run of overlapping failures.
#[derive(Debug, Default, PartialEq, Eq)]
struct Episode {
	active: bool,
	/// Whether Copper is the reason the panel is on screen, and therefore whether
	/// it may put it away again. Decided once per episode.
	owns_visibility: bool,
	/// The newest failure's generation. A timer carrying anything else is stale.
	generation: u64,
}

impl Episode {
	/// Records a failure. Returns whether the panel must be revealed.
	fn begin(&mut self, generation: u64, was_visible: bool) -> bool {
		self.generation = generation;
		if !self.active {
			self.active = true;
			self.owns_visibility = !was_visible;
		}
		!was_visible
	}

	/// Records the expiry of `generation`.
	fn expire(&mut self, generation: u64) -> Expiry {
		if !self.active || self.generation != generation {
			return Expiry::Stale;
		}
		let hide = self.owns_visibility;
		*self = Episode {
			generation,
			..Episode::default()
		};
		Expiry::Clear { hide }
	}

	/// The user revealed the panel themselves. Copper must never hide a window
	/// somebody deliberately opened, so the episode gives up its claim.
	fn user_revealed(&mut self) {
		self.owns_visibility = false;
	}
}

// --- the controller ----------------------------------------------------------

struct Shared {
	app: AppHandle,
	episode: Mutex<Episode>,
	next_generation: AtomicU64,
}

impl Shared {
	fn episode(&self) -> std::sync::MutexGuard<'_, Episode> {
		super::lock(&self.episode)
	}
}

/// The timer's inbox: a generation and the deadline it expires at.
type Schedule = Sender<(u64, Instant)>;

/// Owns the notice surface and the single expiry timer.
pub struct NoticeController {
	shared: Arc<Shared>,
	/// One shared timer, not a thread per failure: a burst of failures would
	/// otherwise spawn a burst of threads and the stated thread topology would
	/// stop being true. Dropping the sender ends the timer thread.
	///
	/// Behind an `Arc` so that [`show`](NoticeController::show) can hand a `Weak`
	/// to the closure it queues onto the main thread rather than a clone. A clone
	/// would keep the channel open from inside a closure whose execution the
	/// joining thread controls, and that is a deadlock rather than a leak: on the
	/// tray-Quit path teardown runs inside `RunEvent::Exit`, where the event loop
	/// no longer dispatches queued work, so [`shutdown`](NoticeController::shutdown)
	/// would wait for a timer whose last sender sits in a closure that will never
	/// run. A `Weak` cannot upgrade once this `Arc` is dropped, so an undispatched
	/// closure holds nothing.
	schedule: Mutex<Option<Arc<Schedule>>>,
	/// Held so shutdown can wait for the timer to actually be gone rather than
	/// assume it. The controller lives in managed state and is never dropped, so
	/// without an explicit teardown the thread would outlive the pipeline it
	/// belongs to.
	timer: Mutex<Option<thread::JoinHandle<()>>>,
}

impl NoticeController {
	pub fn new(app: AppHandle) -> Self {
		let shared = Arc::new(Shared {
			app,
			episode: Mutex::new(Episode::default()),
			next_generation: AtomicU64::new(1),
		});
		let (schedule_tx, schedule_rx) = mpsc::channel::<(u64, Instant)>();

		let timer_shared = Arc::clone(&shared);
		let spawned = thread::Builder::new()
			.name("copper-notice".to_owned())
			.spawn(move || timer_loop(&timer_shared, &schedule_rx));

		let (schedule, timer) = match spawned {
			Ok(handle) => (Some(Arc::new(schedule_tx)), Some(handle)),
			Err(err) => {
				// The notice can still be shown; it just will not clear itself.
				// Better than no notice at all, and it says so.
				diagnostics::log_error(&format!(
					"[copper] capture: could not start the notice timer ({err}); \
					 failure notices will not clear on their own"
				));
				(None, None)
			}
		};

		Self {
			shared,
			schedule: Mutex::new(schedule),
			timer: Mutex::new(timer),
		}
	}

	/// Cancels any pending notice and stops the timer thread. Idempotent.
	///
	/// Dropping the sender is what the timer waits on: its `recv` reports the
	/// channel disconnected and the loop returns. Joining afterwards makes the
	/// teardown observable rather than hopeful — the thread only ever queues work
	/// onto the main thread and never blocks, so this cannot wedge shutdown.
	///
	/// That last claim only holds because [`show`](NoticeController::show) hands
	/// its closure a `Weak`. Dropping the `Arc` here is what actually closes the
	/// channel; a strong clone sitting in a queued closure would keep it open with
	/// no way for this join to ever complete.
	pub fn shutdown(&self) {
		super::lock(&self.schedule).take();
		let handle = super::lock(&self.timer).take();
		if let Some(handle) = handle {
			let _ = handle.join();
		}
	}

	/// Shows a failure notice, revealing the panel without activating it.
	///
	/// The event is emitted **before** the reveal so the notice is painted before
	/// the window becomes visible, rather than flashing an empty panel.
	pub fn show(&self, failure: &CaptureFailure) {
		let generation = self.shared.next_generation.fetch_add(1, Ordering::SeqCst);

		if let Err(err) = self.shared.app.emit(
			FAILED_EVENT,
			FailedPayload {
				cause: failure.cause(),
				message: failure.message(),
				generation,
			},
		) {
			diagnostics::log_error(&format!("[copper] capture: could not emit {FAILED_EVENT}: {err}"));
		}

		let shared = Arc::clone(&self.shared);
		// A `Weak`, so this closure cannot keep the timer's channel open. If it is
		// never dispatched — which is what happens to anything queued after the
		// event loop has stopped — the upgrade below simply fails and the timer
		// still learns to exit when `shutdown` drops the `Arc`.
		let schedule = super::lock(&self.schedule).as_ref().map(Arc::downgrade);

		// Every window operation is marshalled to the main thread; the worker
		// never touches a window handle itself.
		on_main_thread(&self.shared.app, "show a notice", move || {
			let Some(window) = shared.app.get_webview_window(panel::PANEL_LABEL) else {
				diagnostics::log_error("[copper] capture: the panel window is gone; no notice shown");
				return;
			};

			// A failed query must not leave the episode believing it owns a window
			// it never revealed, so an unknown state is treated as visible.
			let was_visible = window.is_visible().unwrap_or(true);
			let reveal = shared.episode().begin(generation, was_visible);

			if reveal {
				// The no-activate flavour, never the focused one: nothing on the
				// capture path may move focus.
				if let Err(err) = panel::reveal_without_activating(&window) {
					diagnostics::log_error(&format!(
						"[copper] capture: could not reveal the panel for a notice: {err}"
					));
				}
			}

			// Started only after the reveal decision has run, so the notice's
			// lifetime is measured from when it is actually on screen. The upgrade
			// fails once the controller has shut down, which is the right answer:
			// there is no notice left to expire.
			if let Some(schedule) = schedule.as_ref().and_then(Weak::upgrade) {
				let _ = schedule.send((generation, Instant::now() + FAILURE_NOTICE_DURATION));
			}
		});
	}

	/// The user revealed the panel themselves — tray click today, summon in
	/// Phase 7. The current episode gives up its claim on the window.
	pub fn user_revealed(&self) {
		self.shared.episode().user_revealed();
	}
}

fn timer_loop(shared: &Arc<Shared>, schedule: &mpsc::Receiver<(u64, Instant)>) {
	let mut pending: Option<(u64, Instant)> = None;
	loop {
		let received = match pending {
			Some((_, deadline)) => {
				schedule.recv_timeout(deadline.saturating_duration_since(Instant::now()))
			}
			None => schedule.recv().map_err(|_| RecvTimeoutError::Disconnected),
		};

		match received {
			// A newer failure. Overlapping failures reset the timer rather than
			// stacking, and inherit the episode's `owns_visibility`.
			Ok(next) => pending = Some(next),
			Err(RecvTimeoutError::Timeout) => {
				if let Some((generation, _)) = pending.take() {
					expire(shared, generation);
				}
			}
			Err(RecvTimeoutError::Disconnected) => return,
		}
	}
}

fn expire(shared: &Arc<Shared>, generation: u64) {
	let shared = Arc::clone(shared);
	let app = shared.app.clone();
	on_main_thread(&app, "clear a notice", move || {
		// Re-checked **inside** the main-thread closure, not only on the timer
		// thread. Checking on the timer thread alone leaves a gap: a stale timer
		// can pass its check, enqueue the hide, and have a newer failure land
		// before the closure runs.
		let expiry = shared.episode().expire(generation);
		let Expiry::Clear { hide } = expiry else {
			return;
		};

		// Carries the generation so a stale clear cannot clear a newer message.
		if let Err(err) = shared
			.app
			.emit(CLEARED_EVENT, ClearedPayload { generation })
		{
			diagnostics::log_error(&format!(
				"[copper] capture: could not emit {CLEARED_EVENT}: {err}"
			));
		}
		if hide {
			// Through panel::hide, so there is one hide path.
			panel::hide_or_log(&shared.app);
		}
	});
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_failure_from_a_hidden_panel_reveals_it_and_hides_it_again() {
		let mut episode = Episode::default();
		assert!(episode.begin(1, false), "a hidden panel must be revealed");
		assert_eq!(episode.expire(1), Expiry::Clear { hide: true });
	}

	#[test]
	fn a_panel_already_visible_at_episode_start_stays_visible() {
		let mut episode = Episode::default();
		assert!(!episode.begin(1, true), "a visible panel is not revealed again");
		assert_eq!(episode.expire(1), Expiry::Clear { hide: false });
	}

	#[test]
	fn three_rapid_failures_from_a_hidden_panel_still_hide_it_once() {
		// The case a per-failure visibility flag gets wrong: failure 2 and 3 find
		// the panel visible *because failure 1 revealed it*, and would each record
		// "leave it visible" — leaving the panel up forever.
		let mut episode = Episode::default();
		assert!(episode.begin(1, false));
		episode.begin(2, true);
		episode.begin(3, true);

		// The first two timers are stale and must do nothing.
		assert_eq!(episode.expire(1), Expiry::Stale);
		assert_eq!(episode.expire(2), Expiry::Stale);
		// Only the last one clears, and it still owns the window.
		assert_eq!(episode.expire(3), Expiry::Clear { hide: true });
	}

	#[test]
	fn a_stale_timer_cannot_hide_a_freshly_revealed_panel() {
		let mut episode = Episode::default();
		episode.begin(1, false);
		episode.begin(2, true);
		assert_eq!(episode.expire(1), Expiry::Stale);
		// And the episode is still live for its own timer.
		assert_eq!(episode.expire(2), Expiry::Clear { hide: true });
	}

	#[test]
	fn a_user_reveal_mid_episode_cancels_the_hide() {
		let mut episode = Episode::default();
		episode.begin(1, false);
		episode.user_revealed();
		assert_eq!(
			episode.expire(1),
			Expiry::Clear { hide: false },
			"Copper must never hide a window the user deliberately opened"
		);
	}

	#[test]
	fn a_new_episode_after_one_cleared_decides_visibility_afresh() {
		let mut episode = Episode::default();
		episode.begin(1, false);
		assert_eq!(episode.expire(1), Expiry::Clear { hide: true });

		// The panel is hidden again, so the next episode owns it too.
		assert!(episode.begin(2, false));
		assert_eq!(episode.expire(2), Expiry::Clear { hide: true });
	}

	#[test]
	fn expiring_twice_does_nothing_the_second_time() {
		let mut episode = Episode::default();
		episode.begin(1, false);
		assert_eq!(episode.expire(1), Expiry::Clear { hide: true });
		assert_eq!(episode.expire(1), Expiry::Stale);
	}

	#[test]
	fn an_expiry_with_no_episode_at_all_is_stale() {
		let mut episode = Episode::default();
		assert_eq!(episode.expire(7), Expiry::Stale);
	}
}
