mod capture;
mod clipboard;
mod commands;
mod diagnostics;
/// Public because `end_all` is the entry point Phase 6 calls on a space switch,
/// and it must stay the only way another module ends every handoff at once.
pub mod editor;
mod panel;
pub mod spaces;
pub mod store;
mod tray;
mod win32;

pub use diagnostics::install_panic_dialog;

use std::sync::{Arc, Mutex};

use tauri::{DeviceEventFilter, Manager, RunEvent, WindowEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
	let app = tauri::Builder::default()
		// tauri-apps/tauri#13919 — a WH_KEYBOARD_LL hook failing to see system
		// keys while a Tauri window is focused — was closed by the reporter
		// changing this setting rather than by a Tauri fix. Tauri's default lets
		// tao register for raw keyboard input while the window is focused, and
		// that registration is the interference. `Always` is the only value that
		// reduces it. The acceptance test is empirical: the double-tap must fire
		// while the Copper panel itself has focus.
		.device_event_filter(DeviceEventFilter::Always)
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
		// The callback hands over and returns. Anything slow in here stalls the
		// message loop it is running inside, and opening a space is filesystem work.
		// It cannot reach managed state either — nothing guarantees `app.manage` has
		// run — so the dispatcher is process-wide and queues until the gate opens.
		.plugin(tauri_plugin_single_instance::init(|_app, argv, cwd| {
			spaces::forwarded_launch(&argv, &cwd);
		}))
		.plugin(tauri_plugin_global_shortcut::Builder::new().build())
		// Rust-side only: no npm package and no capability entry, because the
		// plugin's JS API is never used. It hands back a path and every read and
		// write still goes through the store.
		.plugin(tauri_plugin_dialog::init())
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

			// Store startup is two-stage, and the order is not negotiable (spec 7.5).
			// The watcher's callback resolves the store through the shared handle,
			// and that handle does not exist until bootstrap has returned the Store
			// to put inside it — so a watch registered during bootstrap would have a
			// live callback with nothing to resolve.
			//
			// Nothing here emits. The webview has not registered its listeners yet
			// and Tauri events have no replay, so an emit at this point would be an
			// invisible failure rather than merely a useless one (spec 8A.2). First-
			// run space creation, the recents fallback and a corrupt settings file
			// all change store state and nothing else; the panel learns about them
			// from its mount-time get_status pull.
			let config_dir = app.path().app_config_dir()?;
			let sink = Arc::new(store::events::AppSink::new(app.handle().clone()));
			let shared: store::SharedStore =
				Arc::new(Mutex::new(store::bootstrap_store(&config_dir, sink)?));
			app.manage(Arc::clone(&shared));

			// A watch that will not register leaves the space open and fully
			// writable; it only means external edits go unnoticed. get_status
			// reports it as watching: false.
			//
			// This also reconciles anything written to the space file between
			// bootstrap reading it and the watch going live. The events are logged
			// rather than emitted — nothing is listening — and the state they
			// describe is already in the store for the mount-time pull to read.
			for event in store::attach_watcher(&shared) {
				diagnostics::log_error(&format!("[copper] store startup: {event:?}"));
			}

			// Scavenged *before* the registry exists, so no live handoff can have its
			// temp tree deleted out from under it. Startup scavenging is what makes
			// the cleanup promise true after a crash — which runs no exit hook at all
			// — or after an editor held a file open past shutdown.
			//
			// Ahead of the argv open below rather than after it, because that open
			// goes through the same policy wrapper the switcher does, and that wrapper
			// ends live handoffs through a registry it therefore has to be able to
			// resolve. Nothing creates a handoff before the frontend mounts, so the
			// scavenge still runs against an empty registry.
			editor::scavenge();
			app.manage(editor::HandoffRegistry::default());

			// A `.copper` path on the command line — as Explorer passes on a
			// double-click — is opened **here**, synchronously, before capture starts.
			// Merely submitting it to the dispatcher would order nothing: the
			// dispatcher is asynchronous by design, so capture could still start
			// first, and a double-tap in the moments after the launch would append to
			// whatever space was previously active. Silently, since a successful
			// capture produces nothing at all.
			//
			// A failure degrades to "panel open on the previous active space, with an
			// explanation" rather than propagating: a bad argument must not be able to
			// leave the app with no capture destination.
			let cold = spaces::apply_cold_launch(app.handle());

			// Capture starts here — after the store bootstrap and the argv open above,
			// so a trigger always finds the right open space — but it is deliberately
			// **not armed**. Arming waits for the frontend to report that its notice
			// listeners are registered, because Tauri events are not replayed and a
			// failure arriving before then would reveal an empty panel.
			app.manage(capture::CaptureState(Mutex::new(capture::start_capture(
				app.handle(),
			)?)));
			capture::arm_when_frontend_ready(app.handle());

			// The presentation half of the cold launch, queued rather than performed:
			// window operations before the message pump resumes would block this
			// thread until it does. A cold launch with no file queues a request that
			// reveals nothing, so an ordinary start — including autostart — leaves the
			// panel hidden.
			spaces::start_dispatcher(app.handle(), cold);

			// The window is not shown here. It stays hidden until the tray, a launch
			// argument or a second instance asks for it.
			Ok(())
		})
		.invoke_handler(commands::handler())
		.build(tauri::generate_context!())
		.expect("error while running tauri application");

	// Built and run in two steps rather than `Builder::run`, which passes an empty
	// callback: the hook has to come down explicitly. Relying on managed state
	// being dropped is not enough, because Tauri's exit path does not guarantee
	// that drop runs. `shutdown` is idempotent, so the drop that may or may not
	// follow is harmless.
	app.run(|handle, event| {
		if matches!(event, RunEvent::Exit) {
			capture::shutdown(handle);
			// Before the sweep: each live handoff applies or refuses whatever is on
			// disk, so exiting is not a way to silently discard unsaved editor work.
			// The at-exit form skips the mid-write read retry, which would otherwise
			// cost a debounce window per handoff on the way out.
			editor::end_all_at_exit(handle);
			editor::scavenge();
		}
	});
}
