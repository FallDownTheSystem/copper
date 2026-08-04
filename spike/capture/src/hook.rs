//! `WH_KEYBOARD_LL` installation, the dedicated hook thread, and the pure
//! double-tap state machine.
//!
//! The callback is the hottest and most dangerous code in the spike. Windows
//! silently removes a low-level hook whose callback exceeds
//! `HKEY_CURRENT_USER\Control Panel\Desktop\LowLevelHooksTimeout` (capped at
//! 1000 ms on Windows 10 1709+), with no notification of any kind. So the
//! callback classifies the event, feeds a small state machine, and on a trigger
//! sends over a channel. Nothing else — no logging, no Win32 calls beyond
//! `CallNextHookEx`, no I/O.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use serde::Serialize;
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ, REG_VALUE_TYPE, RRF_RT_REG_DWORD,
    RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VSC_TO_VK_EX};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
    LLKHF_INJECTED, MSG, PM_NOREMOVE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WM_USER,
};

use crate::clipboard::COPPER_INJECTED_TAG;

// ---------------------------------------------------------------------------
// Virtual-key constants used by the trigger specification
// ---------------------------------------------------------------------------

pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12;
pub const VK_LSHIFT: u32 = 0xA0;
pub const VK_RSHIFT: u32 = 0xA1;
pub const VK_LCONTROL: u32 = 0xA2;
pub const VK_RCONTROL: u32 = 0xA3;
pub const VK_LMENU: u32 = 0xA4;
pub const VK_RMENU: u32 = 0xA5;

// ---------------------------------------------------------------------------
// Instrumentation (acceptance criterion 3)
// ---------------------------------------------------------------------------

/// Longest single callback observed, in nanoseconds.
pub static MAX_CALLBACK_NS: AtomicU64 = AtomicU64::new(0);
/// Number of callbacks that have run.
pub static CALLBACK_COUNT: AtomicU64 = AtomicU64::new(0);
/// Sum of callback durations, for the mean.
pub static TOTAL_CALLBACK_NS: AtomicU64 = AtomicU64::new(0);
/// How often a generic `VK_SHIFT` arrived whose side `MapVirtualKeyW` would not
/// resolve. Counted rather than logged because the callback may not log.
pub static GENERIC_SIDE_UNRESOLVED: AtomicU64 = AtomicU64::new(0);
/// How often a *triggering* event carried `LLKHF_INJECTED` — i.e. came from a
/// remapper such as PowerToys or AutoHotkey rather than from hardware.
pub static INJECTED_TRIGGER_COUNT: AtomicU64 = AtomicU64::new(0);
/// Events dropped because our own `Ctrl+C` injection tag was on them.
pub static SELF_INJECTED_FILTERED: AtomicU64 = AtomicU64::new(0);

/// Mean callback duration in nanoseconds, or 0 if nothing has run yet.
pub fn mean_callback_ns() -> u64 {
    let n = CALLBACK_COUNT.load(Ordering::Relaxed);
    if n == 0 {
        0
    } else {
        TOTAL_CALLBACK_NS.load(Ordering::Relaxed) / n
    }
}

// ---------------------------------------------------------------------------
// Pure state machine — no Win32 in this section, so it is unit-testable
// ---------------------------------------------------------------------------

/// Which side of a two-sided modifier produced an event.
///
/// `Either` covers two situations that both have to behave as "matches whatever
/// the other tap was": a trigger key with no sides at all (any non-modifier
/// key), and a generic `VK_SHIFT` whose side could not be resolved from its
/// scan code. The second case is real — keyboard remappers can deliver a
/// generic `VK_SHIFT`, and this spike deliberately accepts remapped input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeySide {
    Left,
    Right,
    Either,
}

impl KeySide {
    fn matches(self, other: KeySide) -> bool {
        self == other || self == KeySide::Either || other == KeySide::Either
    }

    /// Prefer a concrete side over `Either` when refining a stored value, so a
    /// sequence that starts unresolved but later resolves records the real side.
    fn refine(self, other: KeySide) -> KeySide {
        if self == KeySide::Either {
            other
        } else {
            self
        }
    }
}

impl fmt::Display for KeySide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeySide::Left => f.write_str("left"),
            KeySide::Right => f.write_str("right"),
            KeySide::Either => f.write_str("either"),
        }
    }
}

/// A key event as the state machine sees it: either the configured trigger key
/// on some side, or anything else at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    Trigger(KeySide),
    Other,
}

/// The two timing rules, plus the hold guard.
///
/// `tap_max_ms` and `gap_max_ms` are separate deliberately. A single
/// start-to-finish window would conflate holding with tapping: a deliberate but
/// slow second press would fail while a slow hold-and-release could pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoubleTapConfig {
    /// Maximum duration of one press, key-down to its own key-up.
    pub tap_max_ms: u32,
    /// Maximum gap between the first key-up and the second key-down.
    pub gap_max_ms: u32,
    /// Absolute hold ceiling per press. At the defaults `tap_max_ms` (250) is
    /// the stricter of the two and this never binds; it only takes effect if
    /// `tap_max_ms` is configured above it.
    pub hold_max_ms: u32,
}

impl Default for DoubleTapConfig {
    fn default() -> Self {
        Self {
            tap_max_ms: 250,
            gap_max_ms: 400,
            hold_max_ms: 500,
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
/// the hook callback*.
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

    /// Feed one key event. Returns `true` on the key-*up* that completes a
    /// double-tap, and only then.
    pub fn on_key(&mut self, observed: Observed, is_up: bool, time_ms: u32) -> bool {
        let side = match observed {
            // Any other key, down or up, breaks the sequence. This is what stops
            // `Shift+A` from ever counting as a tap, and it subsumes the
            // "dirty" invalidation guard: a foreign key pressed while the
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

    fn on_down(&mut self, side: KeySide, t: u32) {
        self.state = match self.state {
            State::Idle => State::FirstDown { side, down: t },

            // Auto-repeat. While a key is physically held Windows delivers
            // repeated key-downs to the hook. The key is already recorded as
            // down, so this is not a new press: keep the original `down`
            // timestamp, or a long hold would keep resetting its own clock and
            // could satisfy `tap_max_ms` on release.
            State::FirstDown { side: s, down } if s.matches(side) => State::FirstDown {
                side: s.refine(side),
                down,
            },
            State::SecondDown { side: s, down } if s.matches(side) => State::SecondDown {
                side: s.refine(side),
                down,
            },

            State::FirstUp { side: s, up } if s.matches(side) => {
                if t.wrapping_sub(up) <= self.cfg.gap_max_ms {
                    State::SecondDown {
                        side: s.refine(side),
                        down: t,
                    }
                } else {
                    // Too slow to be the second half of a double-tap, but a
                    // perfectly good *first* tap of the next one. Starting over
                    // rather than idling means a slow tap does not poison the
                    // deliberate double-tap that follows it.
                    State::FirstDown { side, down: t }
                }
            }

            // A different side, at any point. Both taps must be the same side,
            // so the sequence is broken — but this press legitimately starts a
            // new one. Left-then-right therefore yields no trigger, which is
            // what acceptance criterion 2 requires.
            _ => State::FirstDown { side, down: t },
        };
    }

    fn on_up(&mut self, side: KeySide, t: u32) -> bool {
        match self.state {
            State::FirstDown { side: s, down } if s.matches(side) => {
                self.state = if self.press_was_a_tap(down, t) {
                    State::FirstUp {
                        side: s.refine(side),
                        up: t,
                    }
                } else {
                    State::Idle
                };
                false
            }
            State::SecondDown { side: s, down } if s.matches(side) => {
                let fired = self.press_was_a_tap(down, t);
                self.state = State::Idle;
                fired
            }
            // An up for the other side, or an up with no matching down (we
            // started listening mid-press). Neither can complete a tap.
            _ => {
                self.state = State::Idle;
                false
            }
        }
    }

    fn press_was_a_tap(&self, down: u32, up: u32) -> bool {
        let held = up.wrapping_sub(down);
        held <= self.cfg.tap_max_ms && held <= self.cfg.hold_max_ms
    }
}

/// The trigger key expressed as the low-level hook actually reports it.
///
/// The hook reports `VK_LSHIFT` / `VK_RSHIFT` in practice rather than the
/// generic `VK_SHIFT`, but that is not documented as guaranteed and remappers
/// can deliver the generic code, so all three are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerKey {
    pub generic: u32,
    pub left: Option<u32>,
    pub right: Option<u32>,
}

/// The result of matching a raw `vkCode` against the configured trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classified {
    Trigger {
        side: KeySide,
        /// A generic modifier code arrived whose side would not resolve.
        generic_unresolved: bool,
    },
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
    pub const MENU: TriggerKey = TriggerKey {
        generic: VK_MENU,
        left: Some(VK_LMENU),
        right: Some(VK_RMENU),
    };

    /// A trigger with no sides — any ordinary key.
    pub fn plain(vk: u32) -> Self {
        Self {
            generic: vk,
            left: None,
            right: None,
        }
    }

    /// Resolve a `--trigger-key` value to a specification. Any of the three
    /// codes for a two-sided modifier selects that whole modifier.
    pub fn from_vk(vk: u32) -> Self {
        match vk {
            VK_SHIFT | VK_LSHIFT | VK_RSHIFT => Self::SHIFT,
            VK_CONTROL | VK_LCONTROL | VK_RCONTROL => Self::CONTROL,
            VK_MENU | VK_LMENU | VK_RMENU => Self::MENU,
            other => Self::plain(other),
        }
    }

    pub fn label(&self) -> &'static str {
        match self.generic {
            VK_SHIFT => "Shift",
            VK_CONTROL => "Ctrl",
            VK_MENU => "Alt",
            _ => "custom",
        }
    }

    /// Match a `vkCode` against this trigger.
    ///
    /// `resolve_generic` is the `MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX)` step,
    /// injected as a closure so the whole classifier stays testable without
    /// Win32. It is called only for a generic two-sided modifier code.
    pub fn classify(&self, vk: u32, resolve_generic: impl FnOnce() -> Option<u32>) -> Classified {
        if self.left == Some(vk) {
            return Classified::Trigger {
                side: KeySide::Left,
                generic_unresolved: false,
            };
        }
        if self.right == Some(vk) {
            return Classified::Trigger {
                side: KeySide::Right,
                generic_unresolved: false,
            };
        }
        if vk != self.generic {
            return Classified::Other;
        }
        // The generic code for this trigger.
        if self.left.is_none() {
            // Sideless trigger: the generic code *is* the key.
            return Classified::Trigger {
                side: KeySide::Either,
                generic_unresolved: false,
            };
        }
        match resolve_generic() {
            Some(resolved) if self.left == Some(resolved) => Classified::Trigger {
                side: KeySide::Left,
                generic_unresolved: false,
            },
            Some(resolved) if self.right == Some(resolved) => Classified::Trigger {
                side: KeySide::Right,
                generic_unresolved: false,
            },
            // Could not resolve a side. Treat it as matching either side rather
            // than dropping it — dropping would silently break the trigger for
            // remapper users — and record that it happened.
            _ => Classified::Trigger {
                side: KeySide::Either,
                generic_unresolved: true,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Channel payloads
// ---------------------------------------------------------------------------

/// Sent on the completing key-up. Carries `injected` because the callback may
/// not log, and by the time the worker sees the trigger the flag is otherwise
/// gone.
#[derive(Debug, Clone, Copy)]
pub struct Trigger {
    pub at: Instant,
    pub injected: bool,
    pub side: KeySide,
}

/// Every key event, when the raw tap is enabled. Exists solely so the Tauri
/// probe can log OS-reserved system combinations as a control — the trigger
/// channel structurally cannot report that Alt+Tab was pressed.
#[derive(Debug, Clone, Copy)]
pub struct RawKey {
    pub vk: u32,
    pub is_up: bool,
    pub injected: bool,
}

// ---------------------------------------------------------------------------
// Hook thread and callback
// ---------------------------------------------------------------------------

struct HookState {
    machine: DoubleTap,
    trigger: TriggerKey,
    tx: Sender<Trigger>,
    raw_tx: Option<Sender<RawKey>>,
}

thread_local! {
    static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

unsafe extern "system" fn keyboard_proc(ncode: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if ncode < 0 {
        return CallNextHookEx(None, ncode, wparam, lparam);
    }

    let started = Instant::now();

    // SAFETY: for ncode >= 0 (HC_ACTION) the OS documents lparam as a pointer to
    // a KBDLLHOOKSTRUCT that is valid for the duration of this call.
    let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    let vk = kb.vkCode;
    let scan = kb.scanCode;
    let extra = kb.dwExtraInfo;
    let injected = kb.flags.0 & LLKHF_INJECTED.0 != 0;
    let time_ms = kb.time;

    let msg = wparam.0 as u32;
    let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;

    // Our own synthesized Ctrl+C must not feed back into the state machine.
    // Note this filters only *our* tag, never `LLKHF_INJECTED` generally:
    // PowerToys Keyboard Manager and AutoHotkey deliver genuine user intent as
    // injected input, and rejecting all of it would silently break the trigger
    // for those users.
    if extra == COPPER_INJECTED_TAG {
        SELF_INJECTED_FILTERED.fetch_add(1, Ordering::Relaxed);
        record_duration(started);
        return CallNextHookEx(None, ncode, wparam, lparam);
    }

    if is_up || is_down {
        HOOK_STATE.with(|cell| {
            // `try_borrow_mut`, not `borrow_mut`. A panic here would cross an
            // `extern "system"` boundary, which aborts the process — the worst
            // possible outcome for a background capture tool, and one that
            // would present as a mysterious crash rather than a failed capture.
            // Re-entrancy is not currently possible (nothing inside this borrow
            // pumps messages), so this should never fail; it is here so that
            // stays true if someone later adds a call that does.
            let Ok(mut borrow) = cell.try_borrow_mut() else {
                return;
            };
            let Some(state) = borrow.as_mut() else {
                return;
            };

            if let Some(raw) = &state.raw_tx {
                let _ = raw.send(RawKey {
                    vk,
                    is_up,
                    injected,
                });
            }

            let classified = state
                .trigger
                .classify(vk, || match MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX) {
                    0 => None,
                    resolved => Some(resolved),
                });

            let observed = match classified {
                Classified::Trigger {
                    side,
                    generic_unresolved,
                } => {
                    if generic_unresolved {
                        GENERIC_SIDE_UNRESOLVED.fetch_add(1, Ordering::Relaxed);
                    }
                    Observed::Trigger(side)
                }
                Classified::Other => Observed::Other,
            };

            if state.machine.on_key(observed, is_up, time_ms) {
                if injected {
                    INJECTED_TRIGGER_COUNT.fetch_add(1, Ordering::Relaxed);
                }
                let side = match observed {
                    Observed::Trigger(side) => side,
                    Observed::Other => KeySide::Either,
                };
                // Unbounded mpsc: `send` never blocks. A bounded channel here
                // could park the callback and get the hook silently removed.
                let _ = state.tx.send(Trigger {
                    at: Instant::now(),
                    injected,
                    side,
                });
            }
        });
    }

    record_duration(started);
    // Always pass the event on. Returning non-zero would swallow Shift from the
    // target application.
    CallNextHookEx(None, ncode, wparam, lparam)
}

#[inline]
fn record_duration(started: Instant) {
    let ns = started.elapsed().as_nanos() as u64;
    MAX_CALLBACK_NS.fetch_max(ns, Ordering::Relaxed);
    TOTAL_CALLBACK_NS.fetch_add(ns, Ordering::Relaxed);
    CALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Install / uninstall
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum HookError {
    Spawn(std::io::Error),
    Install(String),
    ThreadGone,
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HookError::Spawn(e) => write!(f, "could not spawn the hook thread: {e}"),
            HookError::Install(e) => write!(f, "SetWindowsHookExW failed: {e}"),
            HookError::ThreadGone => f.write_str("hook thread exited before reporting readiness"),
        }
    }
}

impl std::error::Error for HookError {}

/// Owns the hook thread. Dropping it uninstalls the hook and joins the thread.
pub struct HookHandle {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl HookHandle {
    pub fn thread_id(&self) -> u32 {
        self.thread_id
    }
}

impl Drop for HookHandle {
    fn drop(&mut self) {
        // Check the post. A blind join after a failed post hangs shutdown
        // forever; dropping the JoinHandle instead detaches the thread, which
        // is survivable, whereas a deadlock at exit is not.
        let posted = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        match posted {
            Ok(()) => {
                if let Some(join) = self.join.take() {
                    let _ = join.join();
                }
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    thread_id = self.thread_id,
                    "PostThreadMessageW(WM_QUIT) failed; detaching hook thread instead of joining"
                );
                self.join.take();
            }
        }
    }
}

pub fn install(
    trigger: TriggerKey,
    cfg: DoubleTapConfig,
    tx: Sender<Trigger>,
) -> Result<HookHandle, HookError> {
    install_with_raw(trigger, cfg, tx, None)
}

/// As [`install`], plus a tap that forwards every key event. Off in normal
/// operation: it makes the callback do more work than the measured hot path.
pub fn install_with_raw(
    trigger: TriggerKey,
    cfg: DoubleTapConfig,
    tx: Sender<Trigger>,
    raw_tx: Option<Sender<RawKey>>,
) -> Result<HookHandle, HookError> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();

    let join = thread::Builder::new()
        .name("copper-hook".to_owned())
        .spawn(move || {
            let mut msg = MSG::default();

            // A thread has no message queue until it calls a message function.
            // Force the queue into existence *before* the thread id is
            // published, or `PostThreadMessageW` from `HookHandle::drop` can
            // fail with ERROR_INVALID_THREAD_ID, the WM_QUIT is lost, and the
            // join blocks forever.
            unsafe {
                let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
            }

            HOOK_STATE.with(|cell| {
                *cell.borrow_mut() = Some(HookState {
                    machine: DoubleTap::new(cfg),
                    trigger,
                    tx,
                    raw_tx,
                });
            });

            // Pass the executable's module handle, not NULL. NULL is documented
            // as valid only when dwThreadId names a thread in this process; it
            // commonly works anyway, but there is no reason to rely on
            // undocumented behaviour for a one-line change.
            let installed = unsafe {
                GetModuleHandleW(None).and_then(|hmod| {
                    SetWindowsHookExW(
                        WH_KEYBOARD_LL,
                        Some(keyboard_proc),
                        Some(HINSTANCE(hmod.0)),
                        0,
                    )
                })
            };

            let hook: HHOOK = match installed {
                Ok(h) => h,
                Err(e) => {
                    HOOK_STATE.with(|cell| *cell.borrow_mut() = None);
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };

            let tid = unsafe { GetCurrentThreadId() };
            if ready_tx.send(Ok(tid)).is_err() {
                // Nobody is waiting for us; unwind rather than pump forever.
                unsafe {
                    let _ = UnhookWindowsHookEx(hook);
                }
                HOOK_STATE.with(|cell| *cell.borrow_mut() = None);
                return;
            }

            loop {
                // GetMessageW: >0 normal message, 0 for WM_QUIT, -1 on error.
                let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if r.0 <= 0 {
                    break;
                }
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }

            // Uninstall on the installing thread, so install and uninstall stay
            // on one thread.
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
        Ok(Err(e)) => {
            let _ = join.join();
            Err(HookError::Install(e))
        }
        Err(_) => {
            let _ = join.join();
            Err(HookError::ThreadGone)
        }
    }
}

/// `HKEY_CURRENT_USER\Control Panel\Desktop\LowLevelHooksTimeout`, in
/// milliseconds. `None` means the value is unset and the system default applies.
///
/// The value is stored as `REG_DWORD` on some machines and `REG_SZ` on others,
/// so both are handled.
pub fn low_level_hooks_timeout() -> Option<u32> {
    let mut buf = [0u8; 64];
    let mut cb = buf.len() as u32;
    let mut kind = REG_VALUE_TYPE(0);

    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\Desktop"),
            w!("LowLevelHooksTimeout"),
            RRF_RT_REG_DWORD | RRF_RT_REG_SZ,
            Some(&mut kind),
            Some(buf.as_mut_ptr().cast()),
            Some(&mut cb),
        )
    };
    if status.is_err() {
        return None;
    }

    match kind {
        REG_DWORD if cb as usize >= 4 => {
            Some(u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]))
        }
        REG_SZ => {
            let units = (cb as usize / 2).min(buf.len() / 2);
            let wide: Vec<u16> = (0..units)
                .map(|i| u16::from_ne_bytes([buf[i * 2], buf[i * 2 + 1]]))
                .take_while(|&c| c != 0)
                .collect();
            String::from_utf16_lossy(&wide).trim().parse().ok()
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests — the pure state machine only. These cover exactly the cases fingers
// cannot reliably reproduce: threshold boundaries, auto-repeat, and rollover.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const L: Observed = Observed::Trigger(KeySide::Left);
    const R: Observed = Observed::Trigger(KeySide::Right);
    const OTHER: Observed = Observed::Other;

    const DOWN: bool = false;
    const UP: bool = true;

    fn machine() -> DoubleTap {
        DoubleTap::new(DoubleTapConfig::default())
    }

    /// Feed a script of `(observed, is_up, time)` and return how many times the
    /// machine fired.
    fn run(m: &mut DoubleTap, script: &[(Observed, bool, u32)]) -> usize {
        script
            .iter()
            .filter(|(o, up, t)| m.on_key(*o, *up, *t))
            .count()
    }

    #[test]
    fn clean_double_tap_fires_exactly_once() {
        let mut m = machine();
        let fires = run(
            &mut m,
            &[(L, DOWN, 0), (L, UP, 40), (L, DOWN, 120), (L, UP, 160)],
        );
        assert_eq!(fires, 1);
    }

    #[test]
    fn single_tap_does_not_fire() {
        let mut m = machine();
        assert_eq!(run(&mut m, &[(L, DOWN, 0), (L, UP, 40)]), 0);
    }

    #[test]
    fn holding_the_trigger_never_satisfies_the_first_tap() {
        let mut m = machine();
        // Held for 300 ms — past tap_max (250). The release must not arm.
        let fires = run(
            &mut m,
            &[(L, DOWN, 0), (L, UP, 300), (L, DOWN, 340), (L, UP, 380)],
        );
        assert_eq!(fires, 0, "a hold must not count as the first tap");
    }

    #[test]
    fn auto_repeat_key_downs_do_not_advance_or_reset_the_clock() {
        let mut m = machine();
        // Windows repeats key-down while held. Held 0..300 with repeats: the
        // release at 300 is still over tap_max relative to the *original* down.
        let fires = run(
            &mut m,
            &[
                (L, DOWN, 0),
                (L, DOWN, 100),
                (L, DOWN, 200),
                (L, DOWN, 280),
                (L, UP, 300),
                (L, DOWN, 340),
                (L, UP, 380),
            ],
        );
        assert_eq!(
            fires, 0,
            "auto-repeat must not restart the press timer and rescue a hold"
        );
    }

    #[test]
    fn auto_repeat_during_the_second_press_also_holds_its_clock() {
        let mut m = machine();
        let fires = run(
            &mut m,
            &[
                (L, DOWN, 0),
                (L, UP, 40),
                (L, DOWN, 100),
                (L, DOWN, 200),
                (L, DOWN, 300),
                (L, UP, 400), // 300 ms after the second down: over tap_max
            ],
        );
        assert_eq!(fires, 0);
    }

    #[test]
    fn mixed_sides_do_not_fire() {
        let mut m = machine();
        assert_eq!(
            run(
                &mut m,
                &[(L, DOWN, 0), (L, UP, 40), (R, DOWN, 100), (R, UP, 140)]
            ),
            0,
            "left then right must not trigger"
        );
    }

    #[test]
    fn a_different_key_mid_sequence_resets() {
        let mut m = machine();
        // Shift+A: the A must break the sequence outright.
        let fires = run(
            &mut m,
            &[
                (L, DOWN, 0),
                (OTHER, DOWN, 10),
                (OTHER, UP, 20),
                (L, UP, 30),
                (L, DOWN, 60),
                (L, UP, 90),
            ],
        );
        assert_eq!(fires, 0);
    }

    #[test]
    fn a_different_key_between_the_two_taps_resets() {
        let mut m = machine();
        let fires = run(
            &mut m,
            &[
                (L, DOWN, 0),
                (L, UP, 40),
                (OTHER, DOWN, 60),
                (OTHER, UP, 70),
                (L, DOWN, 100),
                (L, UP, 140),
            ],
        );
        assert_eq!(fires, 0);
    }

    #[test]
    fn tap_max_boundary_is_inclusive() {
        let mut m = machine();
        // First press exactly at tap_max (250) — must still arm and fire.
        assert_eq!(
            run(
                &mut m,
                &[(L, DOWN, 0), (L, UP, 250), (L, DOWN, 300), (L, UP, 340)]
            ),
            1
        );

        let mut m = machine();
        // One millisecond past — must not.
        assert_eq!(
            run(
                &mut m,
                &[(L, DOWN, 0), (L, UP, 251), (L, DOWN, 300), (L, UP, 340)]
            ),
            0
        );
    }

    #[test]
    fn second_press_tap_max_boundary_is_inclusive() {
        let mut m = machine();
        assert_eq!(
            run(
                &mut m,
                &[(L, DOWN, 0), (L, UP, 40), (L, DOWN, 100), (L, UP, 350)]
            ),
            1,
            "second press exactly at tap_max must fire"
        );

        let mut m = machine();
        assert_eq!(
            run(
                &mut m,
                &[(L, DOWN, 0), (L, UP, 40), (L, DOWN, 100), (L, UP, 351)]
            ),
            0,
            "second press one millisecond past tap_max must not fire"
        );
    }

    #[test]
    fn gap_max_boundary_is_inclusive() {
        let mut m = machine();
        // First up at 40, second down at 440: gap of exactly 400.
        assert_eq!(
            run(
                &mut m,
                &[(L, DOWN, 0), (L, UP, 40), (L, DOWN, 440), (L, UP, 470)]
            ),
            1
        );

        let mut m = machine();
        // Gap of 401.
        assert_eq!(
            run(
                &mut m,
                &[(L, DOWN, 0), (L, UP, 40), (L, DOWN, 441), (L, UP, 470)]
            ),
            0
        );
    }

    #[test]
    fn a_too_slow_second_tap_becomes_a_new_first_tap() {
        let mut m = machine();
        // Tap, long pause, tap, tap. Only the last pair is a double-tap.
        let fires = run(
            &mut m,
            &[
                (L, DOWN, 0),
                (L, UP, 40),
                (L, DOWN, 2000),
                (L, UP, 2040), // too slow to pair with the first
                (L, DOWN, 2100),
                (L, UP, 2140), // pairs with the one at 2000
            ],
        );
        assert_eq!(fires, 1, "a slow tap must not poison the next double-tap");
    }

    #[test]
    fn tick_count_rollover_mid_sequence_still_fires() {
        let mut m = machine();
        // The DWORD tick count wraps roughly every 49.7 days of uptime. Every
        // elapsed calculation must use wrapping_sub; a plain subtraction would
        // underflow here and panic inside the hook callback in a debug build.
        let base = u32::MAX - 50;
        let fires = run(
            &mut m,
            &[
                (L, DOWN, base),
                (L, UP, base.wrapping_add(40)), // wraps past u32::MAX
                (L, DOWN, base.wrapping_add(120)),
                (L, UP, base.wrapping_add(160)),
            ],
        );
        assert_eq!(fires, 1);
    }

    #[test]
    fn tick_count_rollover_still_rejects_an_over_long_press() {
        let mut m = machine();
        let base = u32::MAX - 10;
        let fires = run(
            &mut m,
            &[
                (L, DOWN, base),
                (L, UP, base.wrapping_add(300)), // over tap_max, across the wrap
                (L, DOWN, base.wrapping_add(340)),
                (L, UP, base.wrapping_add(380)),
            ],
        );
        assert_eq!(fires, 0);
    }

    #[test]
    fn hold_max_binds_when_tap_max_is_raised_above_it() {
        let cfg = DoubleTapConfig {
            tap_max_ms: 900,
            gap_max_ms: 400,
            hold_max_ms: 500,
        };
        let mut m = DoubleTap::new(cfg);
        // 600 ms press: inside the (raised) tap_max but past hold_max.
        assert_eq!(
            run(
                &mut m,
                &[(L, DOWN, 0), (L, UP, 600), (L, DOWN, 650), (L, UP, 690)]
            ),
            0
        );
    }

    #[test]
    fn either_side_matches_both_and_refines_to_the_concrete_side() {
        let either = Observed::Trigger(KeySide::Either);
        let mut m = machine();
        assert_eq!(
            run(
                &mut m,
                &[(either, DOWN, 0), (either, UP, 40), (L, DOWN, 100), (L, UP, 140)]
            ),
            1,
            "an unresolved generic modifier must still pair with a concrete side"
        );
    }

    #[test]
    fn a_stray_key_up_with_no_matching_down_does_not_arm() {
        let mut m = machine();
        // We started listening while Shift was already held.
        assert_eq!(
            run(&mut m, &[(L, UP, 10), (L, DOWN, 50), (L, UP, 90)]),
            0
        );
    }

    #[test]
    fn three_taps_fire_once_not_twice() {
        let mut m = machine();
        let fires = run(
            &mut m,
            &[
                (L, DOWN, 0),
                (L, UP, 40),
                (L, DOWN, 100),
                (L, UP, 140), // fires here, machine resets
                (L, DOWN, 200),
                (L, UP, 240), // only the first tap of a new sequence
            ],
        );
        assert_eq!(fires, 1);
    }

    #[test]
    fn classify_maps_both_shift_sides_directly() {
        let t = TriggerKey::SHIFT;
        assert_eq!(
            t.classify(VK_LSHIFT, || unreachable!("no resolution needed")),
            Classified::Trigger {
                side: KeySide::Left,
                generic_unresolved: false
            }
        );
        assert_eq!(
            t.classify(VK_RSHIFT, || unreachable!("no resolution needed")),
            Classified::Trigger {
                side: KeySide::Right,
                generic_unresolved: false
            }
        );
        assert_eq!(t.classify(0x41, || None), Classified::Other);
    }

    #[test]
    fn classify_resolves_a_generic_shift_through_the_scan_code() {
        let t = TriggerKey::SHIFT;
        assert_eq!(
            t.classify(VK_SHIFT, || Some(VK_RSHIFT)),
            Classified::Trigger {
                side: KeySide::Right,
                generic_unresolved: false
            }
        );
        // Unresolvable: match either side, and say so.
        assert_eq!(
            t.classify(VK_SHIFT, || None),
            Classified::Trigger {
                side: KeySide::Either,
                generic_unresolved: true
            }
        );
    }

    #[test]
    fn classify_treats_a_sideless_trigger_as_either() {
        let t = TriggerKey::plain(0x41); // 'A'
        assert_eq!(
            t.classify(0x41, || unreachable!("a sideless trigger never resolves")),
            Classified::Trigger {
                side: KeySide::Either,
                generic_unresolved: false
            }
        );
        assert_eq!(t.classify(VK_LSHIFT, || None), Classified::Other);
    }

    #[test]
    fn from_vk_folds_every_shift_code_onto_the_shift_spec() {
        assert_eq!(TriggerKey::from_vk(VK_SHIFT), TriggerKey::SHIFT);
        assert_eq!(TriggerKey::from_vk(VK_LSHIFT), TriggerKey::SHIFT);
        assert_eq!(TriggerKey::from_vk(VK_RSHIFT), TriggerKey::SHIFT);
        assert_eq!(TriggerKey::from_vk(VK_LCONTROL), TriggerKey::CONTROL);
        assert_eq!(TriggerKey::from_vk(0x41), TriggerKey::plain(0x41));
    }
}
