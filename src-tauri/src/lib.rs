mod diagnostics;
mod panel;
mod tray;

pub use diagnostics::install_panic_dialog;

use tauri::{Manager, WindowEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
	tauri::Builder::default()
		// single-instance must be the literal first plugin on the builder, before
		// every other plugin and before .setup().
		//
		// Tauri's docs say to register it first; Tauri's own boilerplate registers
		// it lazily inside setup(). Whether windows declared in tauri.conf.json are
		// created before setup() runs is undocumented, so rather than resolve that
		// ambiguity we sidestep it: registering first, combined with the window's
		// "visible": false, means neither ordering can produce a visible window in
		// the losing process.
		//
		// On Windows the plugin takes a named mutex; a second process hands its
		// argv and cwd to the first over WM_COPYDATA and exits before it ever runs
		// setup() or creates a window. The callback below runs on the main thread,
		// where the plugin's hidden message window lives, so it may touch window
		// handles directly with no marshalling.
		//
		// argv carrying a .copper file path is task-007's problem, not this one.
		.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
			panel::reveal_or_log(app);
		}))
		.plugin(tauri_plugin_global_shortcut::Builder::new().build())
		.plugin(tauri_plugin_autostart::init(
			tauri_plugin_autostart::MacosLauncher::LaunchAgent,
			None,
		))
		.plugin(tauri_plugin_opener::init())
		// Registered on the builder rather than inside setup() so it is in place
		// before any event can arrive. This is what actually holds the "created
		// hidden and never destroyed" invariant the whole shell rests on: nothing
		// otherwise stops Alt+F4 from destroying the panel while it is revealed and
		// focused, leaving a live process whose tray icon reveals nothing.
		// `closable: false` in the window config is belt-and-braces. Quitting stays
		// the tray menu's job.
		.on_window_event(|window, event| {
			if let WindowEvent::CloseRequested { api, .. } = event {
				if window.label() == panel::PANEL_LABEL {
					api.prevent_close();
					panel::hide_or_log(window);
				}
			}
		})
		// setup() runs once on the main thread before the event loop, and the
		// release profile sets panic = "abort" (see Cargo.toml), so a panic here
		// exits the process instantly with no window and no tray. For an app that
		// deliberately starts hidden that is indistinguishable from a successful
		// silent launch — the worst possible failure to debug. Hence no
		// unwrap()/expect() below: every fallible call propagates with ?.
		//
		// Propagating is necessary but not sufficient. Tauri turns a returned
		// setup error into a panic of its own (tauri-2.11.5/src/app.rs:1424-1425,
		// 1476-1477), so `?` buys a useful message rather than a silent exit only
		// because install_panic_dialog() is in place to surface it.
		.setup(|app| {
			let window = app
				.get_webview_window(panel::PANEL_LABEL)
				.ok_or_else(|| format!("panel window '{}' not found", panel::PANEL_LABEL))?;

			panel::apply_effects(&window)?;
			tray::build(app.handle())?;

			// The window is not shown here. It stays hidden until the tray reveals it.
			Ok(())
		})
		.run(tauri::generate_context!())
		.expect("error while running tauri application");
}
