/// Public because `tests/store_fs.rs` drives `ingest` directly: the attachment
/// assertions are about what ingest and the document do together, and a
/// hand-built `Attachment` would test neither.
pub mod attachments;
mod autostart;
mod capture;
mod clipboard;
mod commands;
mod diagnostics;
/// Public because `end_all` is the entry point Phase 6 calls on a space switch,
/// and it must stay the only way another module ends every handoff at once.
pub mod editor;
/// Public because `tests/markdown.rs` drives `render` directly: which notes each
/// selection resolves to is the whole of this module, and a `State<SharedStore>`
/// cannot be built outside a running app.
pub mod markdown;
mod panel;
/// Public because `LinkPreview` crosses the IPC boundary and `tests/commands.rs`
/// asserts the shape it arrives in, the same reason `store` is.
pub mod previews;
/// Public because `ShareConfig`, `ShareSendOutcome` and their siblings cross the
/// IPC boundary, and `tests/commands.rs` asserts the shapes they arrive in — the
/// same reason `store` and `previews` are public.
pub mod share;
mod shortcuts;
pub mod spaces;
pub mod store;
mod theme;
mod tray;
mod updater;
mod win32;

pub use diagnostics::install_panic_dialog;

use std::sync::{Arc, Mutex};

/// The one file in the tree that needs an alias rather than a plain import.
///
/// `crate::store` still exists here — it is the module holding the command
/// wrappers and `events::AppSink`, both of which `setup()` below reaches by that
/// name — so the store *core* cannot also be called `store` in this scope. Every
/// other module in the crate uses one or the other and can simply rebind the
/// bare name; this one uses both.
use copper_core::store as core_store;

use serde::ser::{Serialize, SerializeStruct, Serializer};
use tauri::{DeviceEventFilter, Manager, RunEvent, WindowEvent};

/// What the shell layer returns to the frontend.
///
/// Declared here rather than inside one of the modules that produce it:
/// `shortcuts`, `autostart`, `theme` and `panel` all return it and none of them
/// owns it, and there is no `shell` module for it to live in — task-002 already
/// established `panel.rs` and `tray.rs` as top-level siblings, so this task grew
/// those rather than introducing a parallel tree.
///
/// Serialised flat as `{ kind, message }`, exactly like task-003's `StoreError`,
/// so the frontend branches on a discriminant. Rust owns the wording: every
/// variant carries a sentence written for a person, matching what
/// `CaptureFailure::message` and `StoreError` already do, and the frontend
/// renders it rather than keeping a second copy of the copy in TypeScript.
///
/// **There is no `Conflict` variant, and that is not an omission.**
/// `tauri-plugin-global-shortcut` flattens `global_hotkey::Error` into
/// `Error::GlobalHotkey(String)`, so "another application holds this chord"
/// cannot be told apart from any other registration failure without matching on
/// an error message. Promising a discriminant the API cannot supply would be a
/// lie the UI then repeats to the user, so every post-validation registration
/// failure is `RegistrationFailed` and its wording hedges on the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellError {
	/// The string is not a chord this app can parse.
	InvalidChord(String),
	/// Modifiers with no main key. The parser rejects this too, but a dedicated
	/// message reads better than "invalid hotkey format".
	ModifierOnly(String),
	/// Windows will never deliver this combination, or Copper has already claimed
	/// it for its other binding.
	Reserved(String),
	/// The OS refused the registration. Cause unknown by construction — see above.
	RegistrationFailed(String),
	/// The OS-side change worked and writing it down did not.
	Persist(String),
	/// A recording token that is not the live one.
	StaleToken(String),
	/// An argument outside the accepted set — a `theme` that is not one of
	/// `system` / `light` / `dark`.
	Invalid(String),
}

impl ShellError {
	/// The stable, lowercase-kebab discriminant the frontend branches on, in the
	/// same spelling task-003's `StoreError::kind` uses.
	pub fn kind(&self) -> &'static str {
		match self {
			Self::InvalidChord(_) => "invalid-chord",
			Self::ModifierOnly(_) => "modifier-only",
			Self::Reserved(_) => "reserved",
			Self::RegistrationFailed(_) => "registration-failed",
			Self::Persist(_) => "persist",
			Self::StaleToken(_) => "stale-token",
			Self::Invalid(_) => "invalid",
		}
	}

	pub fn message(&self) -> &str {
		match self {
			Self::InvalidChord(message)
			| Self::ModifierOnly(message)
			| Self::Reserved(message)
			| Self::RegistrationFailed(message)
			| Self::Persist(message)
			| Self::StaleToken(message)
			| Self::Invalid(message) => message,
		}
	}
}

impl std::fmt::Display for ShellError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.message())
	}
}

impl std::error::Error for ShellError {}

/// Hand-written for the same reason `StoreError`'s is: the derive would emit an
/// externally tagged enum, and the contract is one flat shape for every variant.
impl Serialize for ShellError {
	fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
		let mut state = serializer.serialize_struct("ShellError", 2)?;
		state.serialize_field("kind", self.kind())?;
		state.serialize_field("message", self.message())?;
		state.end()
	}
}

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
		// Rust-side only, like the dialog plugin above: `updater.rs` owns the whole
		// flow behind three application commands, so no `updater:*` permission is
		// granted and the plugin's own four commands stay unreachable from the
		// WebView — `removeUnusedCommands` then prunes them out of the binary.
		// Registering it is still required: `updater_builder()` resolves the
		// endpoint and the public key out of the state this installs.
		.plugin(tauri_plugin_updater::Builder::new().build())
		// Registered on the builder rather than inside setup() so it is in place
		// before any event can arrive. This is what actually holds the "created
		// hidden and never destroyed" invariant the whole shell rests on: nothing
		// otherwise stops Alt+F4 from destroying the panel while it is revealed and
		// focused, leaving a live process whose tray icon reveals nothing.
		// `closable: false` in the window config is belt-and-braces. Quitting stays
		// an explicit menu action — the tray's Quit or the panel menu's.
		.on_window_event(|window, event| {
			if window.label() != panel::PANEL_LABEL {
				return;
			}
			match event {
				WindowEvent::CloseRequested { api, .. } => {
					api.prevent_close();
					panel::hide_or_log(window);
				}
				// Fires for a `data-tauri-drag-region` drag as well as for a
				// programmatic `set_position`; the write behind it is debounced and the
				// store's change-guard makes the programmatic case free.
				//
				// There is deliberately no `WindowEvent::Resized` arm beside it. The
				// stored size is the panel's *default*, so a manual drag-resize is
				// session-only by design — see the ruling on `panel::on_moved`.
				WindowEvent::Moved(position) => panel::on_moved(window.app_handle(), *position),
				// Only while the window is following the system theme, which is exactly
				// the case that needs re-tinting.
				WindowEvent::ThemeChanged(_) => {
					if let Some(panel) = window.get_webview_window(panel::PANEL_LABEL) {
						theme::on_system_theme_changed(&panel);
					}
				}
				// The belt to the reveal paths' braces — see `install_drop_targets`:
				// focus arrives through the event loop after the show it followed, so a
				// WebView2 child window still being built during the reveal's own pass
				// gets its drop target here. Never fires mid-drag (a drag does not
				// focus the panel), and re-registering is a handful of Win32 calls.
				WindowEvent::Focused(true) => {
					if let Some(panel) = window.get_webview_window(panel::PANEL_LABEL) {
						panel::install_drop_targets(&panel);
					}
				}
				// The panel is never destroyed in normal use — `CloseRequested` above
				// sees to that — but if it ever is, a recording session must not take
				// the summon chord with it. Off the main thread, per the note in
				// `shortcuts`: this callback runs on the thread that registration waits
				// on.
				WindowEvent::Destroyed => shortcuts::cancel_recording_off_thread(window.app_handle()),
				_ => {}
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

			// The tray comes before everything that can fail, because it is the
			// recovery path: an app that starts hidden with no tray has no way back
			// in. If the tray itself will not build, the panel is revealed instead —
			// otherwise the process runs with neither a window nor a recovery surface.
			panel::apply_effects(&window, None)?;
			if let Err(err) = tray::build(app.handle()) {
				diagnostics::log_error(&format!(
					"[copper] tray: could not be built ({err}); revealing the panel so the app is \
					 still reachable"
				));
				panel::reveal_or_log(app.handle());
			}

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
			let shared: core_store::SharedStore =
				Arc::new(Mutex::new(core_store::bootstrap_store(&config_dir, sink)?));
			app.manage(Arc::clone(&shared));

			// A watch that will not register leaves the space open and fully
			// writable; it only means external edits go unnoticed. get_status
			// reports it as watching: false.
			//
			// This also reconciles anything written to the space file between
			// bootstrap reading it and the watch going live. The events are logged
			// rather than emitted — nothing is listening — and the state they
			// describe is already in the store for the mount-time pull to read.
			for event in core_store::attach_watcher(&shared) {
				diagnostics::log_error(&format!("[copper] store startup: {event:?}"));
			}

			// Everything below this point degrades rather than propagating, and that
			// is a deliberate departure from the `?` above. task-002's reasoning was
			// about panics; returning `Err` from `setup()` is just as fatal, because
			// Tauri turns it into one. A failed shortcut registration, an unreadable
			// monitor list or a backdrop that will not re-tint must each cost the user
			// that one feature, not the whole app — which for something that starts
			// hidden would be indistinguishable from a silent successful launch.
			//
			// Position before the window is ever shown, and validated against the
			// monitors actually attached: Tauri does not clamp an out-of-bounds
			// `set_position`, so a display that was unplugged since the last run would
			// otherwise put the panel where it cannot be reached.
			app.manage(panel::PositionState::default());
			// Before `restore_position`, and that order is load-bearing rather than
			// tidy: the placement below computes the panel's corner from how big the
			// panel is, so a size applied afterwards would leave a window sized for one
			// rectangle sitting at a corner computed for another — visibly, as a right
			// edge hanging off the work area on any install that chose a wider panel.
			panel::install_panel_size(app.handle(), &window);
			panel::restore_position(&window, core_store::lock(&shared).settings().panel_position);
			// Before `theme::install` and not beside the pin below, because it changes
			// what that call paints rather than being a second thing to paint: it
			// records the material, and `theme::install` is the one call that applies
			// the backdrop. The other order would show Mica for a frame on a panel the
			// user asked to be translucent.
			panel::install_translucency(app.handle());
			theme::install(app.handle(), &window);
			// Beside the theme rather than in the window config: `tauri.conf.json`
			// declares the band the window is *born* in, and this is the stored
			// preference that may contradict it. Before the first reveal, so an
			// unpinned panel is never briefly topmost.
			panel::install_always_on_top(app.handle(), &window);

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
			// After the registry it sweeps. It ends nothing: an editor's exit is not a
			// signal Copper can observe — `code` hands the file to a running instance
			// and returns immediately — so a finished session is recognised by the file
			// going quiet, and all that recognition does is clear the card. The handoff
			// itself lives until something asks for it to end. See
			// `editor::has_gone_idle`.
			editor::start_idle_sweeper(app.handle());

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
			// Before `start_capture`, so the first double-tap after launch is judged
			// against the user's binding rather than the compiled-in default.
			shortcuts::prepare_capture(app.handle());
			app.manage(capture::CaptureState(Mutex::new(capture::start_capture(
				app.handle(),
			)?)));
			capture::arm_when_frontend_ready(app.handle());

			// Inert until the settings view asks for a check, so it has no ordering
			// constraint at all — but it is registered here rather than lazily,
			// because `updater.rs` resolves both through `app.state()`, which panics
			// on a type that was never managed.
			app.manage(updater::PendingUpdate::default());
			app.manage(updater::UpdateGate::default());

			// The link-preview cache's only maintenance, and startup is the only
			// place it can run: a sweep during a session would delete an entry a card
			// on screen is about to ask for. It sweeps whatever the toggle says,
			// because expiring a stale entry is not a disclosure and an install that
			// turned previews off long ago should not still be holding the directory.
			// Detached, like the attachment sweep: nothing waits for it.
			previews::commands::start_prune(app.handle());

			// Task-026's poll thread. Pointed at the same directory `settings.json`
			// lives in — `share.json` is a second app-private state file, kept out of
			// `Settings` because every field of that struct is serialised to the
			// WebView by `get_settings` and a pairing secret must not be.
			//
			// It costs nothing while the feature is off: the thread waits on its
			// `Condvar` with **no timeout** until something calls `share::wake()`.
			// Degrading rather than propagating, like everything else below the store
			// bootstrap — a share that will not start must cost the user that one
			// feature, not the app.
			share::init(&config_dir);
			share::start_poller(app.handle());

			// Last, because it is the only step expected to fail in ordinary use:
			// another application may already hold the chord. A failure here leaves
			// the app running, says so in the tray tooltip, and waits to be asked
			// about through `get_shortcut_state` — the settings view pulls it rather
			// than having had to listen during a startup that predates the webview.
			shortcuts::install(app.handle());

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
			teardown(handle);
		}
	});
}

/// Everything that has to happen before this process stops existing.
///
/// One routine with two callers, and they are not variations on each other: the
/// tray's Quit reaches it through `RunEvent::Exit`, and an update reaches it
/// through the updater's `on_before_exit` hook, moments before the plugin calls
/// `std::process::exit(0)` from inside the install. The second caller is why this
/// is shared rather than inlined — an abrupt exit runs no destructors at all, so
/// anything left only to `Drop` is simply lost on the update path.
///
/// **Runs exactly once, and the second caller waits for the first.** Per-step
/// idempotence is not enough on its own, because the two callers are on
/// different threads: the updater's hook runs on the async runtime while a Quit
/// from the tray runs on the main thread, and a download lasting minutes makes
/// that interleaving real rather than theoretical. Idempotent steps would each
/// individually survive it, but the *ordering between* them would not — the
/// second caller could reach `cleanup_before_exit()` and let the plugin exit the
/// process while the first was still inside the position flush. `Once` blocks
/// the loser until the winner is done, which is the property actually needed.
/// The steps stay individually idempotent anyway; that is worth preserving when
/// adding to this list rather than relying on this guard alone.
///
/// **Not a crash hook.** None of this runs on a panic-abort, a Task Manager kill,
/// or an uninstall started from Windows. The startup `editor::scavenge` is what
/// covers those.
fn teardown(handle: &tauri::AppHandle) {
	static DONE: std::sync::Once = std::sync::Once::new();
	DONE.call_once(|| teardown_steps(handle));
}

/// Set the moment teardown begins, and never cleared.
///
/// Read by background work that would otherwise start something the exit path is
/// in the middle of undoing — the capture watchdog's fallback re-registration is
/// the case it was added for, since that takes the shortcut registry lock and
/// `shortcuts::shutdown` gives that lock up rather than blocking exit for it.
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the process has begun shutting down.
///
/// Advisory, not a barrier: a caller that reads `false` here and is descheduled
/// can still find teardown underway by the time it acts. It shortens that window
/// rather than closing it, which is why the things that read it also cost nothing
/// when they lose the race — the worst case is `shortcuts::shutdown` skipping
/// retirements that Windows reclaims at process exit anyway.
pub fn shutting_down() -> bool {
	SHUTTING_DOWN.load(std::sync::atomic::Ordering::Relaxed)
}

fn teardown_steps(handle: &tauri::AppHandle) {
	// First, before any step that background work might race: everything below
	// undoes state, and a thread starting fresh work against it is the one thing
	// this ordering cannot absorb.
	SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::Relaxed);

	// Before anything slow, because it is the one step the user can see. Windows
	// normally reaps a notification icon when its owner window is destroyed, but
	// `std::process::exit(0)` destroys nothing — so on the update path the icon
	// would linger until the shell next swept it, which reads as an app that
	// half-quit.
	tray::hide(handle);

	// First of the state-preserving steps, because it is the only one that can
	// still be lost: a drag that ended less than the debounce ago has not been
	// written yet.
	panel::flush_position(handle);
	shortcuts::shutdown(handle);
	capture::shutdown(handle);
	// Joined, not merely signalled, and before the two steps that walk the same
	// registry. The sweep only clears cards, so an overlap would be survivable —
	// but "teardown has the registry to itself" is worth being true rather than
	// nearly true, and `shutting_down()` above is explicitly advisory.
	editor::stop_idle_sweeper();
	// Joined for the same reason as the sweeper, and with a sharper one of its
	// own: this thread writes notes into the open space, and teardown must not run
	// beside a delivery in flight. The drain checks the stop flag between
	// messages, so the wait is bounded by one request rather than by a whole
	// drain.
	share::stop_poller();
	// Before `scavenge`: each live handoff applies or refuses whatever is on disk,
	// so exiting is not a way to silently discard unsaved editor work. The at-exit
	// form skips the mid-write read retry, which would otherwise cost a debounce
	// window per handoff on the way out.
	//
	// "Sweep" is deliberately not the word here. `scavenge` collects the editor's
	// own temp directories; task-011's *attachment* sweep runs at space close and
	// at startup only, and never at exit — a session that ends is a session whose
	// undo stack ends with it, so there is nothing left to protect and nothing that
	// needs collecting before the next launch does it.
	editor::end_all_at_exit(handle);
	editor::scavenge();
}
