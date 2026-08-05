//! The panel window as a unit: its label, its native backdrop and corner
//! rounding, and the reveal/hide pair every future call site must go through.
//!
//! This is the only module in the app that handles an `HWND`.

use crate::diagnostics;
use tauri::{Manager, WebviewWindow};
use windows::Win32::Graphics::Dwm::{
	DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
	DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::UI::WindowsAndMessaging::{
	SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
	SW_SHOWNOACTIVATE,
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
		Ok(()) => diagnostics::log("[copper] backdrop: Mica applied"),
		Err(mica_err) => {
			diagnostics::log(&format!(
				"[copper] backdrop: Mica failed ({mica_err}), falling back to Acrylic"
			));
			window_vibrancy::apply_acrylic(window, None)?;
			diagnostics::log("[copper] backdrop: Acrylic applied");
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

/// Reveals the panel **without** giving it focus, for the capture failure
/// notice.
///
/// Tauri's `WebviewWindow::show()` is not used here and neither is `reveal()`
/// above: a capture must never move focus, so the user keeps typing into
/// whatever they were typing into while the notice is on screen.
///
/// `HWND_TOPMOST` rather than `HWND_TOP` because the panel is configured
/// `alwaysOnTop: true`, and `HWND_TOP` would move it to the top of the
/// *non-topmost* band — dropping it out of the band it is supposed to live in.
///
/// Must be called on the main thread, like every other window operation.
///
/// `Box<dyn Error>` rather than `tauri::Result` for the same reason
/// [`apply_effects`] uses it: `windows::core::Error` has no `From` impl into
/// `tauri::Error`, so `?` would not compile.
pub fn reveal_without_activating(
	window: &WebviewWindow,
) -> Result<(), Box<dyn std::error::Error>> {
	let hwnd = window.hwnd()?;
	// SAFETY: `hwnd` is a live window handle owned by Tauri for the lifetime of
	// this call, and both calls are made on the thread that owns the window.
	unsafe {
		let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
		SetWindowPos(
			hwnd,
			Some(HWND_TOPMOST),
			0,
			0,
			0,
			0,
			SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
		)?;
	}
	Ok(())
}

/// Hides the panel. The window is never destroyed, only hidden.
pub fn hide(window: &WebviewWindow) -> tauri::Result<()> {
	window.hide()
}

/// Looks the panel up and runs `op` against it, logging rather than returning
/// any failure. `verb` names the action in that log line, e.g. "reveal".
///
/// For the call sites that return `()` and so cannot use `?` — the tray handler
/// and the single-instance callback. Swallowing the error there with `let _ =`
/// would make a dead tray icon indistinguishable from a working one.
fn with_panel<M: Manager<tauri::Wry>>(
	app: &M,
	verb: &str,
	op: impl FnOnce(&WebviewWindow) -> tauri::Result<()>,
) {
	match app.get_webview_window(PANEL_LABEL) {
		Some(window) => {
			if let Err(err) = op(&window) {
				diagnostics::log_error(&format!("[copper] failed to {verb} the panel: {err}"));
			}
		}
		None => diagnostics::log_error(&format!("[copper] panel window '{PANEL_LABEL}' not found")),
	}
}

/// Reveals the panel, or logs why it could not be reached.
pub fn reveal_or_log<M: Manager<tauri::Wry>>(app: &M) {
	crate::capture::panel_revealed_by_user(app);
	with_panel(app, "reveal", reveal);
}

/// Hides the panel, or logs why it could not be reached.
pub fn hide_or_log<M: Manager<tauri::Wry>>(app: &M) {
	with_panel(app, "hide", hide);
}

/// Hides the panel if it is visible and reveals it otherwise, or logs why it
/// could not be reached. This is the tray's left-click behaviour, kept here so
/// that the window lookup stays in the module that owns the window.
pub fn toggle_or_log<M: Manager<tauri::Wry>>(app: &M) {
	crate::capture::panel_revealed_by_user(app);
	with_panel(app, "toggle", |window| {
		if is_visible(window) {
			hide(window)
		} else {
			reveal(window)
		}
	});
}

/// Whether the panel is currently visible, defaulting to `false` if it cannot be
/// determined — a failed query should not leave the tray toggle stuck.
fn is_visible(window: &WebviewWindow) -> bool {
	window.is_visible().unwrap_or(false)
}
