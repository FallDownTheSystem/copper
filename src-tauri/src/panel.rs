//! The panel window as a unit: its label, its native backdrop and corner
//! rounding, and the reveal/hide pair every future call site must go through.
//!
//! This is the only module in the app that handles an `HWND`.

use tauri::{Manager, WebviewWindow};
use windows::Win32::Graphics::Dwm::{
	DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
	DWM_WINDOW_CORNER_PREFERENCE,
};

/// Label of the single panel window, as declared in `tauri.conf.json`.
pub const PANEL_LABEL: &str = "main";

/// Applies the native backdrop and corner rounding.
///
/// The return type is deliberately `Box<dyn Error>` rather than `tauri::Result`:
/// this calls into `window_vibrancy` and `windows`, and neither
/// `window_vibrancy::Error` nor `windows::core::Error` has a `From` impl into
/// `tauri::Error`, so `?` would not compile. `Box<dyn Error>` is what `setup()`'s
/// closure already returns, so it propagates with no adapter.
pub fn apply_effects(window: &WebviewWindow) -> Result<(), Box<dyn std::error::Error>> {
	// Mica first (Windows 11, follows the system theme), Acrylic as the fallback.
	//
	// Which one takes is part of the deliverable, not a debug aid: the two
	// materials cannot be verified the same way. Acrylic samples what is behind
	// the window, so moving a colourful window behind the panel changes it. Mica
	// is derived from the wallpaper and system theme and ignores other windows
	// entirely, so that same test "fails" on a perfectly working Mica panel.
	// Without this log there is no way to tell those two cases apart.
	match window_vibrancy::apply_mica(window, None) {
		Ok(()) => println!("[copper] backdrop: Mica applied"),
		Err(mica_err) => {
			println!("[copper] backdrop: Mica failed ({mica_err}), falling back to Acrylic");
			window_vibrancy::apply_acrylic(window, None)?;
			println!("[copper] backdrop: Acrylic applied");
		}
	}

	// Windows 11 build 22000+. Windows 11 is the project's minimum supported
	// version, so a failure here is a platform defect rather than an app bug and
	// propagating it is correct.
	let hwnd = window.hwnd()?;
	let preference = DWMWCP_ROUND;
	// SAFETY: `hwnd` is a live window handle owned by Tauri for the lifetime of
	// this call, and `preference` is a valid DWM_WINDOW_CORNER_PREFERENCE whose
	// size is passed alongside it.
	unsafe {
		DwmSetWindowAttribute(
			hwnd,
			DWMWA_WINDOW_CORNER_PREFERENCE,
			&preference as *const DWM_WINDOW_CORNER_PREFERENCE as *const _,
			std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
		)?;
	}

	Ok(())
}

/// Reveals the panel.
///
/// The order matters and is not negotiable: `set_focus()` alone will not un-hide
/// a window hidden with `hide()` (tauri-apps/tauri#12936), and the panel spends
/// most of its life hidden. Every reveal path goes through here so that stays
/// true without anyone having to remember it.
pub fn reveal(window: &WebviewWindow) -> tauri::Result<()> {
	window.show()?;
	window.unminimize()?;
	window.set_focus()?;
	Ok(())
}

/// Hides the panel. The window is never destroyed, only hidden.
pub fn hide(window: &WebviewWindow) -> tauri::Result<()> {
	window.hide()
}

/// Reveals the panel, or logs why it could not be reached.
///
/// For the call sites that return `()` and so cannot use `?` — the tray handler
/// and the single-instance callback. Swallowing the error there with `let _ =`
/// would make a dead tray icon indistinguishable from a working one.
pub fn reveal_or_log<M: Manager<tauri::Wry>>(app: &M) {
	match app.get_webview_window(PANEL_LABEL) {
		Some(window) => {
			if let Err(err) = reveal(&window) {
				eprintln!("[copper] failed to reveal the panel: {err}");
			}
		}
		None => eprintln!("[copper] panel window '{PANEL_LABEL}' not found"),
	}
}

/// Hides the panel, or logs why it could not be reached.
pub fn hide_or_log<M: Manager<tauri::Wry>>(app: &M) {
	match app.get_webview_window(PANEL_LABEL) {
		Some(window) => {
			if let Err(err) = hide(&window) {
				eprintln!("[copper] failed to hide the panel: {err}");
			}
		}
		None => eprintln!("[copper] panel window '{PANEL_LABEL}' not found"),
	}
}

/// Whether the panel is currently visible, defaulting to `false` if it cannot be
/// determined — a failed query should not leave the tray toggle stuck.
pub fn is_visible(window: &WebviewWindow) -> bool {
	window.is_visible().unwrap_or(false)
}
