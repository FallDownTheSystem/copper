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
