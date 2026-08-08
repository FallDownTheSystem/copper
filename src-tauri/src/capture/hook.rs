//! The `WH_KEYBOARD_LL` hook, its dedicated thread, and the two double-tap
//! recognisers.
//!
//! Two, not one: capture and summon may each be bound to a modifier double-tap,
//! and they are independent bindings that must be recognised independently. They
//! share the callback, the classification and the channel; they share no state,
//! because a tap of one family must not be able to advance the other's sequence.
//!
//! The callback is the hottest and most dangerous code in the app. Windows
//! silently removes a low-level hook whose callback exceeds
//! `HKEY_CURRENT_USER\Control Panel\Desktop\LowLevelHooksTimeout` — clamped to a
//! 1000 ms maximum on Windows 10 1709+ — and gives the application **no way to
//! detect that it happened**. Microsoft's own guidance is to run hooks on a
//! dedicated thread that hands work off and returns immediately. So the callback
//! classifies the event, feeds a small state machine, and on a trigger does one
//! non-blocking channel send.
//!
//! What it costs, stated honestly rather than as a slogan, because a claim of
//! "nothing at all" invites the next reader to add something: three relaxed
//! atomic loads on every event — one mute flag and one selector per recogniser;
//! one `MapVirtualKeyW` on the generic two-sided modifier codes only, which
//! remappers produce and ordinary keyboards do not, and which the two recognisers
//! resolve **once between them** rather than once each; and on the key-up that
//! completes a double-tap, an `Instant::now` and a send into an unbounded
//! channel, which takes an uncontended lock and may allocate one node.
//! The probe branch adds a second `usize` compare and, when it matches, an
//! `Instant::elapsed` and a relaxed store. There is no logging on any path, and
//! no blocking call on any path — the last is the invariant that actually
//! matters, and the one to check anything new against.
//!
//! Task-001 measured the pre-probe callback at **7.8 microseconds** worst case
//! against that 1000 ms budget, over 500 injected double-taps that produced
//! exactly 500 triggers. The probe branch is not covered by that measurement; it
//! is bounded by the same reasoning rather than by the same evidence.
//!
//! Being fast turned out not to be sufficient. The timeout is wall-clock, so a
//! thread that is not *scheduled* inside it misses it however little work it has
//! to do — hence the time-critical priority [`install`] asks for, and the
//! liveness probe `watchdog` sends, which is the only way the application can
//! find out that a hook it still holds a handle for is gone.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
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
	VK_CONTROL, VK_F24, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RCONTROL, VK_RMENU,
	VK_RSHIFT, VK_SHIFT,
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
	pub fn matches(self, other: KeySide) -> bool {
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

	/// The two halves of the packed selector — see [`WatchedTrigger`].
	fn code(self) -> u8 {
		match self {
			Self::Either => 0,
			Self::Left => 1,
			Self::Right => 2,
		}
	}

	/// An out-of-range code can only come from a bug, and `Either` is the safe
	/// reading: a binding that matches both sides is the behaviour every install
	/// before sided bindings existed already had.
	fn from_code(code: u8) -> Self {
		match code {
			1 => Self::Left,
			2 => Self::Right,
			_ => Self::Either,
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
///
/// The discriminants are load-bearing beyond that: they are the low two bits of
/// [`WatchedTrigger`]'s packed byte, so all four must stay inside `0..=3`. A
/// fifth family would need the layout widened rather than a variant added.
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

/// Which of the two recognisers a selector, a machine or a trigger belongs to.
///
/// The two are never interchangeable: one reads the foreground selection, the
/// other reveals a window. They are separate here for the same reason
/// `shortcuts::Role` keeps them separate over the plugin's chords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerRole {
	Capture,
	Summon,
}

/// A double-tap binding as the hook recognises it: which modifier family, and
/// which physical side of it.
///
/// `side: Either` is the unsided spelling — `Shift Shift` — and keeps the rule
/// the hook has always had, that both taps be the same physical key whichever one
/// that is. A concrete side is the sided spelling — `LShift LShift` — and matches
/// that side alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchedTrigger {
	pub family: ModifierFamily,
	pub side: KeySide,
}

impl WatchedTrigger {
	/// Nothing to recognise: the role is bound to a conventional chord, which
	/// `tauri-plugin-global-shortcut` services instead.
	pub const OFF: Self = Self {
		family: ModifierFamily::Off,
		side: KeySide::Either,
	};

	pub fn unsided(family: ModifierFamily) -> Self {
		Self {
			family,
			side: KeySide::Either,
		}
	}

	/// **The bit layout, which is the contract the atomics rest on.** Bits 0–1
	/// carry [`ModifierFamily`]'s `#[repr(u8)]` discriminant (0–3), bits 2–3 carry
	/// the side (0 `Either`, 1 `Left`, 2 `Right`). Bits 4–7 are unused and always
	/// zero. Two independent fields in one atomic rather than two atomics, so that
	/// a rebind can never be observed half-applied — a family from the new binding
	/// paired with a side from the old one would be a trigger nobody chose.
	fn pack(self) -> u8 {
		(self.family as u8) | (self.side.code() << 2)
	}

	fn unpack(bits: u8) -> Self {
		Self {
			family: ModifierFamily::from_code(bits & 0b11),
			side: KeySide::from_code((bits >> 2) & 0b11),
		}
	}
}

/// The live selector for each recogniser.
///
/// Capture ships bound to `Shift Shift`, summon to a conventional chord — so the
/// summon recogniser starts with nothing to watch and the capture one starts on
/// the shipped default. These are what the hook recognises between install and
/// the persisted bindings being loaded, which is why they are the shipped values
/// rather than `Off` for both.
static WATCHED_CAPTURE: AtomicU8 = AtomicU8::new(ModifierFamily::Shift as u8);
static WATCHED_SUMMON: AtomicU8 = AtomicU8::new(ModifierFamily::Off as u8);

/// Whether a shortcut recording session has both recognisers stood down.
///
/// `shortcuts::begin_recording` unregisters the plugin's chords so the webview
/// can see the keys the user presses. A double-tap binding is not the plugin's to
/// unregister, so without this it stays live *while the user is recording over
/// it* — and for summon that is not merely untidy: the double-tap toggles the
/// panel, hiding it cancels the recording session, and the session the user just
/// opened ends itself.
static MUTED: AtomicBool = AtomicBool::new(false);

fn selector(role: TriggerRole) -> &'static AtomicU8 {
	match role {
		TriggerRole::Capture => &WATCHED_CAPTURE,
		TriggerRole::Summon => &WATCHED_SUMMON,
	}
}

/// Points a recogniser at a different binding without tearing the hook down.
///
/// Reinstalling `WH_KEYBOARD_LL` to change one selector would be the wrong shape
/// entirely — the hook is installed on its own thread with a published thread id
/// and a message pump, and swapping it means a window with no hook at all.
pub fn watch(role: TriggerRole, trigger: WatchedTrigger) {
	selector(role).store(trigger.pack(), Ordering::Relaxed);
}

/// `Relaxed` is correct rather than merely cheap: each value is a single
/// independent selector that publishes no other memory, so there is nothing for
/// a stronger ordering to synchronise. The worst case is one gesture judged
/// against the previous binding.
pub fn watched(role: TriggerRole) -> WatchedTrigger {
	WatchedTrigger::unpack(selector(role).load(Ordering::Relaxed))
}

/// Stands both recognisers down, or lets them back up. See [`MUTED`].
pub fn mute(muted: bool) {
	MUTED.store(muted, Ordering::Relaxed);
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

/// Turns a raw `vkCode` into what the state machine should make of it, given the
/// side the binding asked for.
///
/// This is where the two spellings diverge, and the whole of the difference:
///
/// - **Unsided** (`want` is `Either`) passes the observed side straight through,
///   so the machine applies the rule it always has — both taps must be the same
///   physical key, whichever one that is.
/// - **Sided** (`want` is `Left` or `Right`) admits only that side. Everything
///   else, including the *other* side of the same family, becomes `Other` and
///   breaks the sequence, because a `LCtrl LCtrl` binding is not a Ctrl binding.
///
/// The consequence worth stating plainly: `classify` answers `Either` for a
/// generic `VK_SHIFT` whose side would not resolve from its scan code, which is
/// what remappers deliver — and `Either` satisfies no sided binding. A user whose
/// keyboard reports generic modifier codes therefore has to use the unsided
/// spelling. Admitting an unresolved code into a sided binding would be worse:
/// `LCtrl LCtrl` would fire on the right-hand key, which is the one thing the
/// user picked that spelling to avoid.
/// Resolves a generic two-sided modifier's side from its scan code, at most once
/// per key event however many recognisers ask.
///
/// The memo is the point. Both recognisers want the same answer for the same
/// event, and `MapVirtualKeyW` is the one OS call on the callback's hot path —
/// paying for it twice would double the only cost this module actually measures.
fn resolve_side(scan: u32, memo: &mut Option<Option<u32>>) -> Option<u32> {
	*memo.get_or_insert_with(|| {
		// SAFETY: no preconditions. An unmapped scan code yields 0, which is read
		// as "the side did not resolve" rather than as a virtual-key code.
		match unsafe { MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX) } {
			0 => None,
			resolved => Some(resolved),
		}
	})
}

fn observe(
	trigger: TriggerKey,
	want: KeySide,
	vk: u32,
	resolve_generic: impl FnOnce() -> Option<u32>,
) -> Observed {
	match trigger.classify(vk, resolve_generic) {
		Classified::Trigger { side } if want == KeySide::Either => Observed::Trigger(side),
		Classified::Trigger { side } if side == want => Observed::Trigger(want),
		_ => Observed::Other,
	}
}

// --- liveness ----------------------------------------------------------------

/// The generation of the hook that is live right now, or zero for none.
///
/// The startup install is only half the question. Windows removes a
/// `WH_KEYBOARD_LL` hook whose callback keeps missing `LowLevelHooksTimeout` and
/// tells the application nothing at all, so "did it install?" answered at startup
/// stays `true` forever while capture has silently stopped working.
///
/// A generation rather than a plain flag because hooks are now replaced while the
/// app runs, and the thread being replaced may not have finished dying. Every
/// clear is a compare-exchange against the clearing party's own generation, so an
/// old thread reaching its exit path after its successor is already installed
/// cannot report the *successor* as gone. A bare `AtomicBool` had exactly that
/// hazard, and its symptom would be the watchdog reinstalling a healthy hook in a
/// loop.
static LIVE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Handed out by [`install`]. Starts at one so that zero can mean "none".
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// How many hook threads were abandoned because `WM_QUIT` would not reach them.
///
/// A watermark, never decremented: an abandoned thread is parked in `GetMessageW`
/// forever, and its thread-local `HookState` still owns a clone of the trigger
/// `Sender`. Nothing can observe it dying, so nothing may assume it did. The
/// worker's receive loop ends only when every sender has been dropped, so
/// `CaptureHandle::shutdown` has to read this before deciding a join is safe —
/// the handle it tracks says nothing about the orphan.
static DETACHED_HOOK_THREADS: AtomicUsize = AtomicUsize::new(0);

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
	LIVE_GENERATION.load(Ordering::Relaxed) != 0
}

/// Whether any hook thread was ever abandoned, and so may still hold a trigger
/// sender open.
pub fn any_detached() -> bool {
	DETACHED_HOOK_THREADS.load(Ordering::Relaxed) > 0
}

/// Marks `generation` as no longer live — but only if it still is.
///
/// The compare-exchange is the whole point. Both the owning thread's exit path
/// and [`HookHandle::stop`] call this, and a replacement hook may already have
/// published a newer generation by the time either gets here.
fn retire(generation: u64) {
	let _ = LIVE_GENERATION.compare_exchange(
		generation,
		0,
		Ordering::Relaxed,
		Ordering::Relaxed,
	);
}

/// Retires the hook thread's generation however the thread leaves — including a
/// `GetMessageW` error, and a debug-build unwind out of the pump.
///
/// A plain call at the bottom of the closure covered neither: the flag stayed set
/// over a thread that was gone, so the watchdog kept injecting probes nothing
/// would swallow and `shortcuts` never stood up the chord that stands in for a
/// dead hook.
struct RetireOnExit(u64);

impl Drop for RetireOnExit {
	fn drop(&mut self) {
		retire(self.0);
	}
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
	/// Which binding fired. One channel carries both rather than two channels
	/// carrying one each, so the worker's receive loop — and the sender-drop
	/// protocol that ends it, which shutdown's join depends on — stays exactly as
	/// it was.
	pub role: TriggerRole,
}

/// One recogniser: a machine and the binding its partial sequence was recorded
/// under.
struct Recogniser {
	machine: DoubleTap,
	/// Compared against [`watched`] on every event, so a rebind that lands
	/// mid-gesture resets rather than pairing a tap of the old binding with a tap
	/// of the new one. The *whole* selector, side included: `Shift Shift` and
	/// `LShift LShift` are different bindings, and a sequence begun under one must
	/// not finish under the other.
	watched: WatchedTrigger,
}

impl Recogniser {
	fn new(watched: WatchedTrigger) -> Self {
		Self {
			machine: DoubleTap::new(DoubleTapConfig::default()),
			watched,
		}
	}

	/// Feeds one key event. Returns `true` on the key-up that completes this
	/// recogniser's double-tap.
	///
	/// `resolve_generic` is passed in rather than called here so the whole of this
	/// stays free of Win32 and testable — and so the callback can share one
	/// resolution between both recognisers instead of paying for two.
	fn feed(
		&mut self,
		watched: WatchedTrigger,
		vk: u32,
		is_up: bool,
		time_ms: u32,
		resolve_generic: impl FnOnce() -> Option<u32>,
	) -> bool {
		if watched != self.watched {
			self.machine.reset();
			self.watched = watched;
		}
		let Some(trigger) = watched.family.trigger() else {
			return false;
		};
		let observed = observe(trigger, watched.side, vk, resolve_generic);
		self.machine.on_key(observed, is_up, time_ms)
	}
}

struct HookState {
	capture: Recogniser,
	summon: Recogniser,
	tx: Sender<Trigger>,
	/// One capture in flight at a time. A bounded channel does not express this:
	/// once the worker receives, the slot is free again and a second trigger
	/// arriving mid-capture would produce a second note.
	///
	/// Deliberately **not** taken by a summon trigger: revealing a window is not a
	/// capture, and making the two share a gate would mean a capture in progress
	/// silently swallowed the user's summon.
	in_flight: Arc<AtomicBool>,
	/// Capture triggers are dropped until every startup gate has cleared.
	///
	/// Summon does not wait on it, and for the reason the gate exists: it is there
	/// so a capture cannot land in the default space before the space the user
	/// double-clicked is open. A summon writes nothing, so it has nothing to land
	/// in the wrong place.
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

		// The watchdog's liveness probe, and **the one event this callback
		// deliberately swallows** — see the note at the bottom of this function for
		// the rule it breaks. A probe carries no meaning for anybody else: passing
		// it on would type a stray F24 into whatever has focus, and this is the only
		// place in the system where Copper can stop it. Reaching this branch is
		// itself the proof the watchdog is after, since a hook Windows has removed
		// never gets here at all.
		//
		// All three conditions, not the tag alone. `dwExtraInfo` is a free-for-all
		// that any application may write anything into, and the cost of a collision
		// here is not a spurious probe — it is a real keystroke silently eaten,
		// which is the exact failure this whole module exists to prevent. Requiring
		// the probe's own key and a key message narrows a one-in-2^64 accident to
		// one that also has to be an F24 press. The tag compare short-circuits
		// first, so an ordinary keystroke still pays a single `usize` compare.
		if event.dwExtraInfo == PROBE_SIGNATURE && vk == VK_F24 && (is_up || is_down) {
			note_probe();
			return LRESULT(1);
		}

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

				// A recording session is open, so neither binding may fire. Both
				// machines are reset rather than merely ignored: a half-finished
				// sequence recorded before the lease must not pair with a tap after
				// it and produce a trigger from two gestures the user never made in
				// one. The probe branch is above this, so muting never blinds the
				// watchdog.
				if MUTED.load(Ordering::Relaxed) {
					state.capture.machine.reset();
					state.summon.machine.reset();
					return;
				}

				// One resolution shared by both recognisers — see `resolve_side`. The
				// two closures below borrow it in turn rather than at once, which is
				// why it is a memo passed by reference and not a single `FnMut` handed
				// to both.
				let mut resolution: Option<Option<u32>> = None;

				// Two relaxed loads per event. The compares inside `feed` are what
				// generalise task-005's Shift-specific machine: a binding change
				// resets that machine, and `Off` — the role bound to a conventional
				// chord instead — leaves it idle rather than recognising anything.
				let fired_capture = state.capture.feed(
					watched(TriggerRole::Capture),
					vk,
					is_up,
					event.time,
					|| resolve_side(scan, &mut resolution),
				);
				let fired_summon = state.summon.feed(
					watched(TriggerRole::Summon),
					vk,
					is_up,
					event.time,
					|| resolve_side(scan, &mut resolution),
				);

				// An unbounded channel, so neither send waits on a receiver the way a
				// bounded one would — parking the callback is what gets the hook
				// silently removed. It is not a hard real-time guarantee: a send takes
				// an uncontended lock and may allocate a node. Task-001 measured the
				// whole callback at 7.8 microseconds worst case against a budget of up
				// to 1000 ms, which is the evidence this rests on rather than the
				// absence of allocation.
				//
				// Both can only fire on one event if both bindings are the same family
				// on overlapping sides, which `shortcuts` refuses to store. Handling
				// them independently anyway costs one branch and means a registry that
				// somehow held such a pair produces two honest triggers rather than
				// one arbitrary winner.
				if fired_summon {
					// Neither gate: see the fields on `HookState` for why each is the
					// capture path's and not this one's.
					let _ = state.tx.send(Trigger {
						at: Instant::now(),
						role: TriggerRole::Summon,
					});
				}

				// The send is the last condition rather than the body, so the gate is
				// taken and the trigger queued in one expression and only the failure
				// needs a statement.
				if fired_capture
					&& state.armed.load(Ordering::SeqCst)
					&& state
						.in_flight
						.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
						.is_ok() && state
					.tx
					.send(Trigger {
						at: Instant::now(),
						role: TriggerRole::Capture,
					})
					.is_err()
				{
					// The worker is gone; do not leave the gate latched shut.
					state.in_flight.store(false, Ordering::SeqCst);
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
	generation: u64,
	join: Option<JoinHandle<()>>,
	/// What the first [`stop`](HookHandle::stop) concluded, so a later call
	/// reports the same thing.
	outcome: Option<bool>,
}

impl HookHandle {
	/// Uninstalls the hook and joins its thread. Idempotent.
	///
	/// Returns whether the quit actually reached the thread. That matters to the
	/// caller and is not merely diagnostic: a detached hook thread keeps running,
	/// and with it the thread-local state holding the trigger `Sender`. The worker
	/// is waiting on that channel to close, so joining the worker after a failed
	/// post would block forever — trading a leaked thread for a hung exit.
	///
	/// **A second call repeats the first's answer rather than inventing a better
	/// one.** Returning `true` on the grounds that the work had already been done
	/// turned a failed detach into a reported success, which is precisely the lie
	/// that hangs the worker join: the orphan is no less alive for having been
	/// abandoned twice.
	pub fn stop(&mut self) -> bool {
		if let Some(outcome) = self.outcome {
			return outcome;
		}
		// Whatever this handle once owned, the caller is asking for it to be gone.
		// Qualified by generation, so a stop arriving after a replacement is already
		// installed cannot report the replacement as dead.
		retire(self.generation);

		let outcome = match self.join.take() {
			// Check the post. A blind join after a failed post hangs shutdown
			// forever; detaching the thread instead is survivable, a deadlock at exit
			// is not.
			// SAFETY: no preconditions; failure is reported through the Result.
			Some(join) => {
				match unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) } {
					Ok(()) => {
						let _ = join.join();
						true
					}
					Err(err) => {
						// Dropping the handle without joining is what detaches it. Counted
						// process-wide, because from here on nothing can observe that
						// thread again and the sender it holds outlives this handle.
						DETACHED_HOOK_THREADS.fetch_add(1, Ordering::Relaxed);
						diagnostics::log_error(&format!(
							"[copper] capture: could not post WM_QUIT to the hook thread ({err}); \
							 detaching it rather than joining. It still holds a trigger sender, so \
							 the worker thread will be left rather than joined at exit"
						));
						false
					}
				}
			}
			// No join handle and no recorded outcome can only mean this handle was
			// built without one, which nothing does.
			None => true,
		};
		self.outcome = Some(outcome);
		outcome
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
/// The watched bindings are **not** parameters: they are the module-level
/// atomics [`watch`] sets, because task-008 rebinds them while the hook is
/// running and tearing the hook down to change one selector would leave a window
/// with no hook at all.
pub fn install(
	tx: Sender<Trigger>,
	in_flight: Arc<AtomicBool>,
	armed: Arc<AtomicBool>,
) -> Result<HookHandle, HookError> {
	let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();

	// Resolved here rather than on first use, so the callback's probe branch is a
	// load and a store and never a one-time initialisation.
	LazyLock::force(&EPOCH);

	// The thread publishes and retires this itself, rather than the installer
	// doing it around the handshake. That is what makes the flag track the thread
	// that actually owns the hook: a pump that exits on a `GetMessageW` error, or
	// unwinds in a debug build, clears it on the way out with no cooperation from
	// anybody.
	let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);

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
					capture: Recogniser::new(watched(TriggerRole::Capture)),
					summon: Recogniser::new(watched(TriggerRole::Summon)),
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

			// Published here, before the handshake, so that every path out of this
			// closure from this point on runs the guard's retire. Publishing from the
			// installer instead left a window in which a thread that died immediately
			// retired a generation that had not been announced yet, and the
			// installer's store then resurrected it.
			LIVE_GENERATION.store(generation, Ordering::Relaxed);
			let _retire = RetireOnExit(generation);

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
			generation,
			join: Some(join),
			outcome: None,
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

	// --- the sided selector ----------------------------------------------------
	// The table above is about timing and is written against `Observed`. This one
	// is about *which key counts*, so it is written against raw virtual-key codes
	// and runs the real selector: `observe` filtering, then the same machine.

	/// Feeds raw `vkCode`s through a recogniser watching `binding`.
	fn run_sided(
		binding: WatchedTrigger,
		script: &[(u32, bool, u32)],
		resolve: fn() -> Option<u32>,
	) -> usize {
		let mut recogniser = Recogniser::new(binding);
		script
			.iter()
			.filter(|(vk, is_up, time)| recogniser.feed(binding, *vk, *is_up, *time, resolve))
			.count()
	}

	/// A clean double-tap of `first` then `second`, well inside both bounds.
	fn two_taps(first: u32, second: u32) -> [(u32, bool, u32); 4] {
		[
			(first, DOWN, 0),
			(first, UP, 40),
			(second, DOWN, 120),
			(second, UP, 160),
		]
	}

	const NO_RESOLUTION: fn() -> Option<u32> = || None;

	#[test]
	fn a_sided_binding_answers_to_its_own_side_and_to_nothing_else() {
		let left = WatchedTrigger {
			family: ModifierFamily::Control,
			side: KeySide::Left,
		};
		let right = WatchedTrigger {
			family: ModifierFamily::Control,
			side: KeySide::Right,
		};
		let either = WatchedTrigger::unsided(ModifierFamily::Control);

		/// A name, the binding being watched, a script of raw key events, and how
		/// many times that binding must fire.
		type SidedRow = (&'static str, WatchedTrigger, [(u32, bool, u32); 4], usize);

		let cases: &[SidedRow] = &[
			("LCtrl LCtrl fires on the left key", left, two_taps(VK_LCONTROL, VK_LCONTROL), 1),
			("LCtrl LCtrl ignores the right key", left, two_taps(VK_RCONTROL, VK_RCONTROL), 0),
			(
				"LCtrl LCtrl is not satisfied by one tap of each",
				left,
				two_taps(VK_LCONTROL, VK_RCONTROL),
				0,
			),
			("RCtrl RCtrl fires on the right key", right, two_taps(VK_RCONTROL, VK_RCONTROL), 1),
			("RCtrl RCtrl ignores the left key", right, two_taps(VK_LCONTROL, VK_LCONTROL), 0),
			// The unsided spelling is unchanged: either side, both taps the same key.
			("Ctrl Ctrl still fires on the left key", either, two_taps(VK_LCONTROL, VK_LCONTROL), 1),
			(
				"Ctrl Ctrl still fires on the right key",
				either,
				two_taps(VK_RCONTROL, VK_RCONTROL),
				1,
			),
			(
				"Ctrl Ctrl still requires both taps to be the same physical key",
				either,
				two_taps(VK_LCONTROL, VK_RCONTROL),
				0,
			),
			// A sided binding is still a binding to one family.
			("a sided Ctrl binding ignores Shift", left, two_taps(VK_LSHIFT, VK_LSHIFT), 0),
		];

		for (name, binding, script, expected) in cases {
			assert_eq!(run_sided(*binding, script, NO_RESOLUTION), *expected, "{name}");
		}
	}

	#[test]
	fn a_generic_modifier_code_satisfies_the_unsided_spelling_and_no_sided_one() {
		// The remapper case, and the one place the two spellings are not simply
		// narrower and wider: a `VK_CONTROL` whose scan code will not resolve names
		// no side, and admitting it into `LCtrl LCtrl` would fire that binding on
		// the right-hand key — the one thing the spelling was chosen to avoid.
		let either = WatchedTrigger::unsided(ModifierFamily::Control);
		let left = WatchedTrigger {
			family: ModifierFamily::Control,
			side: KeySide::Left,
		};
		let generic = two_taps(VK_CONTROL, VK_CONTROL);

		assert_eq!(run_sided(either, &generic, NO_RESOLUTION), 1);
		assert_eq!(run_sided(left, &generic, NO_RESOLUTION), 0);

		// Resolvable is the ordinary case, and there the sided binding does answer.
		let resolves_left: fn() -> Option<u32> = || Some(VK_LCONTROL);
		assert_eq!(run_sided(left, &generic, resolves_left), 1);
		let resolves_right: fn() -> Option<u32> = || Some(VK_RCONTROL);
		assert_eq!(run_sided(left, &generic, resolves_right), 0);
	}

	#[test]
	fn a_recogniser_with_nothing_to_watch_never_fires() {
		// The conventional-chord case: the plugin services that binding, and this
		// recogniser must stay idle rather than recognising the family it last held.
		assert_eq!(
			run_sided(WatchedTrigger::OFF, &two_taps(VK_LSHIFT, VK_LSHIFT), NO_RESOLUTION),
			0
		);
	}

	#[test]
	fn changing_the_side_mid_sequence_resets_the_machine() {
		// `Shift Shift` and `LShift LShift` are different bindings, so a rebind
		// between the two taps must not let a tap recorded under one complete the
		// other.
		let unsided = WatchedTrigger::unsided(ModifierFamily::Shift);
		let sided = WatchedTrigger {
			family: ModifierFamily::Shift,
			side: KeySide::Left,
		};
		let mut recogniser = Recogniser::new(unsided);

		assert!(!recogniser.feed(unsided, VK_LSHIFT, DOWN, 0, NO_RESOLUTION));
		assert!(!recogniser.feed(unsided, VK_LSHIFT, UP, 40, NO_RESOLUTION));
		// The rebind lands between the taps. Without the reset this second tap would
		// complete the pair and fire under a binding half of it was never judged by.
		assert!(!recogniser.feed(sided, VK_LSHIFT, DOWN, 120, NO_RESOLUTION));
		assert!(!recogniser.feed(sided, VK_LSHIFT, UP, 160, NO_RESOLUTION));
		// That tap is the *first* of the new binding's pair rather than nothing at
		// all, so the next one fires: the rebind cost one gesture, not the binding.
		assert!(!recogniser.feed(sided, VK_LSHIFT, DOWN, 220, NO_RESOLUTION));
		assert!(recogniser.feed(sided, VK_LSHIFT, UP, 260, NO_RESOLUTION));
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
		// The two selectors ship as the two bindings do: capture on `Shift Shift`,
		// summon on a conventional chord the plugin services.
		assert_eq!(
			WatchedTrigger::unpack(WATCHED_CAPTURE.load(Ordering::Relaxed)),
			WatchedTrigger::unsided(ModifierFamily::Shift)
		);
		assert_eq!(
			WatchedTrigger::unpack(WATCHED_SUMMON.load(Ordering::Relaxed)),
			WatchedTrigger::OFF
		);
	}

	/// The packing is a contract two atomics rest on, and its failure mode is a
	/// binding silently becoming a different binding — so every reachable value
	/// round-trips rather than a sample of them.
	#[test]
	fn every_selector_survives_the_round_trip_through_one_byte() {
		let families = [
			ModifierFamily::Off,
			ModifierFamily::Shift,
			ModifierFamily::Control,
			ModifierFamily::Alt,
		];
		let sides = [KeySide::Either, KeySide::Left, KeySide::Right];
		let mut seen: std::collections::HashSet<u8> = std::collections::HashSet::new();

		for family in families {
			for side in sides {
				let trigger = WatchedTrigger { family, side };
				let bits = trigger.pack();
				assert_eq!(WatchedTrigger::unpack(bits), trigger);
				// Distinct bytes, or one binding could be read as another.
				assert!(seen.insert(bits), "{trigger:?} collides with another selector");
				// The layout the doc comment promises: two bits each, nothing above.
				assert_eq!(bits & !0b1111, 0, "{trigger:?} used a bit outside the layout");
			}
		}

		// The unsided spelling packs to the bare family discriminant, which is what
		// lets the shipped defaults be written as `ModifierFamily::X as u8` in a
		// const initialiser.
		assert_eq!(
			WatchedTrigger::unsided(ModifierFamily::Shift).pack(),
			ModifierFamily::Shift as u8
		);
		assert_eq!(WatchedTrigger::OFF.pack(), ModifierFamily::Off as u8);
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
			// The shipped selectors, which is what the globals still hold: nothing
			// below calls `watch`, because those atomics are process-wide and these
			// tests share a process with everything else.
			*cell.borrow_mut() = Some(HookState {
				capture: Recogniser::new(watched(TriggerRole::Capture)),
				summon: Recogniser::new(watched(TriggerRole::Summon)),
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
			cell.borrow().as_ref().is_some_and(|state| {
				state.capture.machine.state == State::Idle
					&& state.summon.machine.state == State::Idle
			})
		})
	}

	/// A stamp value no elapsed-millisecond count can be, so "the stamp moved"
	/// cannot be satisfied by the initial zero or by a probe landing in the same
	/// millisecond as the baseline.
	const NEVER: u64 = u64::MAX;

	#[test]
	fn the_two_tags_are_handled_differently_and_neither_reaches_the_machine() {
		let triggers = install_test_state();
		// The two tags have to be distinguishable in the first place, or one branch
		// shadows the other and the difference below proves nothing.
		assert_ne!(EXTRA_INFO_SIGNATURE, PROBE_SIGNATURE);

		PROBE_SEEN_MS.store(NEVER, Ordering::Relaxed);
		let swallowed = feed(PROBE_SIGNATURE, VK_F24, WM_KEYDOWN);
		assert_ne!(
			swallowed.0, 0,
			"a probe must be swallowed, or it lands in whatever has focus"
		);
		assert_ne!(
			probe_stamp(),
			NEVER,
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

		// A recording session stands both recognisers down. The sequence opened just
		// above is dropped rather than parked: a tap from before the lease pairing
		// with one from after it would be a trigger made of two separate gestures.
		mute(true);
		assert_eq!(
			feed(0, VK_LSHIFT, WM_KEYDOWN).0,
			0,
			"a muted hook must still pass the user's keys on"
		);
		assert!(machine_is_idle(), "a muted recogniser holds no sequence");

		// And the probe still gets through, so recording cannot blind the watchdog
		// into reinstalling a hook that was never gone.
		PROBE_SEEN_MS.store(NEVER, Ordering::Relaxed);
		assert_ne!(feed(PROBE_SIGNATURE, VK_F24, WM_KEYDOWN).0, 0);
		assert_ne!(probe_stamp(), NEVER, "muting must not swallow the liveness probe");

		mute(false);
		assert_eq!(feed(0, VK_LSHIFT, WM_KEYDOWN).0, 0);
		assert!(!machine_is_idle(), "unmuting must let a sequence open again");

		assert!(
			triggers.try_recv().is_err(),
			"none of these events completes a double-tap"
		);
		HOOK_STATE.with(|cell| *cell.borrow_mut() = None);
	}

	#[test]
	fn the_probe_branch_needs_all_three_of_its_conditions() {
		let _triggers = install_test_state();

		// `dwExtraInfo` is a free-for-all: any application may write any value into
		// it, so a tag match on its own would let somebody else's collision eat a
		// real keystroke — the exact failure the watchdog exists to prevent. Each
		// case below carries the tag and fails one other condition, and must
		// therefore be passed on untouched.
		for (name, vk, message) in [
			("the tag on a key that is not the probe's", VK_LSHIFT, WM_KEYDOWN),
			("the tag on a key that is not the probe's", 0x41, WM_KEYUP),
			("the tag on something that is not a key message", VK_F24, WM_USER),
		] {
			PROBE_SEEN_MS.store(NEVER, Ordering::Relaxed);
			let result = feed(PROBE_SIGNATURE, vk, message);
			assert_eq!(result.0, 0, "{name} must still reach the next hook");
			assert_eq!(
				probe_stamp(),
				NEVER,
				"{name} must not be recorded as proof the callback ran"
			);
		}

		// And the key-up half of the real probe is swallowed just like its key-down,
		// because the watchdog sends the pair rather than stranding F24 held.
		PROBE_SEEN_MS.store(NEVER, Ordering::Relaxed);
		assert_ne!(feed(PROBE_SIGNATURE, VK_F24, WM_KEYUP).0, 0);
		assert_ne!(probe_stamp(), NEVER);

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
