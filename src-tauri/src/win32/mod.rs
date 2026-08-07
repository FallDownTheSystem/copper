//! Everything that talks to Win32 on the capture path.
//!
//! This directory and `capture/` are the only places in the app besides
//! `panel.rs` that may name an `HWND` (task-005 R1), which is checked by grep
//! rather than trusted: `grep -rn "HWND" src-tauri/src --include=*.rs` must match
//! only under those three.

pub mod clipboard;
pub mod foreground;
pub mod integrity;

/// Written into `KEYBDINPUT.dwExtraInfo` on every event Copper synthesizes, and
/// matched by the keyboard hook so Copper's own `Ctrl+C` cannot feed back into
/// the double-tap state machine.
///
/// The hook filters **this tag only** and never `LLKHF_INJECTED` generally.
/// PowerToys Keyboard Manager and AutoHotkey deliver genuine user intent as
/// injected input, so rejecting all injected events would silently break the
/// trigger for everyone using a remapper — and silently is the operative word,
/// since there is no error to report.
pub const EXTRA_INFO_SIGNATURE: usize = 0x0C0F_FEE0;

/// Written into `KEYBDINPUT.dwExtraInfo` on the watchdog's liveness probe, and
/// matched by the keyboard hook so the probe proves the callback is still being
/// called without reaching anything else.
///
/// A second tag rather than a reuse of [`EXTRA_INFO_SIGNATURE`], because the two
/// are handled differently in a way that matters: Copper's own `Ctrl+C` is passed
/// on to the foreground application — it is *for* the foreground application —
/// while the probe is swallowed, since nothing but Copper has any business seeing
/// it. Sharing one tag would mean either delivering stray keystrokes or
/// swallowing the copy that a capture depends on.
pub const PROBE_SIGNATURE: usize = 0x0C0F_FEE1;

/// Virtual-key codes, as the low-level hook reports them in `vkCode` and as
/// `GetAsyncKeyState` takes them.
///
/// Here for the same reason [`EXTRA_INFO_SIGNATURE`] is here: the hook's trigger
/// families and the clipboard fallback's modifier wait list are the same numbers
/// serving two different purposes, and each module had written them out for
/// itself — `VK_CONTROL` twice verbatim, and the wait list as eight bare hex
/// literals with the name in a trailing comment.
///
/// The values come from the `windows` bindings rather than from `winuser.h` by
/// hand. `u32` rather than `VIRTUAL_KEY` because that is what
/// `KBDLLHOOKSTRUCT.vkCode` carries and what `TriggerKey` compares against, so
/// the conversion happens once here instead of at every use site. `as` rather
/// than `u32::from` because `From` is not usable in a const initialiser.
pub mod keys {
	use windows::Win32::UI::Input::KeyboardAndMouse as keyboard;

	pub const VK_C: u32 = keyboard::VK_C.0 as u32;
	pub const VK_SHIFT: u32 = keyboard::VK_SHIFT.0 as u32;
	pub const VK_CONTROL: u32 = keyboard::VK_CONTROL.0 as u32;
	pub const VK_MENU: u32 = keyboard::VK_MENU.0 as u32;
	pub const VK_LSHIFT: u32 = keyboard::VK_LSHIFT.0 as u32;
	pub const VK_RSHIFT: u32 = keyboard::VK_RSHIFT.0 as u32;
	pub const VK_LCONTROL: u32 = keyboard::VK_LCONTROL.0 as u32;
	pub const VK_RCONTROL: u32 = keyboard::VK_RCONTROL.0 as u32;
	pub const VK_LMENU: u32 = keyboard::VK_LMENU.0 as u32;
	pub const VK_RMENU: u32 = keyboard::VK_RMENU.0 as u32;
	pub const VK_LWIN: u32 = keyboard::VK_LWIN.0 as u32;
	pub const VK_RWIN: u32 = keyboard::VK_RWIN.0 as u32;
	/// The watchdog's probe key. Inert for the same reason `shortcuts` lets the
	/// high function keys be bound bare: no keyboard emits one by accident and
	/// nothing else on the machine is listening for it.
	pub const VK_F24: u32 = keyboard::VK_F24.0 as u32;
}
