//! Capture: the narrow interface between the app and everything Win32 does to
//! turn a double-tap of Shift into a note.
//!
//! The rest of the app sees [`start_capture`], [`CaptureHandle`], and two plain
//! enums. Nothing window-handle shaped crosses this boundary — that is checked
//! by grep rather than by trait, which is why the rule is written as a grep
//! assertion in the task rather than as a principle.
//!
//! # Six threads
//!
//! Four carry a capture; the fifth only expires notices, and the sixth only
//! proves the hook is still installed. Neither of the last two is incidental:
//! the notice timer exists precisely so a burst of failures cannot spawn a
//! thread each, and the watchdog exists because Windows removes a hook without
//! telling anybody. A topology that did not mention them would be describing a
//! design nobody built.
//!
//! ```text
//!   hook thread            worker thread              UIA thread        main thread
//!   (message pump)         (owns clipboard windows)   (COM MTA,         (Tauri loop)
//!    hook proc:              recv()                    owns no window)   run_on_main_thread:
//!     ignore own tag         CAPTURE_CASCADE ─────▶    GetFocusedElement   ShowWindow(NOACTIVATE)
//!     double-tap timing       ├ uia  ◀── recv_timeout ─ TextPattern        SetWindowPos(NOACTIVATE)
//!     IN_FLIGHT gate          └ clipboard fallback      GetSelection       panel::hide()
//!     send()  ──────────▶    normalise + revalidate    (abandoned on
//!     CallNextHookEx         store::append_capture      timeout, never
//!                              ok  → nothing            joined)
//!                              err → notice ───────────────────────────▶ emit + reveal
//!                                                                            │
//!                              notice timer thread ◀── one deadline ─────────┘
//!                              (one, shared)      ──── expiry ──────────────▶ emit + hide
//!
//!   watchdog thread
//!    every 15s: SendInput(F24, PROBE_SIGNATURE) ──▶ hook proc swallows it
//!    +2s: did the probe stamp move? ◀───────────────────────┘
//!    3 misses → reinstall the hook, revisit the fallback chord, raise a notice
//! ```
//!
//! The hook procedure is trivial on purpose: Windows silently uninstalls a
//! low-level hook whose callback exceeds `LowLevelHooksTimeout` (capped at
//! 1000 ms), with no way for the application to notice. The UIA thread is
//! separate from the worker because a UI Automation client must run MTA on a
//! thread that owns no windows, while the worker must create message-only
//! windows to own its clipboard writes. One thread cannot satisfy both, and
//! merging them is the change most likely to produce intermittent UIA hangs.

mod clipboard_fallback;
mod hook;
mod notice;
mod uia;
mod watchdog;
mod worker;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Listener, Manager};

use crate::diagnostics;
use crate::win32::integrity::TargetIntegrity;

// --- tuning constants --------------------------------------------------------
// One named place each. No numeric literal for any of these appears at a call
// site; the clipboard module's own open-retry budget lives with the clipboard
// because it serves Phase 5's copy commands too, which are not captures.

/// The cascade order, as a single named constant.
///
/// dsgn-001 commits to this specifically so that reordering — should the
/// Chromium accessibility cost prove real in practice — is a one-line change
/// rather than a redesign. Do not inline this order into the dispatcher.
pub const CAPTURE_CASCADE: [CaptureStrategy; 2] =
	[CaptureStrategy::UiAutomation, CaptureStrategy::ClipboardFallback];

/// The trigger, as Phase 7 made it: a runtime selector rather than the constant
/// task-005 shipped.
///
/// Re-exported from `hook` so the rest of the app never names the hook module.
/// [`watch`] points the recogniser at a different modifier — or at nothing, when
/// task-008's capture binding is a conventional chord serviced by
/// `tauri-plugin-global-shortcut` and delivered through [`request_capture`].
pub use hook::{watch, ModifierFamily};

/// How long the clipboard fallback waits for the sequence number to reach its
/// expected next value after the injected `Ctrl+C`.
const CLIPBOARD_POLL_TIMEOUT: Duration = Duration::from_millis(200);
const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long to wait for physically-held modifiers to come up before giving up
/// and reporting `ModifierHeld`. Copper never injects key-ups for keys the user
/// is holding — see [`clipboard_fallback`] for why that was rejected.
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(300);

/// Maximum duration of one press, key-down to its own key-up. Longer is a hold,
/// not a tap.
const TAP_MAX_MS: u32 = 250;
/// Maximum gap between the first key-up and the second key-down. Separate from
/// [`TAP_MAX_MS`] on purpose: a single start-to-finish window conflates a slow
/// deliberate press with a slow hold.
const GAP_MAX_MS: u32 = 400;

/// The external bound on a UI Automation read. Not a client-settable UIA
/// timeout — task-001 established that none exists.
const UIA_TIMEOUT: Duration = Duration::from_millis(800);

/// A whole-document selection would bloat the space file and the renderer. An
/// over-large selection is refused with a notice rather than truncated:
/// truncation is a silent partial loss, which is the failure mode the design's
/// visible-only-on-failure decision exists to prevent.
const MAX_CAPTURE_CHARS: usize = 100_000;

/// How long a failure notice stays on screen.
const FAILURE_NOTICE_DURATION: Duration = Duration::from_millis(1500);

// --- outcomes ----------------------------------------------------------------

/// Which strategy the dispatcher is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStrategy {
	UiAutomation,
	ClipboardFallback,
}

/// What one capture attempt produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcome {
	/// A note was written. The app does nothing whatsoever.
	Captured,
	/// A note was written, but the user's previous clipboard could not be put
	/// back. The one successful outcome that still shows a notice.
	CapturedWithClipboardLoss,
	/// Copper itself was in the foreground. No note, no notice, no sound.
	Ignored,
	Failed(CaptureFailure),
}

/// Why a capture produced no note — or, for `ClipboardRestoreFailed`, what it
/// cost on the way to producing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFailure {
	NoForegroundWindow,
	ElevatedTarget,
	InaccessibleTarget,
	ModifierHeld,
	ForegroundChanged,
	/// The restore was attempted and failed: the user's prior clipboard is lost.
	ClipboardRestoreFailed,
	NoSelection,
	NonTextSelection,
	Unsupported,
	ClipboardBusy,
	TooLarge {
		chars: usize,
	},
	/// The store refused the write. `kind` is task-003's `StoreError::kind()`.
	NotSaved {
		kind: &'static str,
	},
	/// Windows removed the keyboard hook and the watchdog caught it. The only
	/// variant that describes no particular capture — it reports that the trigger
	/// itself stopped existing, which is otherwise indistinguishable from a user
	/// who has not double-tapped anything lately.
	HookLost {
		/// Whether reinstalling it worked.
		recovered: bool,
	},
}

impl CaptureFailure {
	/// What the user is told. Total over the enum, and every variant returns a
	/// distinct non-empty string.
	pub fn message(&self) -> String {
		match self {
			Self::NoForegroundWindow => "Nothing was in the foreground to capture from.".to_owned(),
			Self::ElevatedTarget => "Copper can't read from apps running as administrator.".to_owned(),
			Self::InaccessibleTarget => "Copper couldn't reach that window.".to_owned(),
			Self::ModifierHeld => "Let go of the modifier keys and try again.".to_owned(),
			Self::ForegroundChanged => {
				"You switched windows before Copper could read the selection.".to_owned()
			}
			// Deliberately does not claim a note was saved. This variant is
			// reachable both alongside a successful capture and on a failed one —
			// a file copied from Explorer puts `CF_HDROP` on the clipboard, so the
			// sequence moves, the read finds no text, and the restore still runs —
			// and a message that named a note would be a lie on that path.
			Self::ClipboardRestoreFailed => "Copper couldn't put your clipboard back.".to_owned(),
			Self::NoSelection => "Nothing was selected.".to_owned(),
			Self::NonTextSelection => "The selection wasn't text.".to_owned(),
			Self::Unsupported => "This app didn't give Copper anything to capture.".to_owned(),
			Self::ClipboardBusy => "The clipboard was busy. Try again.".to_owned(),
			Self::TooLarge { chars } => {
				format!("That selection is too large to capture ({chars} characters).")
			}
			// The one variant with more than one message. A conflict resolves
			// itself on a retry; a parse failure means the space file needs fixing
			// by hand and task-003 refuses all writes to it until then; an
			// unavailable space is Phase 6's case, where the active space's file has
			// gone out of reach. None is actionable the way a generic I/O error is.
			//
			// A fourth arm rather than a new `CaptureFailure` variant, deliberately:
			// this module knows nothing about spaces, and that boundary is what keeps
			// its interface narrow. `unavailable` is already one of task-003's error
			// kinds, so the case was reachable before it had wording of its own.
			Self::NotSaved { kind } => match *kind {
				"conflict" => "Couldn't save — the space file kept changing.".to_owned(),
				"parse" => {
					"Couldn't save — Copper won't overwrite a space file it can't read.".to_owned()
				}
				"unavailable" => "Couldn't save — the active space isn't available.".to_owned(),
				_ => "Captured, but couldn't save it.".to_owned(),
			},
			// Two outcomes, two sentences. Telling the user their trigger is broken
			// when it has already been fixed would send them to the settings view for
			// nothing; telling them it was fixed when it was not is worse, because the
			// double-tap they then try does nothing at all.
			Self::HookLost { recovered: true } => {
				"Copper's keyboard shortcut stopped responding and had to be restarted.".to_owned()
			}
			Self::HookLost { recovered: false } => {
				"Copper's keyboard shortcut stopped working, so double-tapping no longer captures. \
				 Settings shows the key combination standing in for it."
					.to_owned()
			}
		}
	}

	/// The variant name in kebab-case, carried on `capture://failed` so the
	/// frontend can diverge on styling or wording later with no Rust change.
	///
	/// Exhaustive with no wildcard arm: a new variant must be added here, which
	/// is what makes the message test's expected count derive from the enum
	/// rather than from a number someone remembered to update.
	pub fn cause(&self) -> &'static str {
		match self {
			Self::NoForegroundWindow => "no-foreground-window",
			Self::ElevatedTarget => "elevated-target",
			Self::InaccessibleTarget => "inaccessible-target",
			Self::ModifierHeld => "modifier-held",
			Self::ForegroundChanged => "foreground-changed",
			Self::ClipboardRestoreFailed => "clipboard-restore-failed",
			Self::NoSelection => "no-selection",
			Self::NonTextSelection => "non-text-selection",
			Self::Unsupported => "unsupported",
			Self::ClipboardBusy => "clipboard-busy",
			Self::TooLarge { .. } => "too-large",
			Self::NotSaved { .. } => "not-saved",
			Self::HookLost { .. } => "hook-lost",
		}
	}
}

// --- evidence and the precedence rule ----------------------------------------

/// What the cascade learned on its way to producing nothing.
///
/// Accumulated across every strategy attempted, then resolved to exactly one
/// cause by [`resolve`]. Collecting evidence and deciding once is what lets the
/// elevation probe run only on the failure path — the success path pays nothing
/// for it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Evidence {
	/// UIA found an insertion point but nothing selected.
	pub caret_without_selection: bool,
	/// UIA found no `TextPattern`, or a control supporting no text selection.
	pub no_text_pattern: bool,
	/// A strategy returned text that was empty once normalised.
	pub empty_after_normalisation: bool,
	/// The clipboard changed but yielded no usable text.
	pub clipboard_changed_but_untextual: bool,
	/// `OpenClipboard` never succeeded.
	pub clipboard_never_opened: bool,
	/// The clipboard sequence number never moved.
	pub clipboard_unchanged: bool,
	/// `SendInput` came up short and the recovery had to release keys — the
	/// stranded-modifier hazard. Read by nothing today; kept as the record of the
	/// one input-state failure the short-insert recovery exists for.
	pub send_input_failed: bool,
	/// A modifier was still down past `MODIFIER_RELEASE_TIMEOUT`.
	pub modifier_held: bool,
	/// The foreground window or its process changed mid-capture.
	pub foreground_changed: bool,
	/// The UIA read exceeded `UIA_TIMEOUT` and its thread was abandoned.
	pub uia_timed_out: bool,
	/// The restore was attempted and failed — user data lost.
	pub clipboard_restore_failed: bool,
	/// How the target's integrity compares with ours. Filled in only after the
	/// cascade has failed.
	pub integrity: TargetIntegrity,
	/// Whether our own process token has UIAccess active.
	pub uiaccess: bool,
}

impl Evidence {
	/// Folds one strategy's findings into the running total. Flags only ever go
	/// from false to true: a later strategy cannot un-see what an earlier one
	/// established.
	fn merge(&mut self, other: Evidence) {
		self.caret_without_selection |= other.caret_without_selection;
		self.no_text_pattern |= other.no_text_pattern;
		self.empty_after_normalisation |= other.empty_after_normalisation;
		self.clipboard_changed_but_untextual |= other.clipboard_changed_but_untextual;
		self.clipboard_never_opened |= other.clipboard_never_opened;
		self.clipboard_unchanged |= other.clipboard_unchanged;
		self.send_input_failed |= other.send_input_failed;
		self.modifier_held |= other.modifier_held;
		self.foreground_changed |= other.foreground_changed;
		self.uia_timed_out |= other.uia_timed_out;
		self.clipboard_restore_failed |= other.clipboard_restore_failed;
	}
}

/// What one strategy produced.
pub struct StrategyResult {
	/// Normalised and non-empty, or `None`. Each strategy normalises its own
	/// result before reporting success, so the dispatcher's "first non-empty
	/// wins" test is applied to normalised text — without that, a whitespace-only
	/// UIA return stops the cascade, normalises to empty, and is then misreported
	/// as `Unsupported` when it is really `NoSelection`.
	pub text: Option<String>,
	pub evidence: Evidence,
	/// Stop the cascade even without text.
	///
	/// Inert in Phase 4 and deliberately so: R-Q2 resolved that the cascade never
	/// terminates on a trusted-empty UIA answer, so the injected `Ctrl+C` runs
	/// everywhere — accepting copy-current-line in VS Code and Cursor and a
	/// command interrupt in Windows Terminal. The field is the seam for revisiting
	/// that; nothing in this phase sets it.
	pub terminal: bool,
}

impl StrategyResult {
	fn nothing(evidence: Evidence) -> Self {
		Self {
			text: None,
			evidence,
			terminal: false,
		}
	}

	fn captured(text: String, evidence: Evidence) -> Self {
		Self {
			text: Some(text),
			evidence,
			terminal: false,
		}
	}
}

/// Resolves accumulated evidence to exactly one reported cause.
///
/// Deterministic and total. `Unsupported` is the terminal fallback and is
/// reachable only when nothing else is known.
///
/// The elevation arm needs **both** halves. A target above us proves nothing on
/// its own — a signed installed build with `uiAccess="true"` reaches elevated
/// windows normally — and a target whose token could not be read proves nothing
/// either, which is why `Unknown` gets its own cause instead of being folded
/// into the administrator wording.
///
/// **Orchestrator ruling, 2026-08-05: rule 4's precedence is amended.** As R11
/// was originally written, an `Unknown` probe outranked everything the cascade
/// had actually observed — so a caret with no selection in a process whose token
/// happens to be unreadable (Discord and `audiodg.exe` are the measured cases,
/// both running at ordinary integrity) reported "Copper couldn't reach that
/// window." instead of "Nothing was selected.". The rule's original wording
/// targeted the self-UIAccess probe, which in this design cannot fail. Positive
/// cascade evidence now outranks an unreadable-target probe, and
/// `InaccessibleTarget` sits immediately above the terminal fallback: it is what
/// Copper says when it observed nothing *and* could not find out why.
pub fn resolve(evidence: &Evidence) -> CaptureFailure {
	if evidence.foreground_changed {
		CaptureFailure::ForegroundChanged
	} else if evidence.modifier_held {
		CaptureFailure::ModifierHeld
	} else if evidence.integrity == TargetIntegrity::Higher && !evidence.uiaccess {
		CaptureFailure::ElevatedTarget
	} else if evidence.caret_without_selection || evidence.empty_after_normalisation {
		CaptureFailure::NoSelection
	} else if evidence.clipboard_changed_but_untextual {
		CaptureFailure::NonTextSelection
	} else if evidence.clipboard_never_opened {
		CaptureFailure::ClipboardBusy
	} else if evidence.integrity == TargetIntegrity::Unknown {
		CaptureFailure::InaccessibleTarget
	} else {
		CaptureFailure::Unsupported
	}
}

// --- text normalisation ------------------------------------------------------

/// CRLF and lone CR become LF; leading and trailing whitespace goes.
///
/// The space file is written for git, and literal `\r\n` escapes inside JSON
/// string bodies make diffs noisy. This does not contradict "the body is opaque
/// Markdown" — that is about the app not *parsing* the body. Task-003's
/// `add_note` also trims, so the double trim is harmless.
pub fn normalise(text: &str) -> String {
	// Trimmed first, then unified. The two commute: `\r` is whitespace and maps
	// to `\n`, which is also whitespace, so neither order can trim a different
	// set of characters. Doing it this way round means the no-`\r` path copies
	// once instead of twice, and the `\r` path rewrites the shorter string.
	let trimmed = text.trim();
	if trimmed.contains('\r') {
		trimmed.replace("\r\n", "\n").replace('\r', "\n")
	} else {
		trimmed.to_owned()
	}
}

// --- lifecycle ---------------------------------------------------------------

/// Everything the capture pipeline owns, for as long as the app runs.
///
/// Opaque: nothing window-handle shaped is in it. What is: the hook thread, the
/// worker thread and the trigger channel that ends it, the notice controller,
/// and the two flags that arm and stop the pipeline.
pub struct CaptureHandle {
	/// Shared with the watchdog, which replaces what is in it when Windows takes
	/// the hook away. Behind a lock rather than owned outright because the two
	/// would otherwise each hold a handle to the same thread.
	hook: Arc<Mutex<Option<hook::HookHandle>>>,
	watchdog: watchdog::Watchdog,
	worker: Option<std::thread::JoinHandle<()>>,
	/// Dropping this closes the trigger channel, which is how the worker learns
	/// to exit.
	trigger_tx: Option<mpsc::Sender<hook::Trigger>>,
	notice: Arc<notice::NoticeController>,
	armed: Arc<AtomicBool>,
	/// Held here as well as inside the hook so [`request_capture`] can take the
	/// same one-at-a-time gate the hook takes. Without that, a conventional
	/// capture chord and a double-tap could each start a capture at once.
	in_flight: Arc<AtomicBool>,
	shut_down: AtomicBool,
}

impl CaptureHandle {
	/// Lets triggers through. Called once, after every startup gate has cleared.
	fn arm(&self) {
		if !self.armed.swap(true, Ordering::SeqCst) {
			diagnostics::log("[copper] capture: armed");
		}
	}

	/// Stops the pipeline. Idempotent, so the explicit call on Tauri's exit event
	/// and the drop of managed state cannot double-join.
	pub fn shutdown(&mut self) {
		if self.shut_down.swap(true, Ordering::SeqCst) {
			return;
		}
		// New triggers stop first, so nothing can be queued behind the shutdown.
		self.armed.store(false, Ordering::SeqCst);
		// Before the hook, and joined rather than merely signalled: the watchdog owns
		// a trigger sender of its own, and the worker's receive loop below ends only
		// when every sender has been dropped. It would also reinstall the very hook
		// this is about to stop.
		self.watchdog.shutdown();
		// A hook that never installed holds no sender, so there is nothing to stop
		// and nothing keeping the worker's channel open.
		let hook_stopped = lock(&self.hook).as_mut().is_none_or(hook::HookHandle::stop);
		// Closing the channel is what ends the worker's receive loop.
		self.trigger_tx.take();

		match self.worker.take() {
			// The worker waits for every trigger sender to drop. A hook thread that
			// could not be stopped is still running and still owns one, so that will
			// never happen and the join would block until the process is killed —
			// which is exactly the hang this shutdown exists to avoid. The worker is
			// idle and holds nothing the OS will not reclaim, so leaving it is the
			// cheaper of the two failures.
			Some(worker) if hook_stopped => {
				let _ = worker.join();
			}
			Some(_) => diagnostics::log_error(
				"[copper] capture: the hook thread could not be stopped, so it still holds the \
				 trigger channel open; leaving the worker thread rather than blocking exit on a \
				 join that cannot complete",
			),
			None => {}
		}

		// The controller lives in managed state and outlives this handle, so its
		// timer has to be stopped explicitly rather than by drop.
		self.notice.shutdown();

		// The UIA thread belongs to the worker and uninitialises COM on its own
		// thread as the worker unwinds. Abandoned UIA threads are never joined.
	}
}

impl Drop for CaptureHandle {
	fn drop(&mut self) {
		self.shutdown();
	}
}

/// Starts the capture pipeline, with the hook installed but **not armed**.
///
/// An explicit startup protocol rather than fire-and-forget spawning: the worker
/// starts first so a trigger can never arrive at a channel nobody is reading,
/// and hook installation is waited on and reported. A hook that failed to
/// install presents as a trigger that simply never fires, which is the hardest
/// possible thing to diagnose from the far end.
///
/// Arming waits on the frontend's readiness signal (task-005 R23 gate 3), which
/// is also where task-007's awaited cold-launch argv open attaches when it
/// exists: until the space the user double-clicked is open, a capture would land
/// in the default space — silently, since success produces nothing.
pub fn start_capture(app: &AppHandle) -> Result<CaptureHandle, Box<dyn std::error::Error>> {
	let (trigger_tx, trigger_rx) = mpsc::channel::<hook::Trigger>();
	let in_flight = Arc::new(AtomicBool::new(false));
	let armed = Arc::new(AtomicBool::new(false));
	let notice = Arc::new(notice::NoticeController::new(app.clone()));

	// Managed so the tray's reveal can reach it: a notice must never hide a window
	// the user deliberately opened.
	app.manage(Arc::clone(&notice));

	let worker = worker::spawn(
		app.clone(),
		trigger_rx,
		Arc::clone(&in_flight),
		Arc::clone(&notice),
	)?;

	// A failed install is **not** fatal from Phase 7 onwards. task-005 returned
	// `Err` here and `setup()` propagated it, which for an app that starts hidden
	// means the process exits with no window and no tray — every other feature
	// lost because one of them could not start. task-008's fallback-chord
	// insurance is the alternative: the pipeline stays up, `hook_alive` reports
	// false, and `shortcuts` registers a conventional chord that reaches the same
	// worker through `request_capture`.
	let hook = Arc::new(Mutex::new(
		match hook::install(
			trigger_tx.clone(),
			Arc::clone(&in_flight),
			Arc::clone(&armed),
		) {
			Ok(hook) => Some(hook),
			Err(err) => {
				diagnostics::log_error(&format!(
					"[copper] capture: the keyboard hook could not be installed ({err}); the \
					 double-tap trigger is unavailable and capture falls back to a conventional chord"
				));
				None
			}
		},
	));

	// Started even when the install above failed, because a failed install is the
	// one state it can recover from without anybody pressing anything.
	let watchdog = watchdog::Watchdog::start(
		app.clone(),
		Arc::clone(&hook),
		trigger_tx.clone(),
		Arc::clone(&in_flight),
		Arc::clone(&armed),
		Arc::clone(&notice),
	);

	Ok(CaptureHandle {
		hook,
		watchdog,
		worker: Some(worker),
		trigger_tx: Some(trigger_tx),
		notice,
		armed,
		in_flight,
		shut_down: AtomicBool::new(false),
	})
}

/// Whether the `WH_KEYBOARD_LL` hook is installed **and still there**.
///
/// Read by `shortcuts` to decide whether the fallback chord is needed, and by
/// `get_shortcut_state` so the settings view can say why a double-tap binding is
/// not working.
///
/// This used to ask whether `start_capture` had produced a handle, which is a
/// question answered once at startup and never revisited — so a hook Windows
/// removed an hour later still reported `true`, the insurance chord was never
/// registered, and capture stopped working with nothing said. The answer now
/// comes from the flag the watchdog maintains, which covers both halves: the
/// startup install and every liveness probe since.
///
/// An atomic rather than the pipeline's own state, so that `shortcuts` can read
/// it while holding its registry lock. Reaching through `CaptureState` meant
/// taking a second lock under the first, in the one module whose header rule is
/// about exactly that.
pub fn hook_alive() -> bool {
	hook::alive()
}

/// Starts a capture from outside the hook — the conventional-chord capture
/// binding R-Q52 allows, and the fallback chord that keeps capture reachable
/// when the hook could not be installed.
///
/// Goes through the same arm gate, the same one-at-a-time flag and the same
/// channel the hook procedure uses, so the two entry points cannot produce two
/// overlapping captures or two notes from one gesture.
pub fn request_capture(app: &AppHandle) {
	let Some(state) = app.try_state::<CaptureState>() else {
		return;
	};
	let handle = lock(&state.0);
	if !handle.armed.load(Ordering::SeqCst) {
		return;
	}
	if handle
		.in_flight
		.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
		.is_err()
	{
		return;
	}
	let sent = handle
		.trigger_tx
		.as_ref()
		.is_some_and(|tx| tx.send(hook::Trigger { at: Instant::now() }).is_ok());
	if !sent {
		// The worker is gone; do not leave the gate latched shut.
		handle.in_flight.store(false, Ordering::SeqCst);
	}
}

/// Arms capture when the frontend reports its notice listeners are registered.
///
/// Tauri events are not buffered, so a failure arriving before those listeners
/// resolve would reveal an empty panel — the exact flash the emit-before-reveal
/// ordering exists to prevent. An event rather than a command: this task adds no
/// `#[tauri::command]`, and the panel window already holds `core:event:default`.
pub fn arm_when_frontend_ready(app: &AppHandle) {
	let armed_app = app.clone();
	app.listen_any(FRONTEND_READY_EVENT, move |_| {
		if let Some(handle) = armed_app.try_state::<CaptureState>() {
			lock(&handle.0).arm();
		}
	});
}

/// The event the frontend emits once both notice listeners have resolved.
pub const FRONTEND_READY_EVENT: &str = "capture://ready";

/// The user revealed the panel themselves, so a notice episode in progress gives
/// up its claim on the window.
///
/// Called from `panel.rs`'s reveal paths — the tray today, summon in Phase 7 —
/// rather than from each caller, so a future reveal path cannot forget.
pub fn panel_revealed_by_user<M: Manager<tauri::Wry>>(app: &M) {
	if let Some(notice) = app.try_state::<Arc<notice::NoticeController>>() {
		notice.user_revealed();
	}
}

/// The handle as the app holds it. A `Mutex` because shutdown mutates.
pub struct CaptureState(pub std::sync::Mutex<CaptureHandle>);

/// Locks through a poisoned mutex rather than panicking.
///
/// Everything behind these locks is small owned state — an episode's flags, a
/// timer handle, the pipeline handle — and a panic elsewhere cannot leave any of
/// it in a shape that makes reading it dangerous. Propagating the poison instead
/// would turn one panic into a capture pipeline that can never again be armed or
/// shut down, which is strictly worse. `store::lock` takes the same view of the
/// same problem for the same reason.
fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
	mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Stops capture, if it started. Idempotent.
pub fn shutdown(app: &AppHandle) {
	if let Some(state) = app.try_state::<CaptureState>() {
		lock(&state.0).shutdown();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashMap;

	/// Task-003's six `StoreError::kind()` values, so the branches that collapse
	/// onto the generic arm are covered rather than assumed.
	const STORE_ERROR_KINDS: [&str; 6] = [
		"not-found",
		"io",
		"parse",
		"conflict",
		"invalid",
		"unavailable",
	];

	/// One sample of every variant, plus every `NotSaved` kind.
	fn every_failure() -> Vec<CaptureFailure> {
		let mut all = vec![
			CaptureFailure::NoForegroundWindow,
			CaptureFailure::ElevatedTarget,
			CaptureFailure::InaccessibleTarget,
			CaptureFailure::ModifierHeld,
			CaptureFailure::ForegroundChanged,
			CaptureFailure::ClipboardRestoreFailed,
			CaptureFailure::NoSelection,
			CaptureFailure::NonTextSelection,
			CaptureFailure::Unsupported,
			CaptureFailure::ClipboardBusy,
			CaptureFailure::TooLarge { chars: 123_456 },
			CaptureFailure::HookLost { recovered: true },
			CaptureFailure::HookLost { recovered: false },
		];
		all.extend(STORE_ERROR_KINDS.map(|kind| CaptureFailure::NotSaved { kind }));
		all
	}

	#[test]
	fn every_variant_is_sampled() {
		// `cause()` has no wildcard arm, so a new variant fails to compile there
		// first. This catches the other half: a variant that compiles but was
		// never added to the sample list above.
		let sampled: std::collections::HashSet<_> =
			every_failure().iter().map(CaptureFailure::cause).collect();
		assert_eq!(
			sampled.len(),
			13,
			"every_failure() must cover all thirteen variants, found {sampled:?}"
		);
	}

	#[test]
	fn every_message_is_non_empty_and_belongs_to_one_variant() {
		// The expected number of distinct messages is derived, not written down:
		// one per variant, plus three extra because NotSaved renders four, plus one
		// more because HookLost renders two.
		let samples = every_failure();
		let variants: std::collections::HashSet<_> =
			samples.iter().map(CaptureFailure::cause).collect();
		let expected_distinct = variants.len() + 3 + 1;

		let mut owners: HashMap<String, &'static str> = HashMap::new();
		for failure in &samples {
			let message = failure.message();
			assert!(
				!message.is_empty(),
				"{} rendered an empty message",
				failure.cause()
			);
			if let Some(other) = owners.insert(message.clone(), failure.cause()) {
				assert_eq!(
					other,
					failure.cause(),
					"two different variants render the same message: {message:?}"
				);
			}
		}

		assert_eq!(
			owners.len(),
			expected_distinct,
			"expected one message per variant plus the extra branches NotSaved and HookLost render"
		);
	}

	#[test]
	fn not_saved_branches_on_kind() {
		let branches = ["conflict", "parse", "unavailable"]
			.map(|kind| CaptureFailure::NotSaved { kind }.message());
		let generic = CaptureFailure::NotSaved { kind: "io" }.message();

		let distinct: std::collections::HashSet<&String> = branches.iter().collect();
		assert_eq!(distinct.len(), branches.len(), "two kinds share a message");
		assert!(branches.iter().all(|message| *message != generic));

		// The two remaining kinds collapse onto the generic branch.
		for kind in ["not-found", "invalid"] {
			assert_eq!(CaptureFailure::NotSaved { kind }.message(), generic);
		}
	}

	#[test]
	fn too_large_names_the_size() {
		assert!(CaptureFailure::TooLarge { chars: 250_000 }
			.message()
			.contains("250000"));
	}

	/// Every reachable `Evidence` combination resolves to exactly one cause, and
	/// `Unsupported` only in the absence of other evidence.
	#[test]
	fn the_precedence_rule_is_total() {
		let integrities = [
			TargetIntegrity::Higher,
			TargetIntegrity::NotHigher,
			TargetIntegrity::Unknown,
		];
		let mut seen: std::collections::HashSet<&'static str> = std::collections::HashSet::new();

		// Six booleans participate in the rule; the other flags are diagnostic.
		for bits in 0u32..64 {
			for integrity in integrities {
				for uiaccess in [false, true] {
					let evidence = Evidence {
						foreground_changed: bits & 1 != 0,
						modifier_held: bits & 2 != 0,
						caret_without_selection: bits & 4 != 0,
						empty_after_normalisation: bits & 8 != 0,
						clipboard_changed_but_untextual: bits & 16 != 0,
						clipboard_never_opened: bits & 32 != 0,
						integrity,
						uiaccess,
						..Evidence::default()
					};

					let cause = resolve(&evidence);
					// Deterministic: the same evidence always resolves the same way.
					assert_eq!(cause, resolve(&evidence));
					seen.insert(cause.cause());

					if cause == CaptureFailure::Unsupported {
						assert_eq!(
							evidence,
							Evidence {
								integrity,
								uiaccess,
								..Evidence::default()
							},
							"Unsupported must be reachable only with no other evidence"
						);
						assert!(integrity != TargetIntegrity::Unknown);
						assert!(integrity != TargetIntegrity::Higher || uiaccess);
					}
				}
			}
		}

		// Every cause the rule can produce is actually produced by some
		// combination, so no arm is dead.
		for expected in [
			"foreground-changed",
			"modifier-held",
			"elevated-target",
			"inaccessible-target",
			"no-selection",
			"non-text-selection",
			"clipboard-busy",
			"unsupported",
		] {
			assert!(seen.contains(expected), "{expected} is unreachable");
		}
	}

	#[test]
	fn a_higher_target_reachable_through_uiaccess_is_not_reported_as_elevated() {
		let evidence = Evidence {
			integrity: TargetIntegrity::Higher,
			uiaccess: true,
			no_text_pattern: true,
			..Evidence::default()
		};
		assert_eq!(resolve(&evidence), CaptureFailure::Unsupported);
	}

	#[test]
	fn an_unreadable_token_never_reports_the_administrator_wording() {
		let evidence = Evidence {
			integrity: TargetIntegrity::Unknown,
			..Evidence::default()
		};
		assert_eq!(resolve(&evidence), CaptureFailure::InaccessibleTarget);
	}

	#[test]
	fn positive_evidence_outranks_an_unreadable_target() {
		// The amended rule 4. Discord and audiodg.exe refuse the token read while
		// running at ordinary integrity, so an unreadable probe is common and says
		// nothing about why the capture found nothing. Telling the user "Copper
		// couldn't reach that window." when the cascade plainly saw a caret with no
		// selection would be reporting the probe instead of the observation.
		let caret = Evidence {
			caret_without_selection: true,
			integrity: TargetIntegrity::Unknown,
			..Evidence::default()
		};
		assert_eq!(resolve(&caret), CaptureFailure::NoSelection);

		let untextual = Evidence {
			clipboard_changed_but_untextual: true,
			integrity: TargetIntegrity::Unknown,
			..Evidence::default()
		};
		assert_eq!(resolve(&untextual), CaptureFailure::NonTextSelection);

		let busy = Evidence {
			clipboard_never_opened: true,
			integrity: TargetIntegrity::Unknown,
			..Evidence::default()
		};
		assert_eq!(resolve(&busy), CaptureFailure::ClipboardBusy);
	}

	#[test]
	fn an_unreadable_target_still_beats_the_terminal_fallback() {
		// It is what Copper says when it observed nothing *and* could not find out
		// why, which is more informative than "this app didn't give Copper
		// anything".
		let nothing_but_a_failed_probe = Evidence {
			no_text_pattern: true,
			clipboard_unchanged: true,
			integrity: TargetIntegrity::Unknown,
			..Evidence::default()
		};
		assert_eq!(
			resolve(&nothing_but_a_failed_probe),
			CaptureFailure::InaccessibleTarget
		);
	}

	#[test]
	fn a_whitespace_only_selection_is_no_selection_not_unsupported() {
		let evidence = Evidence {
			empty_after_normalisation: true,
			no_text_pattern: true,
			integrity: TargetIntegrity::NotHigher,
			..Evidence::default()
		};
		assert_eq!(resolve(&evidence), CaptureFailure::NoSelection);
	}

	#[test]
	fn the_cascade_order_is_uia_then_clipboard() {
		// Guards against an accidental reorder during a refactor: the order is a
		// design decision, not an implementation detail.
		assert_eq!(
			CAPTURE_CASCADE,
			[
				CaptureStrategy::UiAutomation,
				CaptureStrategy::ClipboardFallback
			]
		);
	}

	#[test]
	fn normalise_unifies_line_endings_and_trims() {
		assert_eq!(normalise("a\r\nb"), "a\nb");
		assert_eq!(normalise("a\rb"), "a\nb");
		assert_eq!(normalise("  padded \n"), "padded");
		assert_eq!(normalise("\r\n\t  \r\n"), "");
		assert_eq!(normalise("kept\ninside"), "kept\ninside");
		// Carriage returns at both ends: trimming and unifying commute, so the
		// order the implementation picks cannot change the answer.
		assert_eq!(normalise("\r\n  a\r\nb  \r\n"), "a\nb");
	}

	#[test]
	fn normalise_leaves_text_with_no_carriage_returns_alone() {
		assert_eq!(normalise("plain text"), "plain text");
	}
}
