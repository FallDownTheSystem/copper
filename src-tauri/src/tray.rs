//! Tray icon, menu and click handling.
//!
//! Separate from `panel.rs` because it is a different surface with its own
//! lifetime. From this build onwards it is the recovery path when the panel
//! cannot otherwise be reached, since the window starts hidden.

use tauri::{
	menu::{Menu, MenuItem},
	tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
	AppHandle,
};

use crate::panel;

const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";

pub fn build(app: &AppHandle) -> tauri::Result<()> {
	let show = MenuItem::with_id(app, MENU_SHOW, "Show Copper", true, None::<&str>)?;
	let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
	let menu = Menu::with_items(app, &[&show, &quit])?;

	TrayIconBuilder::new()
		// Reusing the window icon avoids needing the `image-png` feature.
		.icon(
			app.default_window_icon()
				.cloned()
				.ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?,
		)
		.tooltip("Copper")
		.menu(&menu)
		// Left-click toggles the panel; it must not also open the menu.
		.show_menu_on_left_click(false)
		.on_menu_event(|app, event| match event.id().as_ref() {
			MENU_SHOW => panel::reveal_or_log(app),
			MENU_QUIT => app.exit(0),
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

	Ok(())
}
