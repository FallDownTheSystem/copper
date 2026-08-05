//! The `WH_KEYBOARD_LL` hook, its dedicated thread, and the double-tap
//! recogniser.
//!
//! The callback is the hottest and most dangerous code in the app. Windows
//! silently removes a low-level hook whose callback exceeds
//! `HKEY_CURRENT_USER\Control Panel\Desktop\LowLevelHooksTimeout` — clamped to a
//! 1000 ms maximum on Windows 10 1709+ — and gives the application **no way to
//! detect that it happened**. Microsoft's own guidance is to run hooks on a
//! dedicated thread that hands work off and returns immediately. So the callback
//! classifies the event, feeds a small state machine, and on a trigger does one
//! non-blocking channel send. Nothing else: no logging, no allocation, no Win32
//! call beyond `CallNextHookEx`.
//!
//! Task-001 measured this callback at **7.8 microseconds** worst case against
//! that 1000 ms budget, over 500 injected double-taps that produced exactly 500
//! triggers.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VSC_TO_VK_EX};
use windows::Win32::UI::WindowsAndMessaging::{
	CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
	SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG,
	PM_NOREMOVE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER,
};

use crate::diagnostics;
use crate::win32::EXTRA_INFO_SIGNATURE;

use super::{GAP_MAX_MS, TAP_MAX_MS};

// Virtual-key codes, as the low-level hook reports them in `vkCode`.
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;

// --- the pure state machine --------------------------------------------------
// No Win32 below this line until the callback, so all of it is unit-testable.

/// Which side of a two-sided modifier produced an event.
///
/// `Either` covers two cases that both behave as "matches whatever the other tap
/// was": a trigger key with no sides (any ordinary key), and a generic
/// `VK_SHIFT` whose side would not resolve from its scan code. The second is
/// real — remappers can deliver a generic `VK_SHIFT`, and Copper deliberately
/// accepts remapped input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySide {
	Left,
	Right,
	Either,
}

impl KeySide {
	fn matches(self, other: KeySide) -> bool {
		self == other || self == KeySide::Either || other == KeySide::Either
	}

	/// Prefers a concrete side over `Either`, so a sequence that starts
	/// unresolved but later resolves records the real side.
	fn refine(self, other: KeySide) -> KeySide {
		if self == KeySide::Either {
			other
		} else {
			self
		}
	}
}

/// A key event as the machine sees it: the trigger key on some side, or anything
/// else at all.
///
/// "Anything else" rather than "not Shift" on purpose (task-005 R22a): the reset
/// rule is written against the trigger family so Phase 7 can rebind the trigger
/// without touching the transition logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
	Trigger(KeySide),
	Other,
}

/// The two timing bounds.
///
/// Separate deliberately. A single start-to-finish window would conflate holding
/// with tapping: a deliberate but slow second press would fail while a slow
/// hold-and-release could pass. Task-001 recorded that conflation as a bug it had
/// already fixed once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoubleTapConfig {
	/// Maximum duration of one press, key-down to its own key-up.
	pub tap_max_ms: u32,
	/// Maximum gap between the first key-up and the second key-down.
	pub gap_max_ms: u32,
}

impl Default for DoubleTapConfig {
	fn default() -> Self {
		Self {
			tap_max_ms: TAP_MAX_MS,
			gap_max_ms: GAP_MAX_MS,
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
	Idle,
	FirstDown { side: KeySide, down: u32 },
	FirstUp { side: KeySide, up: u32 },
	SecondDown { side: KeySide, down: u32 },
}

/// The double-tap recogniser.
///
/// Every elapsed-time calculation uses `wrapping_sub`. `KBDLLHOOKSTRUCT.time` is
/// a `DWORD` tick count that wraps roughly every 49.7 days of uptime; a plain
/// subtraction underflows across the wrap and, in a debug build, panics *inside
/// the hook callback* — which crosses an `extern "system"` boundary and aborts
/// the process.
#[derive(Debug, Clone)]
pub struct DoubleTap {
	cfg: DoubleTapConfig,
	state: State,
}

impl DoubleTap {
	pub fn new(cfg: DoubleTapConfig) -> Self {
		Self {
			cfg,
			state: State::Idle,
		}
	}

	/// Feeds one key event. Returns `true` on the key-**up** that completes a
	/// double-tap, and only then. Firing on the release is also what makes the
	/// modifier wait meaningful: by then nothing is held, by construction.
	pub fn on_key(&mut self, observed: Observed, is_up: bool, time_ms: u32) -> bool {
		let side = match observed {
			// Any key outside the trigger family, down or up, breaks the sequence.
			// This is what stops `Shift+A` from ever counting as a tap, and it
			// subsumes a separate dirty-flag guard: a foreign key pressed while the
			// trigger is held cancels the sequence outright.
			Observed::Other => {
				self.state = State::Idle;
				return false;
			}
			Observed::Trigger(side) => side,
		};

		if is_up {
			self.on_up(side, time_ms)
		} else {
			self.on_down(side, time_ms);
			false
		}
	}

	fn on_down(&mut self, side: KeySide, now: u32) {
		self.state = match self.state {
			State::Idle => State::FirstDown { side, down: now },

			// Auto-repeat. Windows delivers repeated key-downs while a key is
			// physically held. The key is already recorded as down, so this is not
			// a new press: keep the original timestamp, or a long hold would keep
			// resetting its own clock and could satisfy `tap_max_ms` on release.
			State::FirstDown { side: held, down } if held.matches(side) => State::FirstDown {
				side: held.refine(side),
				down,
			},
			State::SecondDown { side: held, down } if held.matches(side) => State::SecondDown {
				side: held.refine(side),
				down,
			},

			State::FirstUp { side: held, up } if held.matches(side) => {
				if now.wrapping_sub(up) <= self.cfg.gap_max_ms {
					State::SecondDown {
						side: held.refine(side),
						down: now,
					}
				} else {
					// Too slow to be the second half of a double-tap, but a perfectly
					// good *first* tap of the next one. Starting over rather than
					// idling means a slow tap does not poison the deliberate
					// double-tap that follows it.
					State::FirstDown { side, down: now }
				}
			}

			// A different side, at any point. Both taps must be the same physical
			// key, so the sequence is broken — but this press legitimately starts a
			// new one. Left-then-right therefore yields no trigger.
			_ => State::FirstDown { side, down: now },
		};
	}

	fn on_up(&mut self, side: KeySide, now: u32) -> bool {
		match self.state {
			State::FirstDown { side: held, down } if held.matches(side) => {
				self.state = if self.was_a_tap(down, now) {
					State::FirstUp {
						side: held.refine(side),
						up: now,
					}
				} else {
					State::Idle
				};
				false
			}
			State::SecondDown { side: held, down } if held.matches(side) => {
				let fired = self.was_a_tap(down, now);
				self.state = State::Idle;
				fired
			}
			// An up for the other side, or an up with no matching down because the
			// hook started listening mid-press. Neither can complete a tap.
			_ => {
				self.state = State::Idle;
				false
			}
		}
	}

	fn was_a_tap(&self, down: u32, up: u32) -> bool {
		up.wrapping_sub(down) <= self.cfg.tap_max_ms
	}
}

/// The trigger key expressed as the low-level hook actually reports it.
///
/// The hook reports `VK_LSHIFT` / `VK_RSHIFT` in practice rather than the generic
/// `VK_SHIFT`, but that is not documented as guaranteed and remappers can deliver
/// the generic code, so all three are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerKey {
	pub generic: u32,
	pub left: Option<u32>,
	pub right: Option<u32>,
}

/// The result of matching a raw `vkCode` against the configured trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classified {
	Trigger { side: KeySide },
	Other,
}

impl TriggerKey {
	pub const SHIFT: TriggerKey = TriggerKey {
		generic: VK_SHIFT,
		left: Some(VK_LSHIFT),
		right: Some(VK_RSHIFT),
	};
	/// Unused in Phase 4. Present because the trigger-family test table is run
	/// against a second family to prove the machine is not Shift-specific, which
	/// is the seam task-008 inherits.
	#[cfg_attr(not(test), allow(dead_code))]
	pub const CONTROL: TriggerKey = TriggerKey {
		generic: VK_CONTROL,
		left: Some(VK_LCONTROL),
		right: Some(VK_RCONTROL),
	};
	#[cfg_attr(not(test), allow(dead_code))]
	pub const ALT: TriggerKey = TriggerKey {
		generic: VK_MENU,
		left: Some(VK_LMENU),
		right: Some(VK_RMENU),
	};

	/// Matches a `vkCode` against this trigger.
	///
	/// `resolve_generic` is the `MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX)` step,
	/// injected as a closure so the classifier stays testable without Win32. It is
	/// called only for a generic two-sided modifier code.
	pub fn classify(&self, vk: u32, resolve_generic: impl FnOnce() -> Option<u32>) -> Classified {
		if self.left == Some(vk) {
			return Classified::Trigger {
				side: KeySide::Left,
			};
		}
		if self.right == Some(vk) {
			return Classified::Trigger {
				side: KeySide::Right,
			};
		}
		if vk != self.generic {
			return Classified::Other;
		}
		if self.left.is_none() {
			// A sideless trigger: the generic code *is* the key.
			return Classified::Trigger {
				side: KeySide::Either,
			};
		}
		match resolve_generic() {
			Some(resolved) if self.left == Some(resolved) => Classified::Trigger {
				side: KeySide::Left,
			},
			Some(resolved) if self.right == Some(resolved) => Classified::Trigger {
				side: KeySide::Right,
			},
			// Unresolvable. Match either side rather than dropping the event:
			// dropping would silently break the trigger for remapper users.
			_ => Classified::Trigger {
				side: KeySide::Either,
			},
		}
	}
}

// --- the hook thread ---------------------------------------------------------

/// Sent on the key-up that completes a double-tap.
pub struct Trigger {
	/// Read only by the debug-build attempt log, which is the only thing that
	/// makes "the hook fired and the capture found nothing" distinguishable from
	/// "the hook never fired".
	#[cfg_attr(not(debug_assertions), allow(dead_code))]
	pub at: Instant,
}

struct HookState {
	machine: DoubleTap,
	trigger: TriggerKey,
	tx: Sender<Trigger>,
	/// One capture in flight at a time. A bounded channel does not express this:
	/// once the worker receives, the slot is free again and a second trigger
	/// arriving mid-capture would produce a second note.
	in_flight: Arc<AtomicBool>,
	/// Triggers are dropped until every startup gate has cleared.
	armed: Arc<AtomicBool>,
}

thread_local! {
	static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

unsafe extern "system" fn keyboard_proc(ncode: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
	// SAFETY (whole function): for ncode >= 0 the OS documents lparam as a
	// pointer to a KBDLLHOOKSTRUCT valid for the duration of this call, and
	// CallNextHookEx is the required return on every path.
	unsafe {
		if ncode < 0 {
			return CallNextHookEx(None, ncode, wparam, lparam);
		}

		let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
		let vk = event.vkCode;
		let scan = event.scanCode;

		// Copper's own synthesized Ctrl+C must not feed back into the machine.
		// This filters **only** Copper's tag, never `LLKHF_INJECTED` generally:
		// PowerToys Keyboard Manager and AutoHotkey deliver genuine user intent as
		// injected input, and rejecting all of it would silently break the trigger
		// for those users.
		if event.dwExtraInfo == EXTRA_INFO_SIGNATURE {
			return CallNextHookEx(None, ncode, wparam, lparam);
		}

		let message = wparam.0 as u32;
		let is_up = message == WM_KEYUP || message == WM_SYSKEYUP;
		let is_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;

		if is_up || is_down {
			HOOK_STATE.with(|cell| {
				// `try_borrow_mut`, not `borrow_mut`. A panic here would cross an
				// `extern "system"` boundary and abort the process — the worst
				// possible outcome for a background tool, presenting as a mysterious
				// crash rather than a failed capture. Re-entrancy is not currently
				// possible; this keeps that safe if someone later adds a call that
				// pumps messages.
				let Ok(mut borrow) = cell.try_borrow_mut() else {
					return;
				};
				let Some(state) = borrow.as_mut() else {
					return;
				};

				let classified = state
					.trigger
					.classify(vk, || match MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX) {
						0 => None,
						resolved => Some(resolved),
					});
				let observed = match classified {
					Classified::Trigger { side } => Observed::Trigger(side),
					Classified::Other => Observed::Other,
				};

				if state.machine.on_key(observed, is_up, event.time)
					&& state.armed.load(Ordering::SeqCst)
					&& state
						.in_flight
						.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
						.is_ok()
				{
					// An unbounded channel, so the send does not wait on a receiver
					// the way a bounded one would — parking the callback is what gets
					// the hook silently removed. It is not a hard real-time
					// guarantee: the send takes an uncontended lock and may allocate a
					// node. Task-001 measured the whole callback at 7.8 microseconds
					// worst case against a budget of up to 1000 ms, which is the
					// evidence this rests on rather than the absence of allocation.
					if state.tx.send(Trigger { at: Instant::now() }).is_err() {
						// The worker is gone; do not leave the gate latched shut.
						state.in_flight.store(false, Ordering::SeqCst);
					}
				}
			});
		}

		// Always pass the event on. Returning non-zero would swallow Shift from
		// the target application.
		CallNextHookEx(None, ncode, wparam, lparam)
	}
}

#[derive(Debug)]
pub enum HookError {
	Spawn(std::io::Error),
	Install(String),
	ThreadGone,
}

impl std::fmt::Display for HookError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			HookError::Spawn(err) => write!(f, "could not spawn the hook thread: {err}"),
			HookError::Install(err) => write!(f, "SetWindowsHookExW failed: {err}"),
			HookError::ThreadGone => {
				f.write_str("the hook thread exited before reporting readiness")
			}
		}
	}
}

impl std::error::Error for HookError {}

/// Owns the hook thread.
pub struct HookHandle {
	thread_id: u32,
	join: Option<JoinHandle<()>>,
}

impl HookHandle {
	/// Uninstalls the hook and joins its thread. Idempotent.
	///
	/// Returns whether the quit actually reached the thread. That matters to the
	/// caller and is not merely diagnostic: a detached hook thread keeps running,
	/// and with it the thread-local state holding the trigger `Sender`. The worker
	/// is waiting on that channel to close, so joining the worker after a failed
	/// post would block forever — trading a leaked thread for a hung exit.
	pub fn stop(&mut self) -> bool {
		let Some(join) = self.join.take() else {
			// Already stopped. Reporting success is right: whatever the first call
			// decided has already been acted on.
			return true;
		};
		// Check the post. A blind join after a failed post hangs shutdown forever;
		// detaching the thread instead is survivable, a deadlock at exit is not.
		// SAFETY: no preconditions; failure is reported through the Result.
		match unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) } {
			Ok(()) => {
				let _ = join.join();
				true
			}
			Err(err) => {
				diagnostics::log_error(&format!(
					"[copper] capture: could not post WM_QUIT to the hook thread ({err}); \
					 detaching it rather than joining"
				));
				false
			}
		}
	}
}

impl Drop for HookHandle {
	fn drop(&mut self) {
		let _ = self.stop();
	}
}

/// Installs the hook on its own thread and waits for it to report readiness.
///
/// The order inside the thread is task-001's and is easy to get wrong. It is, in
/// full: force the message queue into existence with `PeekMessageW`; install the
/// state the callback reads; install the hook; and only then publish the thread
/// id. A thread has no message queue until it calls a message function, so
/// publishing the id before `PeekMessageW` leaves a window in which
/// `PostThreadMessageW` fails with `ERROR_INVALID_THREAD_ID`, the `WM_QUIT` is
/// lost, and shutdown blocks forever. The state goes in before the hook because
/// the callback can fire the instant the hook is installed.
pub fn install(
	trigger: TriggerKey,
	tx: Sender<Trigger>,
	in_flight: Arc<AtomicBool>,
	armed: Arc<AtomicBool>,
) -> Result<HookHandle, HookError> {
	let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();

	let join = thread::Builder::new()
		.name("copper-hook".to_owned())
		.spawn(move || {
			let mut message = MSG::default();

			// A thread has no message queue until it calls a message function.
			// SAFETY: `message` is a live local for the call.
			unsafe {
				let _ = PeekMessageW(&mut message, None, WM_USER, WM_USER, PM_NOREMOVE);
			}

			HOOK_STATE.with(|cell| {
				*cell.borrow_mut() = Some(HookState {
					machine: DoubleTap::new(DoubleTapConfig::default()),
					trigger,
					tx,
					in_flight,
					armed,
				});
			});

			// The executable's module handle, not NULL. Task-001 found it is
			// genuinely required here and added the LibraryLoader feature for it.
			// SAFETY: `keyboard_proc` has the documented signature and outlives the
			// hook, which is uninstalled below on this same thread.
			let installed = unsafe {
				GetModuleHandleW(None).and_then(|module| {
					SetWindowsHookExW(
						WH_KEYBOARD_LL,
						Some(keyboard_proc),
						Some(HINSTANCE(module.0)),
						0,
					)
				})
			};

			let hook: HHOOK = match installed {
				Ok(hook) => hook,
				Err(err) => {
					HOOK_STATE.with(|cell| *cell.borrow_mut() = None);
					let _ = ready_tx.send(Err(err.to_string()));
					return;
				}
			};

			// SAFETY: no preconditions.
			let thread_id = unsafe { GetCurrentThreadId() };
			if ready_tx.send(Ok(thread_id)).is_err() {
				// Nobody is waiting; unwind rather than pump forever.
				// SAFETY: installed on this thread, uninstalled on this thread.
				unsafe {
					let _ = UnhookWindowsHookEx(hook);
				}
				HOOK_STATE.with(|cell| *cell.borrow_mut() = None);
				return;
			}

			loop {
				// GetMessageW: >0 a normal message, 0 for WM_QUIT, -1 on error.
				// Treating -1 as a message and looping until zero would spin forever.
				// SAFETY: `message` is a live local for the call.
				let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
				if result.0 <= 0 {
					break;
				}
				// SAFETY: `message` was filled by the GetMessageW above.
				unsafe {
					let _ = TranslateMessage(&message);
					DispatchMessageW(&message);
				}
			}

			// Uninstall on the installing thread, so install and uninstall stay on
			// one thread.
			// SAFETY: `hook` is live and is unhooked exactly once.
			unsafe {
				let _ = UnhookWindowsHookEx(hook);
			}
			HOOK_STATE.with(|cell| *cell.borrow_mut() = None);
		})
		.map_err(HookError::Spawn)?;

	match ready_rx.recv() {
		Ok(Ok(thread_id)) => Ok(HookHandle {
			thread_id,
			join: Some(join),
		}),
		Ok(Err(err)) => {
			let _ = join.join();
			Err(HookError::Install(err))
		}
		Err(_) => {
			let _ = join.join();
			Err(HookError::ThreadGone)
		}
	}
}

// --- tests -------------------------------------------------------------------
// The table is parameterised by trigger family and run against two of them, so
// task-008 can rebind the trigger and re-run this unchanged rather than writing
// a second suite against a machine whose behaviour is supposed to be identical.

#[cfg(test)]
mod tests {
	use super::*;

	const L: Observed = Observed::Trigger(KeySide::Left);
	const R: Observed = Observed::Trigger(KeySide::Right);
	const OTHER: Observed = Observed::Other;
	const DOWN: bool = false;
	const UP: bool = true;

	/// Every family the machine is expected to behave identically for.
	const FAMILIES: [TriggerKey; 3] = [TriggerKey::SHIFT, TriggerKey::CONTROL, TriggerKey::ALT];

	/// Feeds a script and returns how many times the machine fired.
	fn run(script: &[(Observed, bool, u32)]) -> usize {
		let mut machine = DoubleTap::new(DoubleTapConfig::default());
		script
			.iter()
			.filter(|(observed, is_up, time)| machine.on_key(*observed, *is_up, *time))
			.count()
	}

	/// A name, a script of `(event, is_up, tick)`, and how many times the machine
	/// must fire.
	type Row = (&'static str, &'static [(Observed, bool, u32)], usize);

	/// The behaviour table. Every row is family-independent by construction — the
	/// machine sees `Observed`, never a virtual-key code.
	#[test]
	fn the_state_machine_table() {
		let cases: &[Row] = &[
			(
				"a clean double-tap fires exactly once",
				&[(L, DOWN, 0), (L, UP, 40), (L, DOWN, 120), (L, UP, 160)],
				1,
			),
			("a single tap does not fire", &[(L, DOWN, 0), (L, UP, 40)], 0),
			(
				"a hold is not a tap, so its release cannot arm the first half",
				&[(L, DOWN, 0), (L, UP, 300), (L, DOWN, 340), (L, UP, 380)],
				0,
			),
			(
				"auto-repeat key-downs do not restart the press timer and rescue a hold",
				&[
					(L, DOWN, 0),
					(L, DOWN, 100),
					(L, DOWN, 200),
					(L, DOWN, 280),
					(L, UP, 300),
					(L, DOWN, 340),
					(L, UP, 380),
				],
				0,
			),
			(
				"auto-repeat during the second press also holds its clock",
				&[
					(L, DOWN, 0),
					(L, UP, 40),
					(L, DOWN, 100),
					(L, DOWN, 200),
					(L, DOWN, 300),
					(L, UP, 400),
				],
				0,
			),
			(
				"the two taps must be the same physical key",
				&[(L, DOWN, 0), (L, UP, 40), (R, DOWN, 100), (R, UP, 140)],
				0,
			),
			(
				"a key outside the family mid-press resets the sequence",
				&[
					(L, DOWN, 0),
					(OTHER, DOWN, 10),
					(OTHER, UP, 20),
					(L, UP, 30),
					(L, DOWN, 60),
					(L, UP, 90),
				],
				0,
			),
			(
				"a key outside the family between the taps resets the sequence",
				&[
					(L, DOWN, 0),
					(L, UP, 40),
					(OTHER, DOWN, 60),
					(OTHER, UP, 70),
					(L, DOWN, 100),
					(L, UP, 140),
				],
				0,
			),
			(
				"a press of exactly tap_max still counts as a tap",
				&[(L, DOWN, 0), (L, UP, 250), (L, DOWN, 300), (L, UP, 340)],
				1,
			),
			(
				"a press one millisecond past tap_max does not",
				&[(L, DOWN, 0), (L, UP, 251), (L, DOWN, 300), (L, UP, 340)],
				0,
			),
			(
				"the second press is bounded by tap_max too",
				&[(L, DOWN, 0), (L, UP, 40), (L, DOWN, 100), (L, UP, 351)],
				0,
			),
			(
				"a gap of exactly gap_max still pairs",
				&[(L, DOWN, 0), (L, UP, 40), (L, DOWN, 440), (L, UP, 470)],
				1,
			),
			(
				"a gap one millisecond past gap_max does not",
				&[(L, DOWN, 0), (L, UP, 40), (L, DOWN, 441), (L, UP, 470)],
				0,
			),
			(
				"a too-slow second tap becomes a new first tap rather than poisoning it",
				&[
					(L, DOWN, 0),
					(L, UP, 40),
					(L, DOWN, 2000),
					(L, UP, 2040),
					(L, DOWN, 2100),
					(L, UP, 2140),
				],
				1,
			),
			(
				"a stray key-up with no matching down does not arm",
				&[(L, UP, 10), (L, DOWN, 50), (L, UP, 90)],
				0,
			),
			(
				"three taps fire once, not twice",
				&[
					(L, DOWN, 0),
					(L, UP, 40),
					(L, DOWN, 100),
					(L, UP, 140),
					(L, DOWN, 200),
					(L, UP, 240),
				],
				1,
			),
			(
				"an unresolved generic modifier still pairs with a concrete side",
				&[
					(Observed::Trigger(KeySide::Either), DOWN, 0),
					(Observed::Trigger(KeySide::Either), UP, 40),
					(L, DOWN, 100),
					(L, UP, 140),
				],
				1,
			),
		];

		for (name, script, expected) in cases {
			assert_eq!(run(script), *expected, "{name}");
		}
	}

	#[test]
	fn tick_count_rollover_mid_sequence_still_fires() {
		// The DWORD tick count wraps roughly every 49.7 days of uptime. A plain
		// subtraction would underflow here and panic inside the hook callback in a
		// debug build.
		let base = u32::MAX - 50;
		assert_eq!(
			run(&[
				(L, DOWN, base),
				(L, UP, base.wrapping_add(40)),
				(L, DOWN, base.wrapping_add(120)),
				(L, UP, base.wrapping_add(160)),
			]),
			1
		);
	}

	#[test]
	fn tick_count_rollover_still_rejects_an_over_long_press() {
		let base = u32::MAX - 10;
		assert_eq!(
			run(&[
				(L, DOWN, base),
				(L, UP, base.wrapping_add(300)),
				(L, DOWN, base.wrapping_add(340)),
				(L, UP, base.wrapping_add(380)),
			]),
			0
		);
	}

	#[test]
	fn every_family_classifies_its_own_two_sides_directly() {
		for family in FAMILIES {
			assert_eq!(
				family.classify(family.left.unwrap(), || unreachable!(
					"a concrete side needs no resolution"
				)),
				Classified::Trigger {
					side: KeySide::Left
				}
			);
			assert_eq!(
				family.classify(family.right.unwrap(), || unreachable!(
					"a concrete side needs no resolution"
				)),
				Classified::Trigger {
					side: KeySide::Right
				}
			);
			// 'A' is outside every modifier family.
			assert_eq!(family.classify(0x41, || None), Classified::Other);
		}
	}

	#[test]
	fn every_family_resolves_its_generic_code_through_the_scan_code() {
		for family in FAMILIES {
			assert_eq!(
				family.classify(family.generic, || family.right),
				Classified::Trigger {
					side: KeySide::Right
				}
			);
			// Unresolvable: match either side rather than dropping the event.
			assert_eq!(
				family.classify(family.generic, || None),
				Classified::Trigger {
					side: KeySide::Either
				}
			);
		}
	}

	#[test]
	fn the_families_do_not_classify_each_other() {
		// Shift must not see a Ctrl key as its own, or the reset rule would never
		// fire for the very keys most likely to be held during a capture.
		assert_eq!(
			TriggerKey::SHIFT.classify(VK_LCONTROL, || None),
			Classified::Other
		);
		assert_eq!(
			TriggerKey::CONTROL.classify(VK_LSHIFT, || None),
			Classified::Other
		);
	}

	#[test]
	fn the_configured_trigger_is_shift() {
		assert_eq!(super::super::CAPTURE_TRIGGER, TriggerKey::SHIFT);
	}
}
