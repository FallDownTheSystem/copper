//! Tray icon, menu and click handling.
//!
//! Separate from `panel.rs` because it is a different surface with its own
//! lifetime. From this build onwards it is the recovery path when the panel
//! cannot otherwise be reached, since the window starts hidden — which is why it
//! is built before anything that can fail, and why nothing in here is allowed to
//! take a failure elsewhere as a reason not to appear.

use tauri::{
	menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
	tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
	AppHandle, Emitter, Manager,
};

use crate::{autostart, diagnostics, panel};

const MENU_SHOW: &str = "show";
const MENU_SETTINGS: &str = "settings";
const MENU_AUTOSTART: &str = "autostart";
const MENU_QUIT: &str = "quit";

const TOOLTIP: &str = "Copper";
const TOOLTIP_NO_SUMMON: &str = "Copper: summon shortcut unavailable";

/// The tray's Settings item reveals the panel *and* puts it on the settings view.
pub const OPEN_SETTINGS: &str = "open-settings";

/// The two handles this task has to reach after build time.
///
/// **Not** about keeping the icon alive. Tauri retains the `TrayIcon` in its own
/// internal state — `remove_tray_by_id` is documented as removing it "from
/// tauri's internal state", `tray_by_id` looks one up, and Tauri's own guide
/// discards `build()`'s return — so task-002 discarding it was never a bug. What
/// is genuinely needed is narrower: `set_tooltip` for the registration-failure
/// path, and `set_checked` for the autostart item.
struct TrayState {
	icon: TrayIcon,
	autostart: CheckMenuItem<tauri::Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
	let show = MenuItem::with_id(app, MENU_SHOW, "Show Copper", true, None::<&str>)?;
	let settings = MenuItem::with_id(app, MENU_SETTINGS, "Settings", true, None::<&str>)?;
	let launch = CheckMenuItem::with_id(
		app,
		MENU_AUTOSTART,
		"Launch Copper at login",
		true,
		autostart::initial_state(app),
		None::<&str>,
	)?;
	let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;

	let menu = Menu::with_items(
		app,
		&[
			&show,
			&settings,
			&PredefinedMenuItem::separator(app)?,
			&launch,
			&PredefinedMenuItem::separator(app)?,
			&quit,
		],
	)?;

	let icon = TrayIconBuilder::new()
		// Reusing the window icon avoids needing the `image-png` feature.
		.icon(
			app.default_window_icon()
				.cloned()
				.ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?,
		)
		.tooltip(TOOLTIP)
		.menu(&menu)
		// Left-click toggles the panel; it must not also open the menu.
		.show_menu_on_left_click(false)
		.on_menu_event(|app, event| match event.id().as_ref() {
			MENU_SHOW => panel::reveal_or_log(app),
			MENU_SETTINGS => open_settings(app),
			// Off the main thread: `set_autostart_enabled` writes the registry and
			// then re-reads it, and this callback runs on the thread the message loop
			// lives on.
			MENU_AUTOSTART => {
				let app = app.clone();
				std::thread::spawn(move || autostart::toggle_from_tray(&app));
			}
			// One quit sequence, shared with the panel menu's entry — the flush
			// ordering it needs is recorded on `panel::quit` itself.
			MENU_QUIT => panel::quit(app),
			_ => {}
		})
		.on_tray_icon_event(|tray, event| {
			// Match on the button *and* the button state. TrayIconEvent::Click
			// fires for both the down stroke and the up stroke, so matching the
			// button alone toggles twice per click — which reads as the panel
			// never appearing at all.
			if let TrayIconEvent::Click {
				button: MouseButton::Left,
				button_state: MouseButtonState::Up,
				..
			} = event
			{
				panel::toggle_or_log(tray.app_handle());
			}
		})
		.build(app)?;

	app.manage(TrayState {
		icon,
		autostart: launch,
	});

	Ok(())
}

/// Reveals the panel *and* switches it to settings, in one action.
///
/// The event is safe to emit here — unlike one sent from `setup()` — because by
/// the time a tray menu can be clicked the webview has loaded and its listener
/// is registered.
fn open_settings(app: &AppHandle) {
	panel::reveal_or_log(app);
	if let Err(err) = app.emit(OPEN_SETTINGS, ()) {
		diagnostics::log_error(&format!("[copper] tray: could not open settings: {err}"));
	}
}

/// Reflects the live autostart state on the checkmark.
///
/// Called from both directions — the tray's own toggle and the settings view's —
/// so the two can never drift.
pub fn report_autostart(app: &AppHandle, enabled: bool) {
	let Some(state) = app.try_state::<TrayState>() else {
		return;
	};
	if let Err(err) = state.autostart.set_checked(enabled) {
		diagnostics::log_error(&format!(
			"[copper] tray: could not update the autostart checkmark: {err}"
		));
	}
}

/// Says in the tooltip whether the summon chord is actually live.
///
/// The quiet half of the registration-failure presentation: the tray is where a
/// user whose shortcut failed goes looking, so it is where the reason belongs.
/// The settings view carries the rest, pulled rather than pushed.
pub fn report_summon(app: &AppHandle, registered: bool) {
	let Some(state) = app.try_state::<TrayState>() else {
		return;
	};
	let tooltip = if registered { TOOLTIP } else { TOOLTIP_NO_SUMMON };
	if let Err(err) = state.icon.set_tooltip(Some(tooltip)) {
		diagnostics::log_error(&format!("[copper] tray: could not update the tooltip: {err}"));
	}
}

/// Takes the icon out of the notification area, now.
///
/// Exists for the one exit that runs no destructors: an update replaces this
/// process through `std::process::exit(0)` from inside the updater plugin, so
/// nothing gets dropped, no window gets destroyed, and Windows has no event that
/// tells it to reap the icon. `Shell_NotifyIcon` is not window-message based, so
/// this is safe from the async runtime thread the install runs on rather than
/// only from the main thread.
///
/// Silent when there is no tray — a build where `tray::build` failed reveals the
/// panel instead and has nothing to hide.
pub fn hide(app: &AppHandle) {
	let Some(state) = app.try_state::<TrayState>() else {
		return;
	};
	if let Err(err) = state.icon.set_visible(false) {
		diagnostics::log_error(&format!("[copper] tray: could not hide the icon: {err}"));
	}
}
