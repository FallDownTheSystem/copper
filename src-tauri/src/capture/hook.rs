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
//!
//! Being fast turned out not to be sufficient. The timeout is wall-clock, so a
//! thread that is not *scheduled* inside it misses it however little work it has
//! to do — hence the time-critical priority [`install`] asks for, and the
//! liveness probe `watchdog` sends, which is the only way the application can
//! find out that a hook it still holds a handle for is gone.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
	GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VSC_TO_VK_EX};
use windows::Win32::UI::WindowsAndMessaging::{
	CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
	SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG,
	PM_NOREMOVE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER,
};

use crate::diagnostics;
use crate::win32::keys::{
	VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
	VK_SHIFT,
};
use crate::win32::{EXTRA_INFO_SIGNATURE, PROBE_SIGNATURE};

use super::{GAP_MAX_MS, TAP_MAX_MS};

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

	/// Drops any half-finished sequence.
	///
	/// Called when the watched family changes, so a tap recorded under the old
	/// modifier cannot pair with a tap under the new one.
	pub fn reset(&mut self) {
		self.state = State::Idle;
	}
}

// --- the watched family, swappable at runtime --------------------------------

/// Which two-sided modifier the recogniser is watching — or nothing at all, when
/// task-008 has bound capture to a conventional chord instead and the chord is
/// serviced by `tauri-plugin-global-shortcut`.
///
/// `#[repr(u8)]` because the live value is a module-level atomic: the hook
/// procedure reads it on every key event, and task-005's R2 rules out anything
/// that can block on that path — a `Mutex` would put a lock acquisition on the
/// hot path of a callback Windows silently uninstalls when it runs slowly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ModifierFamily {
	Off = 0,
	Shift = 1,
	Control = 2,
	Alt = 3,
}

impl ModifierFamily {
	fn from_code(code: u8) -> Self {
		match code {
			1 => Self::Shift,
			2 => Self::Control,
			3 => Self::Alt,
			_ => Self::Off,
		}
	}

	/// The virtual-key family this selector stands for, or `None` when the hook
	/// has no double-tap binding to recognise.
	pub fn trigger(self) -> Option<TriggerKey> {
		match self {
			Self::Off => None,
			Self::Shift => Some(TriggerKey::SHIFT),
			Self::Control => Some(TriggerKey::CONTROL),
			Self::Alt => Some(TriggerKey::ALT),
		}
	}
}

static WATCHED: AtomicU8 = AtomicU8::new(ModifierFamily::Shift as u8);

/// Points the recogniser at a different modifier without tearing the hook down.
///
/// Reinstalling `WH_KEYBOARD_LL` to change one selector would be the wrong shape
/// entirely — the hook is installed on its own thread with a published thread id
/// and a message pump, and swapping it means a window with no hook at all.
pub fn watch(family: ModifierFamily) {
	WATCHED.store(family as u8, Ordering::Relaxed);
}

/// `Relaxed` is correct rather than merely cheap: the value is a single
/// independent selector that publishes no other memory, so there is nothing for
/// a stronger ordering to synchronise. The worst case is one gesture judged
/// against the previous modifier.
pub fn watched() -> ModifierFamily {
	ModifierFamily::from_code(WATCHED.load(Ordering::Relaxed))
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
	pub const CONTROL: TriggerKey = TriggerKey {
		generic: VK_CONTROL,
		left: Some(VK_LCONTROL),
		right: Some(VK_RCONTROL),
	};
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

// --- liveness ----------------------------------------------------------------

/// Whether a hook is installed right now.
///
/// The startup install is only half the question. Windows removes a
/// `WH_KEYBOARD_LL` hook whose callback keeps missing `LowLevelHooksTimeout` and
/// tells the application nothing at all, so "did it install?" answered at startup
/// stays `true` forever while capture has silently stopped working. This flag is
/// the whole answer: [`install`] sets it, [`HookHandle::stop`] clears it, and the
/// watchdog drives both when its liveness probe stops coming back.
static HOOK_LIVE: AtomicBool = AtomicBool::new(false);

/// When the callback last saw a liveness probe, in milliseconds since [`EPOCH`].
///
/// Milliseconds in an atomic rather than the `Instant` this is derived from,
/// because an `Instant` does not fit in one and the callback may not take a lock.
/// Zero means no probe has ever arrived.
static PROBE_SEEN_MS: AtomicU64 = AtomicU64::new(0);

/// The zero the probe stamp is measured from. Any fixed point in the process's
/// life does; this one is simply the first time anything asks.
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Whether the hook is installed and has not been torn down.
pub fn alive() -> bool {
	HOOK_LIVE.load(Ordering::Relaxed)
}

/// The current probe stamp, for the watchdog to compare against the one it read
/// before injecting.
///
/// Comparing two readings rather than measuring an age is what makes the
/// representation's wrap irrelevant, and it is also the only question worth
/// asking: a stamp that moved proves the callback ran.
pub fn probe_stamp() -> u64 {
	PROBE_SEEN_MS.load(Ordering::Relaxed)
}

/// `Relaxed` for the same reason [`watched`] is: one independent value that
/// publishes no other memory, read on the far side of a two-second grace window.
fn note_probe() {
	PROBE_SEEN_MS.store(EPOCH.elapsed().as_millis() as u64, Ordering::Relaxed);
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
	/// The family the machine's current partial sequence was recorded under.
	/// Compared against [`watched`] on every event, so a rebind that lands
	/// mid-gesture resets rather than pairing a tap of the old modifier with a tap
	/// of the new one.
	family: ModifierFamily,
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

		// The watchdog's liveness probe, and **the one event this callback
		// deliberately swallows** — see the note at the bottom of this function for
		// the rule it breaks. A probe carries no meaning for anybody else: passing
		// it on would type a stray F24 into whatever has focus every fifteen
		// seconds, and this is the only place in the system where Copper can stop
		// it. Reaching this branch is itself the proof the watchdog is after, since
		// a hook Windows has removed never gets here at all.
		//
		// Second, not first, because a capture's own `Ctrl+C` is on the latency path
		// of a gesture the user is waiting on and a probe arrives once per fifteen
		// seconds. Both are one compare against a field already loaded.
		if event.dwExtraInfo == PROBE_SIGNATURE {
			note_probe();
			return LRESULT(1);
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

				// One relaxed load per event. The compare is what generalises
				// task-005's Shift-specific machine: a family change resets the
				// machine, and `Off` — capture bound to a conventional chord instead
				// — leaves it idle rather than recognising anything.
				let family = watched();
				if family != state.family {
					state.machine.reset();
					state.family = family;
				}
				let Some(trigger) = family.trigger() else {
					return;
				};

				let classified =
					trigger.classify(vk, || match MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX) {
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

		// Pass the event on. Returning non-zero would swallow Shift from the target
		// application, so the liveness probe above — which no application other
		// than Copper has any use for — is the only event that ever takes the other
		// path.
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
		// Before the early return below, and unconditionally: whatever this handle
		// once owned, the caller is asking for it to be gone, and a liveness flag
		// that outlived the request would keep `shortcuts` from standing its
		// insurance chord up.
		HOOK_LIVE.store(false, Ordering::Relaxed);
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
///
/// The watched modifier is **not** a parameter: it is the module-level atomic
/// [`watch`] sets, because task-008 rebinds it while the hook is running and
/// tearing the hook down to change one selector would leave a window with no
/// hook at all.
pub fn install(
	tx: Sender<Trigger>,
	in_flight: Arc<AtomicBool>,
	armed: Arc<AtomicBool>,
) -> Result<HookHandle, HookError> {
	let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();

	// Resolved here rather than on first use, so the callback's probe branch is a
	// load and a store and never a one-time initialisation.
	LazyLock::force(&EPOCH);

	let join = thread::Builder::new()
		.name("copper-hook".to_owned())
		.spawn(move || {
			let mut message = MSG::default();

			// The whole incident this guards against is a scheduling one, not a slow
			// one: the callback measures 7.8 microseconds against a 300 ms default
			// budget, and still loses keystrokes machine-wide when this thread is not
			// scheduled inside that window — under a debugger's suspension, or heavy
			// CPU contention. Windows then removes the hook and says nothing.
			//
			// Time-critical cannot starve the app in return. This thread spends
			// essentially all of its life blocked in `GetMessageW`, and the callback
			// yields the instant it has classified one key event.
			//
			// Best-effort. A refusal costs the priority, not the hook, so there is
			// nothing to abort for and the process still has a working callback.
			// SAFETY: no preconditions; the pseudo-handle needs no close.
			unsafe {
				if let Err(err) = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) {
					diagnostics::log_error(&format!(
						"[copper] capture: the hook thread could not be raised to time-critical \
						 ({err}); it runs at normal priority and is likelier to miss \
						 LowLevelHooksTimeout under load"
					));
				}
			}

			// A thread has no message queue until it calls a message function.
			// SAFETY: `message` is a live local for the call.
			unsafe {
				let _ = PeekMessageW(&mut message, None, WM_USER, WM_USER, PM_NOREMOVE);
			}

			HOOK_STATE.with(|cell| {
				*cell.borrow_mut() = Some(HookState {
					machine: DoubleTap::new(DoubleTapConfig::default()),
					family: watched(),
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
		Ok(Ok(thread_id)) => {
			HOOK_LIVE.store(true, Ordering::Relaxed);
			Ok(HookHandle {
				thread_id,
				join: Some(join),
			})
		}
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

	use windows::Win32::UI::WindowsAndMessaging::HC_ACTION;

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
	fn the_shipped_default_family_is_shift() {
		// The atomic's initial value is what the hook recognises between install
		// and the persisted binding being loaded, so it has to be the shipped
		// default rather than whatever a test left behind.
		assert_eq!(ModifierFamily::from_code(ModifierFamily::Shift as u8), ModifierFamily::Shift);
		assert_eq!(ModifierFamily::Shift.trigger(), Some(TriggerKey::SHIFT));
	}

	#[test]
	fn every_family_maps_to_its_own_trigger_and_off_maps_to_none() {
		assert_eq!(ModifierFamily::Control.trigger(), Some(TriggerKey::CONTROL));
		assert_eq!(ModifierFamily::Alt.trigger(), Some(TriggerKey::ALT));
		// The conventional-chord case: nothing for the hook to recognise, so the
		// callback returns before it ever reaches the machine.
		assert_eq!(ModifierFamily::Off.trigger(), None);
		// An out-of-range byte can only come from a bug, and reading it as "no
		// binding" is the safe answer: capture stops rather than firing on a
		// modifier nobody chose.
		assert_eq!(ModifierFamily::from_code(9), ModifierFamily::Off);
	}

	// --- the callback itself ---------------------------------------------------
	// `HOOK_STATE` is thread-local, so a test can install its own and drive the
	// real `keyboard_proc` over a synthetic event. Everything below runs on one
	// test thread and shares that state, which is why it is one test rather than
	// several: two of them could otherwise interleave on the same thread-local.

	/// Installs callback state the test can inspect, and hands back the trigger
	/// channel.
	fn install_test_state() -> mpsc::Receiver<Trigger> {
		let (tx, rx) = mpsc::channel();
		HOOK_STATE.with(|cell| {
			*cell.borrow_mut() = Some(HookState {
				machine: DoubleTap::new(DoubleTapConfig::default()),
				family: ModifierFamily::Shift,
				tx,
				in_flight: Arc::new(AtomicBool::new(false)),
				armed: Arc::new(AtomicBool::new(true)),
			});
		});
		rx
	}

	/// Runs the real callback over one synthetic event.
	fn feed(extra: usize, vk: u32, message: u32) -> LRESULT {
		let mut event = KBDLLHOOKSTRUCT {
			vkCode: vk,
			dwExtraInfo: extra,
			..Default::default()
		};
		// SAFETY: `event` outlives the call and `HC_ACTION` is the documented
		// "process this event" code, which is what the pointer contract depends on.
		unsafe {
			keyboard_proc(
				HC_ACTION as i32,
				WPARAM(message as usize),
				LPARAM(&mut event as *mut KBDLLHOOKSTRUCT as isize),
			)
		}
	}

	fn machine_is_idle() -> bool {
		HOOK_STATE.with(|cell| {
			cell.borrow()
				.as_ref()
				.is_some_and(|state| state.machine.state == State::Idle)
		})
	}

	#[test]
	fn the_two_tags_are_handled_differently_and_neither_reaches_the_machine() {
		let triggers = install_test_state();
		// The two tags have to be distinguishable in the first place, or one branch
		// shadows the other and the difference below proves nothing.
		assert_ne!(EXTRA_INFO_SIGNATURE, PROBE_SIGNATURE);

		// A value no elapsed-millisecond count can be, so "the stamp moved" cannot
		// be satisfied by the initial zero or by a probe that happened to land in
		// the same millisecond as this baseline.
		PROBE_SEEN_MS.store(u64::MAX, Ordering::Relaxed);

		// The probe carries a vkCode that would otherwise open a double-tap, so a
		// branch that fell through would leave the machine mid-sequence.
		let swallowed = feed(PROBE_SIGNATURE, VK_LSHIFT, WM_KEYDOWN);
		assert_ne!(
			swallowed.0, 0,
			"a probe must be swallowed, or every fifteen seconds an F24 lands in whatever has focus"
		);
		assert_ne!(
			probe_stamp(),
			u64::MAX,
			"a probe must record that the callback ran; that recording is the whole watchdog"
		);
		assert!(machine_is_idle(), "a probe must not reach the tap machine");

		// Copper's own injected Ctrl+C: filtered from the machine just the same, but
		// passed on, because the foreground application is who it is for.
		let passed = feed(EXTRA_INFO_SIGNATURE, VK_LSHIFT, WM_KEYDOWN);
		assert_eq!(passed.0, 0, "Copper's own tag must still reach the next hook");
		assert!(machine_is_idle());

		// And an ordinary keystroke is unaffected by either branch: it reaches the
		// machine, which is what the tags exist to keep them out of.
		let real = feed(0, VK_LSHIFT, WM_KEYDOWN);
		assert_eq!(real.0, 0);
		assert!(!machine_is_idle(), "a real key-down must open a sequence");

		assert!(
			triggers.try_recv().is_err(),
			"none of these events completes a double-tap"
		);
		HOOK_STATE.with(|cell| *cell.borrow_mut() = None);
	}

	/// The other half of the family swap: a partial sequence must not survive it.
	#[test]
	fn a_reset_drops_a_half_finished_double_tap() {
		let mut machine = DoubleTap::new(DoubleTapConfig::default());
		assert!(!machine.on_key(L, DOWN, 0));
		assert!(!machine.on_key(L, UP, 40));
		machine.reset();
		// Without the reset this second tap would complete the pair and fire.
		assert!(!machine.on_key(L, DOWN, 100));
		assert!(!machine.on_key(L, UP, 140));
	}
}
