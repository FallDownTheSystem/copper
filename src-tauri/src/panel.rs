//! The panel window as a unit: its label, its native backdrop and corner
//! rounding, and the reveal/hide pair every future call site must go through.
//!
//! One of the three places allowed to handle an `HWND` — this file, `win32/`
//! and `capture/` — per the grep-checked rule in `win32/mod.rs`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use copper_core::store::settings::{
	clamp_panel_size, PanelPosition, Settings, SettingsPatch, PANEL_HEIGHT_BOUNDS,
	PANEL_WIDTH_BOUNDS,
};

use crate::diagnostics;
// `store` stays bound to the app's own module: every use of it below is
// `store::commands::…`, and the command wrappers did not move.
use crate::{store, ShellError};
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewWindow};
use windows::Win32::Graphics::Dwm::{
	DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
	DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::UI::WindowsAndMessaging::{
	SetWindowPos, ShowWindow, HWND_TOP, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
	SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
};

/// Label of the single panel window, as declared in `tauri.conf.json`.
pub const PANEL_LABEL: &str = "main";

/// The panel's declared logical size, as it appears in `tauri.conf.json`. These
/// two, the config and `settings.rs`'s `panelWidth`/`panelHeight` defaults are
/// separate declarations of one fact and **have to be changed together**;
/// `the_constants_match_the_window_in_tauri_conf` reads the file and
/// `the_shipped_default_size_is_the_declared_one` reads the store, so neither can
/// drift silently.
///
/// The size the window is *born* with, not the size it runs at. The live size is
/// [`configured_size`], which the settings hold and the user can change — these
/// only have to be right for the moments before [`install_panel_size`] has run.
const PANEL_WIDTH: f64 = 440.0;
const PANEL_HEIGHT: f64 = 760.0;

/// The height of the draggable header, matching `h-12` in `SettingsView` and
/// `PanelHeader`. It is what the visibility test below is written against: a
/// panel whose header is off-screen cannot be moved and is functionally lost,
/// however much of its body happens to overlap a monitor.
const HEADER_HEIGHT: f64 = 48.0;

/// How far the default placement sits in from the right edge of the work area.
const DEFAULT_INSET: f64 = 24.0;

/// Applies the native backdrop and corner rounding.
///
/// `dark` is the `window-vibrancy` crate's own argument: `None` follows the
/// system, `Some(true)` / `Some(false)` force the tint. task-002 hardcoded `None`
/// here, which meant the backdrop could not be made dark while Windows was light
/// — exactly what an explicit theme preference asks for. This is an extension of
/// that function rather than a second vibrancy path; there is still exactly one
/// place in the app that calls the crate.
///
/// **Which material is applied is the translucency setting's whole native side**,
/// which is why it is read here rather than applied from a second call. Every
/// path that re-tints the backdrop — startup, a theme change, a system appearance
/// change — runs through this function, so a material chosen anywhere else would
/// be silently replaced by the next theme change. Reading the mirror here makes
/// the material survive all three by construction.
///
/// The return type is deliberately `Box<dyn Error>` rather than `tauri::Result`:
/// this calls into `window_vibrancy` and `windows`, and neither
/// `window_vibrancy::Error` nor `windows::core::Error` has a `From` impl into
/// `tauri::Error`, so `?` would not compile. `Box<dyn Error>` is what `setup()`'s
/// closure already returns, so it propagates with no adapter.
pub fn apply_effects(
	window: &WebviewWindow,
	dark: Option<bool>,
) -> Result<(), Box<dyn std::error::Error>> {
	// The material this call is *not* applying is cleared first, and the failure is
	// discarded because "it was never applied" is the ordinary case.
	//
	// On Windows 11 22523 and newer both materials are one DWM attribute, so
	// setting either replaces the other and this clear is a no-op that costs one
	// call. Below that build they are two different mechanisms — Mica is
	// `DWMWA_MICA_EFFECT`, Acrylic is `SetWindowCompositionAttribute` — and
	// neither switches the other off, so without this a 21H2 machine toggling
	// translucency would end up wearing both at once.
	if translucent() {
		let _ = window_vibrancy::clear_mica(window);
		// Acrylic, because Acrylic is the one that blurs. Mica is derived from the
		// wallpaper and ignores what is actually behind the window, so it cannot
		// produce what this setting is for.
		window_vibrancy::apply_acrylic(window, acrylic_tint(dark))?;
		diagnostics::log("[copper] backdrop: Acrylic applied (translucent)");
	} else {
		let _ = window_vibrancy::clear_acrylic(window);
		// Mica first (Windows 11, follows the system theme), Acrylic as the fallback.
		//
		// Which one takes is part of the deliverable, not a debug aid: the two
		// materials cannot be verified the same way. Acrylic samples what is behind
		// the window, so moving a colourful window behind the panel changes it. Mica
		// is derived from the wallpaper and system theme and ignores other windows
		// entirely, so that same test "fails" on a perfectly working Mica panel.
		// Without this log there is no way to tell those two cases apart.
		match window_vibrancy::apply_mica(window, dark) {
			Ok(()) => diagnostics::log("[copper] backdrop: Mica applied"),
			Err(mica_err) => {
				diagnostics::log(&format!(
					"[copper] backdrop: Mica failed ({mica_err}), falling back to Acrylic"
				));
				window_vibrancy::apply_acrylic(window, acrylic_tint(dark))?;
				diagnostics::log("[copper] backdrop: Acrylic applied");
			}
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

/// The two materials take the theme differently, which is why this exists rather
/// than one argument passed through to both.
///
/// `apply_mica` takes `Option<bool>` — the flag `dark` already is. `apply_acrylic`
/// takes an RGBA *tint* instead, so an explicit preference has to be turned into
/// a colour. These two are deliberately near-neutral and only lightly opaque: the
/// CSS `--surface` token carries the panel's actual opacity, and the native tint's
/// only job is to stop the material reading as the opposite appearance behind it.
/// `None` in both cases means "follow the system", which is what `system` wants.
fn acrylic_tint(dark: Option<bool>) -> Option<(u8, u8, u8, u8)> {
	match dark {
		None => None,
		Some(true) => Some((26, 26, 28, 125)),
		Some(false) => Some((250, 250, 250, 125)),
	}
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
	install_drop_targets(window);
	Ok(())
}

/// Puts the panel's own OLE drop targets on the WebView2 window tree, replacing
/// wry's — see `win32::drop_target` for why wry's are dead on arrival for a
/// window born hidden. Called from **every** reveal path — the first reveal is
/// the moment the child windows this must land on come to exist, and WebView2
/// may recreate them later — and from the `Focused(true)` window event, which
/// is the belt to the reveal's braces: focus arrives through the event loop
/// strictly after the show that preceded it, so it catches a child window that
/// was still being built while the reveal's synchronous pass enumerated.
///
/// The listener re-emits each drag as the `tauri://drag-*` event tauri's own
/// pipeline would have produced, with the same payload shape — so the
/// frontend's `onDragDropEvent` subscription cannot tell whose target fired,
/// and keeps working unchanged the day a fixed wry takes the registration back.
pub fn install_drop_targets(window: &WebviewWindow) {
	use crate::win32::drop_target::{self, DropEvent};

	let Ok(hwnd) = window.hwnd() else {
		diagnostics::log_error("[copper] panel: no HWND to register drop targets on");
		return;
	};

	let app = window.app_handle().clone();
	drop_target::reinstall(
		hwnd,
		std::rc::Rc::new(move |event| {
			use tauri::Emitter;

			// The shapes tauri's own emission uses (manager/window.rs
			// DragDropPayload): `paths` present for enter and drop, null for over,
			// and a null payload for leave. The JS API synthesises its
			// `{ type: ... }` wrapper from the event name alone.
			#[derive(serde::Serialize, Clone)]
			struct Payload {
				paths: Option<Vec<std::path::PathBuf>>,
				position: Position,
			}
			#[derive(serde::Serialize, Clone, Copy)]
			struct Position {
				x: i32,
				y: i32,
			}

			let target = tauri::EventTarget::labeled(PANEL_LABEL);
			let sent = match event {
				DropEvent::Enter { paths, position } => app.emit_to(
					target,
					"tauri://drag-enter",
					Payload {
						paths: Some(paths),
						position: Position { x: position.0, y: position.1 },
					},
				),
				DropEvent::Over { position } => app.emit_to(
					target,
					"tauri://drag-over",
					Payload {
						paths: None,
						position: Position { x: position.0, y: position.1 },
					},
				),
				DropEvent::Drop { paths, position } => app.emit_to(
					target,
					"tauri://drag-drop",
					Payload {
						paths: Some(paths),
						position: Position { x: position.0, y: position.1 },
					},
				),
				DropEvent::Leave => app.emit_to(target, "tauri://drag-leave", ()),
			};
			if let Err(err) = sent {
				diagnostics::log_error(&format!("[copper] panel: could not emit a drag event: {err}"));
			}
		}),
	);
}

/// Reveals the panel **without** giving it focus, for the capture failure
/// notice.
///
/// Tauri's `WebviewWindow::show()` is not used here and neither is `reveal()`
/// above: a capture must never move focus, so the user keeps typing into
/// whatever they were typing into while the notice is on screen.
///
/// **This is the one call in the app that names a z-order band, so it is the one
/// call the pin has to be read from.**
///
/// `HWND_TOPMOST` while pinned. `HWND_TOP` does *not* clear `WS_EX_TOPMOST` —
/// only `HWND_NOTOPMOST` does that — so on a pinned window it is close to a
/// no-op: the window is already at the top of its own band and stays in it. The
/// explicit `HWND_TOPMOST` is what makes the raise unambiguous rather than
/// dependent on that.
///
/// `HWND_TOP` while unpinned, and *this* is the branch that matters: passing
/// `HWND_TOPMOST` there sets `WS_EX_TOPMOST` and promotes the window into the
/// topmost band, so a capture that failed would silently undo the user's unpin.
/// The corollary is that nothing here needs `HWND_NOTOPMOST`: the pin's own
/// setter has already cleared the style through `set_always_on_top`, and this
/// call only has to avoid putting it back. Both raise without activating, which
/// is all the notice needs.
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
	let band = if pinned() { HWND_TOPMOST } else { HWND_TOP };
	// SAFETY: `hwnd` is a live window handle owned by Tauri for the lifetime of
	// this call, and both calls are made on the thread that owns the window.
	unsafe {
		let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
		SetWindowPos(
			hwnd,
			Some(band),
			0,
			0,
			0,
			0,
			SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
		)?;
	}
	// A reveal is a reveal for the drop targets too, however it activates.
	install_drop_targets(window);
	Ok(())
}

// --- always on top -----------------------------------------------------------

/// The live pin state, mirroring `settings.json`'s `alwaysOnTop`.
///
/// An atomic for the same reason `theme`'s preference is one: the only reader is
/// [`reveal_without_activating`], which runs on the main thread inside a window
/// operation, and one byte there is cheaper than resolving managed state. It
/// starts `true` because `tauri.conf.json` declares the window `alwaysOnTop`, so
/// until [`install_always_on_top`] has run the mirror and the window agree.
static PINNED: AtomicBool = AtomicBool::new(true);

fn pinned() -> bool {
	PINNED.load(Ordering::Relaxed)
}

/// Applies the band to the window and records it for the reveal path.
///
/// The mirror is written whatever the window call does. A `set_always_on_top`
/// that failed leaves the two disagreeing either way; recording the *intent* at
/// least keeps the capture notice from re-promoting a window the user asked to
/// unpin, which is the failure this mirror exists to prevent.
fn apply_always_on_top(window: &WebviewWindow, enabled: bool) -> tauri::Result<()> {
	PINNED.store(enabled, Ordering::Relaxed);
	window.set_always_on_top(enabled)
}

/// Startup. Never fails the launch, exactly as `theme::install` does not: a pin
/// that could not be applied costs the user one window behaviour, and returning
/// `Err` from `setup()` would cost them the whole app.
pub fn install_always_on_top(app: &AppHandle, window: &WebviewWindow) {
	let enabled = store::commands::settings(app).always_on_top;
	if let Err(err) = apply_always_on_top(window, enabled) {
		diagnostics::log_error(&format!(
			"[copper] panel: could not apply the always-on-top setting: {err}"
		));
	}
}

/// Serialises the apply-persist-undo sequence below.
///
/// The three steps are one transaction and nothing else made them one. Two
/// requests a double-click apart interleave: both read `previous` before either
/// has applied anything, so the loser's undo restores a band the winner had
/// already replaced, and the *file* is then whichever `patch_settings` returned
/// last. The window and `settings.json` end up disagreeing, which is exactly the
/// contradiction the undo-on-failure step exists to prevent.
///
/// A lock rather than a generation counter, because the frontend already has the
/// generation half — `useSettings.attempt` discards a stale *answer*. What it
/// cannot do is stop two writes from being half-applied against each other on
/// this side, and only the side holding the window can.
static PIN_WRITE: Mutex<()> = Mutex::new(());

/// Applies first, persists second, and undoes the application if the write
/// fails — `theme::set_theme_preference`'s shape, for the same reason it has it.
///
/// A command rather than `getCurrentWindow().setAlwaysOnTop()` from JS. The
/// capability for that is not granted and `build.removeUnusedCommands` prunes an
/// ungranted window command out of the binary, so the first call would be a
/// runtime "command not found" — but the stronger reason is the one `hide_panel`
/// records: window operations are centralised here, and this one also owns the
/// mirror the capture notice reads.
#[tauri::command]
pub async fn set_always_on_top(enabled: bool, app: AppHandle) -> Result<Settings, ShellError> {
	let Some(window) = app.get_webview_window(PANEL_LABEL) else {
		return Err(ShellError::Invalid(
			"The panel window is not available.".to_owned(),
		));
	};

	// Held across the whole sequence, and there is deliberately no `.await` inside
	// it — every step below is blocking, so the guard cannot be carried across a
	// suspension point. Poisoning is recovered from rather than propagated: the
	// guarded value is `()`, so a panicking writer left no invariant broken, and
	// refusing every later pin over it would be the worse outcome.
	let _serialised = PIN_WRITE
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner);

	// Read inside the lock. Read outside it, this is the value the *other* request
	// is in the middle of replacing.
	let previous = pinned();
	if let Err(err) = apply_always_on_top(&window, enabled) {
		return Err(ShellError::Invalid(format!(
			"Copper couldn't change the window's always-on-top state: {err}"
		)));
	}

	let patch = SettingsPatch {
		always_on_top: Some(enabled),
		..SettingsPatch::default()
	};
	match store::commands::patch_settings(&app, patch) {
		Ok(settings) => Ok(settings),
		Err(err) => {
			let _ = apply_always_on_top(&window, previous);
			Err(ShellError::Persist(format!(
				"Copper couldn't save the always-on-top setting: {}",
				err.message()
			)))
		}
	}
}

// --- translucency ------------------------------------------------------------

/// The live material choice, mirroring `settings.json`'s `translucent`.
///
/// An atomic for the same reasons [`PINNED`] is one, plus a third: its reader is
/// [`apply_effects`], which the theme module calls from inside a window-event
/// callback on the main thread. It starts `false` because the window is born
/// wearing Mica — [`apply_effects`] runs once in `setup()` before the store is
/// even readable — so until [`install_translucency`] has run the mirror and the
/// window agree.
static TRANSLUCENT: AtomicBool = AtomicBool::new(false);

fn translucent() -> bool {
	TRANSLUCENT.load(Ordering::Relaxed)
}

/// Startup. **Records the choice; it does not apply it.**
///
/// The application is `theme::install`'s, which runs immediately after this and
/// calls [`apply_effects`] with the stored theme. Applying here as well would
/// paint the backdrop twice on every launch and — worse — the first of the two
/// would be painted with a theme this module has no business deciding. Recording
/// a byte cannot fail, so unlike [`install_always_on_top`] there is nothing here
/// to log.
pub fn install_translucency(app: &AppHandle) {
	TRANSLUCENT.store(store::commands::settings(app).translucent, Ordering::Relaxed);
}

/// Serialises the apply-persist-undo sequence below, exactly as [`PIN_WRITE`]
/// does for the band and for the same reason: the three steps are one
/// transaction, and two requests a double-click apart would otherwise both read
/// `previous` before either had applied anything.
static EFFECT_WRITE: Mutex<()> = Mutex::new(());

/// Sets the mirror and re-applies the backdrop, so the material actually changes.
///
/// The theme is read back out of `theme` rather than passed in: this call changes
/// the material only, and inventing a tint for it would drop an explicit light or
/// dark preference every time the user toggled translucency.
///
/// The error is flattened to a `String` here rather than propagated. [`apply_effects`]
/// returns `Box<dyn Error>`, which is not `Send`, and the caller is an async
/// command whose future has to be.
fn apply_translucency(window: &WebviewWindow, enabled: bool) -> Result<(), String> {
	TRANSLUCENT.store(enabled, Ordering::Relaxed);
	apply_effects(window, crate::theme::backdrop_dark()).map_err(|err| err.to_string())
}

/// Applies first, persists second, and undoes the application if the write fails
/// — [`set_always_on_top`]'s shape, for the same reason it has it.
///
/// The failure that is *not* a bug is the first step: Acrylic needs Windows 10
/// v1809 or newer, and a machine that cannot paint it must be told so rather than
/// left with a setting whose file says on and whose window says off. Nothing is
/// persisted in that case, so the row shows the reason and the panel keeps the
/// backdrop it had.
#[tauri::command]
pub async fn set_translucency(enabled: bool, app: AppHandle) -> Result<Settings, ShellError> {
	let Some(window) = app.get_webview_window(PANEL_LABEL) else {
		return Err(ShellError::Invalid(
			"The panel window is not available.".to_owned(),
		));
	};

	// Held across the whole sequence, with no `.await` inside it — see the note on
	// `PIN_WRITE`, including why poisoning is recovered from rather than
	// propagated.
	let _serialised = EFFECT_WRITE
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner);

	// Read inside the lock, or this is the value the *other* request is in the
	// middle of replacing.
	let previous = translucent();
	if let Err(err) = apply_translucency(&window, enabled) {
		// The mirror was already moved, so put it back before reporting — and put
		// the window back with it, since a half-applied material is exactly what the
		// undo step exists to prevent.
		let _ = apply_translucency(&window, previous);
		return Err(ShellError::Invalid(format!(
			"Copper couldn't change the panel's background: {err}"
		)));
	}

	let patch = SettingsPatch {
		translucent: Some(enabled),
		..SettingsPatch::default()
	};
	match store::commands::patch_settings(&app, patch) {
		Ok(settings) => Ok(settings),
		Err(err) => {
			let _ = apply_translucency(&window, previous);
			Err(ShellError::Persist(format!(
				"Copper couldn't save the background setting: {}",
				err.message()
			)))
		}
	}
}

// --- size and resizing -------------------------------------------------------

/// The live panel size, mirroring `settings.json`'s `panelWidth`/`panelHeight`.
///
/// A `Mutex` rather than the atomics [`PINNED`] and [`TRANSLUCENT`] use, because
/// this mirror is a *pair*: read as two atomics, a size command landing between
/// the two loads would hand [`fit_to_work_area`] one axis of the old size and one
/// of the new, and place the panel against a rectangle that was never asked for.
/// One lock makes the pair indivisible; nothing reads it on a hot path.
///
/// It starts at the declared size because that is what `tauri.conf.json` creates
/// the window at, so until [`install_panel_size`] has run the mirror and the
/// window agree — the same property [`TRANSLUCENT`]'s `false` gives.
static PANEL_SIZE: Mutex<(f64, f64)> = Mutex::new((PANEL_WIDTH, PANEL_HEIGHT));

/// The panel's configured logical size: what the settings ask for, before any
/// display has been consulted.
fn configured_size() -> (f64, f64) {
	*PANEL_SIZE
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn set_configured_size(size: (f64, f64)) {
	*PANEL_SIZE
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner) = size;
}

/// Applies the window's drag handles, and with them the floor a drag may not go
/// below.
///
/// A minimum and deliberately **no maximum**. The floor exists because a drag has
/// no undo: below [`PANEL_WIDTH_BOUNDS`]'s lower bound the composer and the
/// toolbar collide, and a user who collapses the panel to a sliver has no visible
/// way back. A ceiling would be inventing a limit that is already enforced —
/// Windows will not let a drag leave the work area, and [`size_to_fit`] shrinks the
/// panel to what the display can hold on the next launch regardless.
///
/// The floor is cleared rather than left standing when the setting goes off, so
/// the two branches are exact opposites: toggling twice leaves the window with the
/// constraints it was found with rather than with a limit nothing put there.
fn apply_resizable(window: &WebviewWindow, enabled: bool) -> tauri::Result<()> {
	window.set_resizable(enabled)?;
	if enabled {
		window.set_min_size(Some(LogicalSize::new(
			PANEL_WIDTH_BOUNDS.0,
			PANEL_HEIGHT_BOUNDS.0,
		)))
	} else {
		window.set_min_size(None::<LogicalSize<f64>>)
	}
}

/// Records the size and applies it, shrunk to the display the panel is on, then
/// checks the panel is still reachable.
///
/// The mirror moves *first*, because [`size_to_fit`] reads it: the fit has to be
/// computed against the size that was asked for, not the one the window still has.
///
/// The reachability check is the same one every reveal runs, and it is needed here
/// for a reason the reveal path does not have: growing the panel moves its right
/// and bottom edges without moving its position, so a window that was on-screen at
/// 440 wide can have its header pushed past the edge at 1200.
fn apply_panel_size(window: &WebviewWindow, size: (f64, f64)) -> tauri::Result<()> {
	set_configured_size(size);
	let Ok(position) = window.outer_position() else {
		// No position means no display to measure against, so the size applies
		// unfitted — closer to what was asked for than refusing to resize at all.
		return window.set_size(LogicalSize::new(size.0, size.1));
	};
	size_to_fit(
		window,
		PanelPosition {
			x: position.x,
			y: position.y,
		},
	)?;
	ensure_reachable(window);
	Ok(())
}

/// Startup. Never fails the launch, exactly as [`install_always_on_top`] does not.
///
/// Applies rather than merely recording, unlike [`install_translucency`]: nothing
/// downstream is going to paint this on its own. [`restore_position`] runs
/// immediately after and re-sizes the window against the display it places it on,
/// but it gives up early when the monitor list is unreadable — and a size the user
/// chose has to land in that case too, so it is applied unfitted here and narrowed
/// a moment later.
pub fn install_panel_size(app: &AppHandle, window: &WebviewWindow) {
	let settings = store::commands::settings(app);
	// Clamped on the way out as well as on the way in. The load-time repair already
	// corrected the file, but `update_settings` can write these keys directly, so
	// the value in memory is not guaranteed to have been through it.
	let size = clamp_panel_size(settings.panel_width, settings.panel_height);
	set_configured_size(size);

	if let Err(err) = window.set_size(LogicalSize::new(size.0, size.1)) {
		diagnostics::log_error(&format!(
			"[copper] panel: could not apply the stored size: {err}"
		));
	}
	if let Err(err) = apply_resizable(window, settings.resizable) {
		diagnostics::log_error(&format!(
			"[copper] panel: could not apply the resizable setting: {err}"
		));
	}
}

/// Serialises both writes below, exactly as [`PIN_WRITE`] and [`EFFECT_WRITE`] do
/// for the band and the material, and for the same reason: apply-persist-undo is
/// one transaction, and two requests a double-click apart would otherwise both
/// read `previous` before either had applied anything.
///
/// **One lock for both commands**, not one each. They are not independent: a
/// resize and a resizable toggle both end in `patch_settings`, and only one of the
/// two can be the last writer — sharing the lock is what stops an undo from one
/// restoring a `Settings` the other had already moved past.
static SIZE_WRITE: Mutex<()> = Mutex::new(());

/// Applies first, persists second, and undoes the application if the write fails
/// — [`set_translucency`]'s shape, for the same reason it has it.
#[tauri::command]
pub async fn set_resizable(enabled: bool, app: AppHandle) -> Result<Settings, ShellError> {
	let Some(window) = app.get_webview_window(PANEL_LABEL) else {
		return Err(ShellError::Invalid(
			"The panel window is not available.".to_owned(),
		));
	};

	// Held across the whole sequence, with no `.await` inside it — see the note on
	// `PIN_WRITE`, including why poisoning is recovered from rather than propagated.
	let _serialised = SIZE_WRITE
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner);

	// Read inside the lock, and read from the store rather than from a mirror of its
	// own. `PINNED` and `TRANSLUCENT` exist because something else in this process
	// reads them — the capture notice's band, the backdrop's material — and nothing
	// reads `resizable` at all: the window is the only thing that holds it. A second
	// copy would exist purely to be kept in sync.
	let previous = store::commands::settings(&app).resizable;
	if let Err(err) = apply_resizable(&window, enabled) {
		return Err(ShellError::Invalid(format!(
			"Copper couldn't change whether the panel can be resized: {err}"
		)));
	}

	let patch = SettingsPatch {
		resizable: Some(enabled),
		..SettingsPatch::default()
	};
	match store::commands::patch_settings(&app, patch) {
		Ok(settings) => Ok(settings),
		Err(err) => {
			let _ = apply_resizable(&window, previous);
			Err(ShellError::Persist(format!(
				"Copper couldn't save the resizable setting: {}",
				err.message()
			)))
		}
	}
}

/// Applies first, persists second, and undoes the application if the write fails
/// — [`set_translucency`]'s shape again.
///
/// The clamp is [`clamp_panel_size`], which is the *same* function
/// `RawSettings::repair` uses: a size the file would have been corrected for must
/// not be reachable by asking for it politely. It happens before the lock because
/// it touches nothing shared, and the clamped pair is what gets both applied and
/// persisted, so the window and the file cannot end up describing different sizes.
#[tauri::command]
pub async fn set_panel_size(
	width: f64,
	height: f64,
	app: AppHandle,
) -> Result<Settings, ShellError> {
	let Some(window) = app.get_webview_window(PANEL_LABEL) else {
		return Err(ShellError::Invalid(
			"The panel window is not available.".to_owned(),
		));
	};
	let (width, height) = clamp_panel_size(width, height);

	// Held across the whole sequence, with no `.await` inside it — see the note on
	// `PIN_WRITE`.
	let _serialised = SIZE_WRITE
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner);

	// Read inside the lock, or this is the size the *other* request is in the middle
	// of replacing.
	let previous = configured_size();
	if let Err(err) = apply_panel_size(&window, (width, height)) {
		// The mirror was already moved, so put it back — and put the window back with
		// it, or the panel keeps a size the file never records.
		let _ = apply_panel_size(&window, previous);
		return Err(ShellError::Invalid(format!(
			"Copper couldn't resize the panel: {err}"
		)));
	}

	let patch = SettingsPatch {
		panel_width: Some(width),
		panel_height: Some(height),
		..SettingsPatch::default()
	};
	match store::commands::patch_settings(&app, patch) {
		Ok(settings) => Ok(settings),
		Err(err) => {
			let _ = apply_panel_size(&window, previous);
			Err(ShellError::Persist(format!(
				"Copper couldn't save the panel size: {}",
				err.message()
			)))
		}
	}
}

/// Hides the panel. The window is never destroyed, only hidden.
///
/// Every hide path also ends any recording session, which is why the call lives
/// here rather than at each caller. A lease has the live chords unregistered while
/// it is open, and hiding is exactly the route that bypasses the recorder's own
/// cancel — the tray toggle, the summon chord, `Escape`, the close button. Left
/// standing behind a hidden panel, the user is left with no summon shortcut; the
/// Rust lease's watchdog would catch it eventually, this catches it at once. Off
/// the main thread, per the note in `shortcuts`.
pub fn hide(window: &WebviewWindow) -> tauri::Result<()> {
	crate::shortcuts::cancel_recording_off_thread(window.app_handle());
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

/// Brings a hidden panel back, checking first that it can be seen.
///
/// **The one reveal-from-hidden path.** Placement has to be validated here rather
/// than at each caller: a display unplugged while Copper was hidden leaves the
/// saved position pointing at nothing, and a reveal that skipped the check would
/// bring the panel back where the user cannot reach it — with the tray, the
/// supposed recovery surface, unable to help. Every path that reveals goes
/// through this, so none of them can be the one that forgets.
fn reveal_reachable(window: &WebviewWindow) -> tauri::Result<()> {
	ensure_reachable(window);
	reveal(window)
}

/// Reveals the panel, or logs why it could not be reached.
pub fn reveal_or_log<M: Manager<tauri::Wry>>(app: &M) {
	crate::capture::panel_revealed_by_user(app);
	with_panel(app, "reveal", reveal_reachable);
}

/// Hides the panel, or logs why it could not be reached.
pub fn hide_or_log<M: Manager<tauri::Wry>>(app: &M) {
	with_panel(app, "hide", hide);
}

/// The Escape ladder's last rung, from the webview.
///
/// A command rather than `getCurrentWindow().hide()` from JS, even though
/// `core:window:allow-hide` is granted and would allow it. Hiding is not just a
/// window call here — it also ends an open recording session — and task-002
/// centralised the window operations precisely so a later phase could not end up
/// with a second path that forgets half of one.
#[tauri::command]
pub async fn hide_panel(app: AppHandle) {
	hide_or_log(&app);
}

/// Hides the panel if it is visible and reveals it otherwise, or logs why it
/// could not be reached. The tray's left-click **and** the summon chord, kept
/// here so that the window lookup stays in the module that owns the window.
///
/// **Two states, deliberately — visible means hide, whatever holds focus.** The
/// chord spent a while as a three-state toggle that raised a
/// visible-but-unfocused panel instead of hiding it, reasoning that the user
/// was reaching for a panel buried under other windows. That reading loses to
/// the panel's actual life: it ships always-on-top, so it is never buried —
/// visible-but-unfocused is its *resting* state while the user types elsewhere,
/// and the raise turned "go away" into a chord that had to be pressed twice. A
/// user who can see the panel and presses the chord is dismissing it.
///
/// The hidden case is also where placement is checked, so a panel left on a
/// monitor that has since been unplugged comes back somewhere reachable instead
/// of being summoned into nowhere.
pub fn toggle_or_log<M: Manager<tauri::Wry>>(app: &M) {
	with_panel(app, "toggle", |window| {
		if is_visible(window) {
			hide(window)
		} else {
			// Only the reveal branch. Telling capture the user opened the panel when
			// they in fact just closed it would hand a live notice episode a window
			// it does not own, and the notice would then leave the panel up.
			crate::capture::panel_revealed_by_user(app);
			reveal_reachable(window)
		}
	});
}

/// Whether the panel is currently visible, defaulting to `false` if it cannot be
/// determined — a failed query should not leave the tray toggle stuck.
///
/// Read by capture as well as by the two toggles: a capture notification is worth
/// firing only when the note lands somewhere the user cannot see it. The default
/// serves that reading too, since a toast the user did not need costs less than a
/// capture they never learn about. Main thread only, like every window call here.
pub fn is_visible(window: &WebviewWindow) -> bool {
	window.is_visible().unwrap_or(false)
}

// --- position ----------------------------------------------------------------

/// One monitor's usable rectangle, in physical pixels.
///
/// Built from `work_area()` rather than `position()`/`size()`: the work area
/// excludes the taskbar, and validating against the raw monitor rectangle
/// happily places the panel underneath it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorRect {
	pub x: i32,
	pub y: i32,
	pub width: u32,
	pub height: u32,
}

impl MonitorRect {
	fn left(self) -> i64 {
		i64::from(self.x)
	}

	fn top(self) -> i64 {
		i64::from(self.y)
	}

	fn right(self) -> i64 {
		self.left() + i64::from(self.width)
	}

	fn bottom(self) -> i64 {
		self.top() + i64::from(self.height)
	}
}

/// The part of the panel a user has to be able to hit to move it — its draggable
/// header — in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrabRect {
	pub width: i64,
	pub height: i64,
}

/// How much of the header's width has to be on a monitor for it to be grabbable.
const MIN_GRAB_WIDTH: i64 = 64;

/// Keeps a restored position only if the panel can actually be reached there.
///
/// Pure over rectangles, so it is unit-testable with no display attached — which
/// matters, because the case it exists for is a monitor that is *not* attached.
///
/// The test is written against the header strip rather than the whole panel: a
/// window whose bottom-right corner clips a monitor while its header sits
/// off-screen cannot be dragged back and is lost. The two thresholds differ for
/// the same reason — 64 px of width is a comfortable pointer target, but the
/// header is only 48 logical pixels tall, so requiring 64 px vertically would
/// reject every position on every display. Half the header's height is the
/// vertical equivalent of the same idea.
///
/// All arithmetic widens to `i64` so that a corrupt or extreme saved coordinate
/// cannot overflow and compute as visible.
pub fn clamp_to_visible_monitor(
	saved: PanelPosition,
	grab: GrabRect,
	monitors: &[MonitorRect],
	fallback: PanelPosition,
) -> PanelPosition {
	if is_reachable(saved, grab, monitors) {
		saved
	} else {
		fallback
	}
}

/// The predicate on its own, so a caller that only wants the answer does not have
/// to compute a fallback it will not use — locating the cursor's monitor costs a
/// display enumeration, and every reveal asks this question.
fn is_reachable(saved: PanelPosition, grab: GrabRect, monitors: &[MonitorRect]) -> bool {
	let left = i64::from(saved.x);
	let top = i64::from(saved.y);
	let right = left.saturating_add(grab.width);
	let bottom = top.saturating_add(grab.height);
	let needed_height = (grab.height / 2).max(1);

	monitors.iter().any(|monitor| {
		let overlap_x = right.min(monitor.right()) - left.max(monitor.left());
		let overlap_y = bottom.min(monitor.bottom()) - top.max(monitor.top());
		overlap_x >= MIN_GRAB_WIDTH && overlap_y >= needed_height
	})
}

/// The panel's logical size on a given monitor: the **configured** size, shrunk on
/// each axis to whatever the work area can actually hold.
///
/// **Scale is why this exists.** The size is declared in *logical* units and
/// Windows multiplies it by the display's scale factor: at 150% on a 1080p screen
/// a 760-tall panel is 1140 physical pixels against a work area of roughly 1040,
/// so its bottom lands under the taskbar wherever it is placed. No arithmetic in
/// [`default_position`] can fix a window taller than the space it goes in — only a
/// smaller window can — which is why placement asks this first rather than
/// clamping a coordinate afterwards.
///
/// Reads [`configured_size`] rather than the declared constants, and that is the
/// whole of what makes the size setting real: every consumer of the panel's size —
/// placement, the grab rectangle, the sizing call itself — goes through here, so a
/// user who asked for 620×900 gets their placement computed against 620×900 and
/// not against the size the window was born at.
///
/// Returned in logical units because that is what `set_size` takes; every caller
/// that needs physical pixels multiplies by the same scale it passed in.
fn fitted_logical_size(monitor: MonitorRect, scale: f64) -> (f64, f64) {
	fit_to_work_area(configured_size(), monitor, scale)
}

/// The arithmetic on its own, over a size handed in rather than read from the
/// mirror — so the fit can be tested against a panel that is not the shipped one
/// without a global to put back afterwards.
fn fit_to_work_area((width, height): (f64, f64), monitor: MonitorRect, scale: f64) -> (f64, f64) {
	// A scale of zero or worse is not something a display reports, but it arrives
	// here through an API rather than a constant, and dividing by it would poison
	// the size with an infinity.
	let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
	(
		width.min(f64::from(monitor.width) / scale),
		height.min(f64::from(monitor.height) / scale),
	)
}

/// Right-aligned with an inset, vertically centred.
///
/// A corner rather than the screen centre, because the panel is a side companion
/// to whatever the user is actually working in — centring would put it on top of
/// that.
///
/// Computed against [`fitted_logical_size`] rather than the declared size, so the
/// placement and the window agree about how big the panel is. With the two out of
/// step the arithmetic centred a panel that did not fit: `y` clamped to the top of
/// the work area and the bottom disappeared under the taskbar.
fn default_position(monitor: MonitorRect, scale: f64) -> PanelPosition {
	let (width, height) = fitted_logical_size(monitor, scale);
	let panel_width = (width * scale).round() as i64;
	let panel_height = (height * scale).round() as i64;
	let inset = (DEFAULT_INSET * scale).round() as i64;

	// Only a lower bound is needed on each axis. The panel now fits by construction,
	// so subtracting it from the far edge cannot overshoot the near one — an inset
	// wider than the leftover space is the single case that can, and it clamps to a
	// panel flush with the right edge rather than one hanging off it.
	let x = (monitor.right() - panel_width - inset).max(monitor.left());
	let y = (monitor.top() + (i64::from(monitor.height) - panel_height) / 2).max(monitor.top());
	PanelPosition {
		x: x as i32,
		y: y as i32,
	}
}

fn rect_of(monitor: &tauri::window::Monitor) -> MonitorRect {
	let area = monitor.work_area();
	MonitorRect {
		x: area.position.x,
		y: area.position.y,
		width: area.size.width,
		height: area.size.height,
	}
}

/// The monitor the pointer is on, falling back to the primary one.
///
/// The panel is summoned to where the user is working, not to wherever the app
/// happened to be when it last exited — which for a multi-monitor desk is the
/// difference between a companion panel and one that keeps appearing on the
/// wrong screen. This only decides the *default*: a position the user chose is
/// kept as long as it is reachable.
fn current_monitor(window: &WebviewWindow) -> Option<tauri::window::Monitor> {
	if let Ok(cursor) = window.cursor_position() {
		if let Ok(Some(monitor)) = window.monitor_from_point(cursor.x, cursor.y) {
			return Some(monitor);
		}
	}
	window.primary_monitor().ok().flatten()
}

fn monitor_rects(window: &WebviewWindow) -> Vec<MonitorRect> {
	window
		.available_monitors()
		.map(|monitors| monitors.iter().map(rect_of).collect())
		.unwrap_or_default()
}

/// Where the panel goes when the saved position is unusable, and the scale to
/// compute it at.
///
/// `monitors` is the list that has already been proved non-empty, and it is the
/// last resort here for a reason: giving up because the *cursor* could not be
/// located would leave the panel wherever Tauri put it, which is the outcome this
/// whole validation exists to prevent. A display we can enumerate is a display we
/// can place on, so an unreadable cursor costs the user a preferred screen rather
/// than a reachable window. Its scale is unknown at that point, so 1.0 stands in
/// — a wrong scale can only shift the default placement, never make it unreachable.
fn fallback_position(window: &WebviewWindow, monitors: &[MonitorRect]) -> Option<PanelPosition> {
	if let Some(monitor) = current_monitor(window) {
		return Some(default_position(rect_of(&monitor), monitor.scale_factor()));
	}
	monitors
		.first()
		.map(|rect| default_position(*rect, 1.0))
}

/// The monitor a position lands on, falling back to the one the pointer is on.
fn monitor_at(window: &WebviewWindow, at: PanelPosition) -> Option<tauri::window::Monitor> {
	window
		.monitor_from_point(f64::from(at.x), f64::from(at.y))
		.ok()
		.flatten()
		.or_else(|| current_monitor(window))
}

/// The grab rectangle at the scale of whichever monitor the position lands on.
///
/// The saved position is physical, and on a 150% display the panel is half again
/// as wide as its logical size — so a logical-pixel grab rect would judge a
/// perfectly reachable position as lost. Its width comes from
/// [`fitted_logical_size`] for the same reason [`default_position`] does: a panel
/// narrowed to fit the display has a narrower header to grab, and measuring the
/// declared width would judge reachability against a window that is not there.
///
/// The header's height is never fitted. It is 48 logical pixels against a monitor
/// hundreds tall, and a display too short for it is one no placement could save.
fn grab_rect(window: &WebviewWindow, at: PanelPosition) -> GrabRect {
	let monitor = monitor_at(window, at);
	let scale = monitor.as_ref().map_or(1.0, |monitor| monitor.scale_factor());
	let width = monitor
		.as_ref()
		.map_or(configured_size().0, |monitor| {
			fitted_logical_size(rect_of(monitor), scale).0
		});
	GrabRect {
		width: (width * scale).round() as i64,
		height: (HEADER_HEIGHT * scale).round() as i64,
	}
}

/// Sizes the window to [`configured_size`], shrunk to the work area it is about to
/// sit in.
///
/// Applies the size unconditionally rather than only when it has to shrink, and
/// that changed with the size setting: the window is born at the *declared* size,
/// so a configured 620×900 is a size nothing else would ever put on it. The log
/// line still fires only for the shrinking case, which is the one worth
/// explaining.
///
/// `resizable: false` stops the *user* resizing the panel and not this: `set_size`
/// applies either way. Called from [`restore_position`] before the panel is ever
/// shown, so a display whose scale cannot fit the configured size gets a panel that
/// is smaller rather than one whose bottom is behind the taskbar.
///
/// One-way per call, by design. Nothing here grows the window back on a monitor
/// that could hold it, because nothing re-runs it: this is startup placement and
/// [`set_panel_size`], and a panel dragged to a roomier display keeps the size it
/// was given until one of those two happens again. Recording that rather than
/// hiding it — the alternative is a resize on every `Moved` event, which is a far
/// larger mechanism for a case that resolves itself.
fn size_to_fit(window: &WebviewWindow, at: PanelPosition) -> tauri::Result<()> {
	let (width, height) = configured_size();
	let Some(monitor) = monitor_at(window, at) else {
		// No display to measure against; the configured size unfitted is closer to
		// what was asked for than leaving the window at whatever size it had.
		return window.set_size(LogicalSize::new(width, height));
	};
	let scale = monitor.scale_factor();
	let (fitted_width, fitted_height) = fitted_logical_size(rect_of(&monitor), scale);
	if fitted_width < width || fitted_height < height {
		diagnostics::log(&format!(
			"[copper] panel: work area {}×{} at {scale}× cannot hold {width}×{height}; sizing to \
			 {fitted_width}×{fitted_height}",
			monitor.work_area().size.width,
			monitor.work_area().size.height,
		));
	}
	window.set_size(LogicalSize::new(fitted_width, fitted_height))
}

/// Places the panel at startup: the saved position when it is still reachable,
/// the current monitor's default otherwise.
///
/// Called before the window is ever shown. `panel_position` is `Option` and
/// defaults to `null`, so a first run takes the default path rather than the
/// restore path. Tauri does not clamp an out-of-bounds `set_position`, so
/// skipping this validation puts the panel where the user cannot reach it.
pub fn restore_position(window: &WebviewWindow, saved: Option<PanelPosition>) {
	let monitors = monitor_rects(window);
	if monitors.is_empty() {
		// Nothing to validate against, and computing a position from nothing would
		// be worse than Tauri's own centring.
		diagnostics::log_error("[copper] panel: no monitors reported; keeping the default placement");
		return;
	}

	let Some(fallback) = fallback_position(window, &monitors) else {
		return;
	};

	let target = match saved {
		Some(saved) => clamp_to_visible_monitor(saved, grab_rect(window, saved), &monitors, fallback),
		None => fallback,
	};

	// Sized before it is placed, and against the monitor it is going to: the
	// placement above already assumes the fitted size, so applying it afterwards
	// would leave the window one size and the arithmetic another.
	if let Err(err) = size_to_fit(window, target) {
		diagnostics::log_error(&format!("[copper] panel: could not size the window: {err}"));
	}

	if let Err(err) = window.set_position(PhysicalPosition::new(target.x, target.y)) {
		diagnostics::log_error(&format!("[copper] panel: could not place the window: {err}"));
	}
}

/// Moves the panel back on-screen if it is not, leaving it alone if it is.
///
/// Run before revealing a hidden panel, so a display unplugged while Copper was
/// hidden cannot summon it into nowhere.
fn ensure_reachable(window: &WebviewWindow) {
	let Ok(position) = window.outer_position() else {
		return;
	};
	let current = PanelPosition {
		x: position.x,
		y: position.y,
	};
	let monitors = monitor_rects(window);
	if monitors.is_empty() {
		return;
	}
	// Asked before a fallback is computed, not after: locating the cursor's
	// monitor is a display enumeration of its own, and the panel is reachable on
	// almost every reveal.
	if is_reachable(current, grab_rect(window, current), &monitors) {
		return;
	}
	let Some(target) = fallback_position(window, &monitors) else {
		return;
	};

	if target != current {
		if let Err(err) = window.set_position(PhysicalPosition::new(target.x, target.y)) {
			diagnostics::log_error(&format!("[copper] panel: could not recover the window: {err}"));
		}
	}
}

/// How long the panel has to stop moving before its position is written.
const POSITION_DEBOUNCE: Duration = Duration::from_millis(500);

/// The debounce behind `WindowEvent::Moved`.
///
/// A drag emits a move per frame, and each one would otherwise be an atomic
/// rewrite of `settings.json`. This collapses a burst of movement into one writer
/// thread rather than one per frame, and the store's own change-guard makes a
/// redundant flush free.
///
/// Two writers can overlap briefly — `scheduled` is cleared *before* the write, so
/// that a move arriving during it schedules a fresh pass rather than being
/// swallowed — and `flush_position` holding `pending` across the whole take-and-
/// write is what makes that safe rather than a lost update.
#[derive(Default)]
pub struct PositionState {
	pending: Mutex<Option<PanelPosition>>,
	/// Bumped on every move, so the sleeping writer can tell a finished drag from
	/// a paused one without holding anything.
	generation: AtomicU64,
	scheduled: AtomicBool,
}

/// Records a move and schedules the write.
///
/// **There is deliberately no `WindowEvent::Resized` counterpart, and its absence
/// is a ruling rather than an omission.** `panelWidth`/`panelHeight` hold the
/// panel's *default* size — what it opens at — and a manual drag-resize is
/// session-only: it lasts until the window is next sized, and the next launch
/// opens at the stored size again. Position and size are asymmetric here on
/// purpose. A dragged position is the only record of where the user wants the
/// panel, so losing it loses information; a dragged size is a temporary
/// accommodation ("let me see more of this note for a minute"), and persisting it
/// would silently overwrite a number the settings view presents as a deliberate
/// choice. The one way to change the stored size is [`set_panel_size`].
pub fn on_moved(app: &AppHandle, position: PhysicalPosition<i32>) {
	let Some(state) = app.try_state::<PositionState>() else {
		return;
	};
	if let Ok(mut pending) = state.pending.lock() {
		*pending = Some(PanelPosition {
			x: position.x,
			y: position.y,
		});
	}
	state.generation.fetch_add(1, Ordering::Relaxed);

	if state
		.scheduled
		.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
		.is_err()
	{
		return;
	}

	let app = app.clone();
	std::thread::spawn(move || {
		loop {
			let Some(state) = app.try_state::<PositionState>() else {
				return;
			};
			let seen = state.generation.load(Ordering::Relaxed);
			std::thread::sleep(POSITION_DEBOUNCE);
			if state.generation.load(Ordering::Relaxed) == seen {
				// Cleared before the write, so a move arriving during it schedules a
				// fresh pass rather than being swallowed.
				state.scheduled.store(false, Ordering::SeqCst);
				break;
			}
		}
		flush_position(&app);
	});
}

/// Writes the pending position now.
///
/// Called at exit as well as by the debounce: a drag followed promptly by
/// quitting is the common case for someone repositioning the panel and then
/// closing the app, and without this it is exactly the case that is lost.
pub fn flush_position(app: &AppHandle) {
	let Some(state) = app.try_state::<PositionState>() else {
		return;
	};
	// The guard is held across the write, not merely across the take. Dropped
	// between the two, an exit flush arriving in that window would find an empty
	// slot, return, and let the process exit before the write it was waiting on had
	// landed — losing exactly the drag-then-quit this function exists to catch.
	let Ok(mut pending) = state.pending.lock() else {
		return;
	};
	let Some(position) = pending.take() else {
		return;
	};

	let patch = SettingsPatch {
		panel_position: Some(Some(position)),
		..SettingsPatch::default()
	};
	if let Err(err) = crate::store::commands::patch_settings(app, patch) {
		diagnostics::log_error(&format!(
			"[copper] panel: could not save the window position: {}",
			err.message()
		));
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The panel as the clamp sees it: its full width, and only the header's
	/// height. Derived from the constant rather than repeating its number, so a
	/// resize cannot leave these tests asserting about the old window.
	const GRAB: GrabRect = GrabRect {
		width: PANEL_WIDTH as i64,
		height: HEADER_HEIGHT as i64,
	};

	const PRIMARY: MonitorRect = MonitorRect {
		x: 0,
		y: 0,
		width: 1920,
		height: 1040,
	};

	const SECOND: MonitorRect = MonitorRect {
		x: 1920,
		y: 0,
		width: 1920,
		height: 1040,
	};

	/// What `default_position` produces on `PRIMARY` at scale 1: inset from the
	/// right edge, centred vertically. Written out rather than computed, so the
	/// arithmetic cannot agree with itself — and tied to the real function by
	/// `the_fallback_fixture_is_the_real_default` below, which is what keeps these
	/// tests describing the panel that actually ships after a resize.
	const FALLBACK: PanelPosition = PanelPosition { x: 1456, y: 140 };

	fn clamp(saved: PanelPosition, monitors: &[MonitorRect]) -> PanelPosition {
		clamp_to_visible_monitor(saved, GRAB, monitors, FALLBACK)
	}

	#[test]
	fn a_position_inside_a_monitor_is_kept() {
		let saved = PanelPosition { x: 200, y: 300 };
		assert_eq!(clamp(saved, &[PRIMARY, SECOND]), saved);
	}

	#[test]
	fn a_position_on_a_disconnected_monitor_falls_back() {
		let saved = PanelPosition { x: 2400, y: 300 };
		assert_eq!(clamp(saved, &[PRIMARY, SECOND]), saved);
		// The second display is unplugged and the saved point is now nowhere.
		assert_eq!(clamp(saved, &[PRIMARY]), FALLBACK);
	}

	#[test]
	fn a_position_straddling_two_monitors_is_kept() {
		// Two hundred pixels of header on each side: reachable on either.
		let saved = PanelPosition { x: 1720, y: 300 };
		assert_eq!(clamp(saved, &[PRIMARY, SECOND]), saved);
	}

	#[test]
	fn a_position_barely_touching_a_monitor_falls_back() {
		// Ten pixels of header showing is not something a user can grab.
		let saved = PanelPosition { x: -380, y: 300 };
		assert_eq!(clamp(saved, &[PRIMARY]), FALLBACK);
	}

	#[test]
	fn a_body_on_screen_with_the_header_above_it_falls_back() {
		// The whole point of testing the header rather than the panel: the body
		// overlaps by hundreds of pixels and the window still cannot be moved.
		let saved = PanelPosition { x: 200, y: -48 };
		assert_eq!(clamp(saved, &[PRIMARY]), FALLBACK);
		// Half the header showing is still grabbable.
		let half = PanelPosition { x: 200, y: -24 };
		assert_eq!(clamp(half, &[PRIMARY]), half);
	}

	#[test]
	fn an_extreme_coordinate_cannot_overflow_into_looking_visible() {
		for saved in [
			PanelPosition {
				x: i32::MIN,
				y: i32::MIN,
			},
			PanelPosition {
				x: i32::MAX,
				y: i32::MAX,
			},
			PanelPosition {
				x: i32::MAX,
				y: 300,
			},
		] {
			assert_eq!(clamp(saved, &[PRIMARY, SECOND]), FALLBACK, "{saved:?}");
		}
	}

	#[test]
	fn no_monitors_means_no_opinion() {
		let saved = PanelPosition { x: 200, y: 300 };
		assert_eq!(clamp(saved, &[]), FALLBACK);
	}

	#[test]
	fn the_default_sits_inside_the_work_area_at_every_scale() {
		for scale in [1.0, 1.25, 1.5, 2.0] {
			let monitor = MonitorRect {
				x: 0,
				y: 0,
				width: (1920.0 * scale) as u32,
				height: (1040.0 * scale) as u32,
			};
			let placed = default_position(monitor, scale);
			assert!(placed.x >= monitor.x, "scale {scale}: {placed:?}");
			assert!(placed.y >= monitor.y, "scale {scale}: {placed:?}");
			// And the panel that lands there is reachable, which is the property that
			// actually matters — the fallback must never itself need clamping.
			let grab = GrabRect {
				width: (PANEL_WIDTH * scale).round() as i64,
				height: (HEADER_HEIGHT * scale).round() as i64,
			};
			assert_eq!(
				clamp_to_visible_monitor(placed, grab, &[monitor], FALLBACK),
				placed,
				"scale {scale}: the fallback position is not reachable"
			);
		}
	}

	/// The mirror the capture notice reads, and the reason it exists: with the pin
	/// off, `reveal_without_activating` must pick the non-topmost band, or a failed
	/// capture silently undoes the unpin.
	///
	/// The initial value is deliberately *not* asserted here. `PINNED` is a process
	/// static and the test binary is threaded, so "what it was before anything
	/// touched it" is a claim about test ordering rather than about the code —
	/// `the_shipped_default_is_pinned` below makes the same claim in a form that
	/// cannot flake.
	#[test]
	fn the_pin_mirror_round_trips() {
		let held = pinned();

		PINNED.store(false, Ordering::Relaxed);
		assert!(!pinned());

		PINNED.store(true, Ordering::Relaxed);
		assert!(pinned());

		PINNED.store(held, Ordering::Relaxed);
	}

	/// The same round trip for the material mirror, and the same reason the
	/// initial value is asserted separately below rather than here: `TRANSLUCENT`
	/// is a process static and the test binary is threaded.
	#[test]
	fn the_material_mirror_round_trips() {
		let held = translucent();

		TRANSLUCENT.store(true, Ordering::Relaxed);
		assert!(translucent());

		TRANSLUCENT.store(false, Ordering::Relaxed);
		assert!(!translucent());

		TRANSLUCENT.store(held, Ordering::Relaxed);
	}

	/// The static's initial value and the store's default have to agree, or
	/// `apply_effects` runs once in `setup()` — before the store is readable —
	/// against a material the file does not name.
	#[test]
	fn the_shipped_default_is_opaque() {
		assert!(!copper_core::store::settings::Settings::default().translucent);
	}

	#[test]
	fn the_shipped_default_is_pinned() {
		// The static's initial value, `tauri.conf.json`'s `alwaysOnTop` and the
		// store's default all have to agree, or the first launch runs with the window
		// in one band and the file naming the other.
		assert!(copper_core::store::settings::Settings::default().always_on_top);
	}

	/// Ties the fixtures in here to the real function, so a resize cannot leave
	/// these tests describing a panel that no longer ships. It failed on the
	/// 390×660 → 440×760 change, which is the whole reason it exists.
	#[test]
	fn the_fallback_fixture_is_the_real_default() {
		assert_eq!(default_position(PRIMARY, 1.0), FALLBACK);
	}

	/// The drift this half is about is between the constants and
	/// `tauri.conf.json` — the *other* declaration of the same size, and the one
	/// that actually creates the window. The test above was documented as catching
	/// it and never could have: it reads the constants only, so editing both of
	/// them and leaving the config alone passed. This reads the file.
	#[test]
	fn the_constants_match_the_window_in_tauri_conf() {
		let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
			.expect("tauri.conf.json is valid JSON");
		let window = config["app"]["windows"]
			.as_array()
			.and_then(|windows| {
				windows
					.iter()
					.find(|window| window["label"] == serde_json::json!(PANEL_LABEL))
			})
			.expect("tauri.conf.json declares the panel window");

		assert_eq!(window["width"].as_f64(), Some(PANEL_WIDTH));
		assert_eq!(window["height"].as_f64(), Some(PANEL_HEIGHT));
		// Still `false`, and still correct now that the panel *can* be made
		// resizable. The config declares what the window is born with, and it is born
		// before any setting can be read: `install_panel_size` applies the stored
		// `resizable` in `setup()`, after creation. Declaring `true` here would hand a
		// drag handle to every install for the moments before that call, including the
		// ones whose file says the panel is fixed.
		assert_eq!(window["resizable"].as_bool(), Some(false));
	}

	/// The third declaration of the panel's size, after the constants above and
	/// `tauri.conf.json`. `install_panel_size` reads the store, so a store default
	/// that disagreed with the window would resize every panel on first launch —
	/// silently, and to a size nobody chose.
	#[test]
	fn the_shipped_default_size_is_the_declared_one() {
		let defaults = copper_core::store::settings::Settings::default();
		assert_eq!(defaults.panel_width, PANEL_WIDTH);
		assert_eq!(defaults.panel_height, PANEL_HEIGHT);
		// And the panel ships fixed, matching `resizable` in the config above.
		assert!(!defaults.resizable);
	}

	/// The mirror's initial value and the window's birth size have to agree, or
	/// `restore_position` — which runs before anything has read the store on the
	/// paths where `install_panel_size` failed — places the panel against a
	/// rectangle the window does not have.
	///
	/// Asserted as an equality with the constants rather than by moving the mirror,
	/// deliberately: `PANEL_SIZE` is a process static read by `fitted_logical_size`,
	/// which almost every test in this module reaches through `default_position`.
	/// Nothing here writes it, which is what keeps those tests describing the panel
	/// that actually ships.
	#[test]
	fn the_size_mirror_starts_at_the_declared_size() {
		assert_eq!(configured_size(), (PANEL_WIDTH, PANEL_HEIGHT));
	}

	/// The fit is a cap on whatever size is configured, not a cap on the shipped
	/// one — the property that makes a user-chosen 1200×1600 as safe on a small
	/// display as the default is.
	#[test]
	fn the_fit_caps_a_configured_size_the_display_cannot_hold() {
		let work_area = MonitorRect {
			x: 0,
			y: 0,
			width: 1920,
			height: 1040,
		};

		// Wider and taller than the work area allows at 1×: both axes come down.
		assert_eq!(
			fit_to_work_area((1200.0, 1600.0), work_area, 1.0),
			(1200.0, 1040.0),
			"the height must be capped and the width left alone"
		);
		// At 150% the same work area is 1280×693 logical, so the width still fits and
		// the height comes down much further — the axes are capped independently, which
		// is the point.
		let (width, height) = fit_to_work_area((1200.0, 1600.0), work_area, 1.5);
		assert_eq!(width, 1200.0, "a width the display still holds must be left alone");
		assert!(height < 700.0, "{height}");
		assert!((height * 1.5).round() as u32 <= work_area.height, "{height} overflows");

		// A size below the smallest sensible display is left exactly alone: the fit
		// only ever shrinks.
		assert_eq!(
			fit_to_work_area((360.0, 480.0), work_area, 1.0),
			(360.0, 480.0)
		);
	}

	/// The floor a drag may not cross is the same number the store clamps to, not a
	/// second opinion about how small the panel may be.
	#[test]
	fn the_drag_floor_is_the_stores_lower_bound() {
		use copper_core::store::settings::{clamp_panel_size, PANEL_HEIGHT_BOUNDS, PANEL_WIDTH_BOUNDS};

		assert_eq!(
			clamp_panel_size(0.0, 0.0),
			(PANEL_WIDTH_BOUNDS.0, PANEL_HEIGHT_BOUNDS.0),
			"a size below the floor must clamp to the floor the drag also stops at"
		);
		// And the shipped panel is comfortably inside its own bounds, so the floor is
		// never itself the size the app opens at.
		assert!(PANEL_WIDTH > PANEL_WIDTH_BOUNDS.0 && PANEL_WIDTH < PANEL_WIDTH_BOUNDS.1);
		assert!(PANEL_HEIGHT > PANEL_HEIGHT_BOUNDS.0 && PANEL_HEIGHT < PANEL_HEIGHT_BOUNDS.1);
	}

	#[test]
	fn the_default_fits_a_work_area_too_small_for_the_declared_size() {
		// The reported case, in the shape it was reported: a 1920×1080 display at
		// 150%, whose work area is 1920×1040 physical once the taskbar is out. The
		// panel wants 760 × 1.5 = 1140 physical pixels of height, which is 100 more
		// than there is — so the old arithmetic centred it to a negative `y`, clamped
		// that to the top, and left the bottom under the taskbar.
		let work_area = MonitorRect {
			x: 0,
			y: 0,
			width: 1920,
			height: 1040,
		};
		for scale in [1.25, 1.5] {
			let placed = default_position(work_area, scale);
			let (width, height) = fitted_logical_size(work_area, scale);
			let right = i64::from(placed.x) + (width * scale).round() as i64;
			let bottom = i64::from(placed.y) + (height * scale).round() as i64;

			assert!(i64::from(placed.x) >= work_area.left(), "scale {scale}: {placed:?}");
			assert!(i64::from(placed.y) >= work_area.top(), "scale {scale}: {placed:?}");
			assert!(
				right <= work_area.right(),
				"scale {scale}: right edge {right} past {}",
				work_area.right()
			);
			assert!(
				bottom <= work_area.bottom(),
				"scale {scale}: bottom edge {bottom} past {} — under the taskbar",
				work_area.bottom()
			);
		}
	}

	#[test]
	fn a_work_area_that_fits_the_panel_leaves_its_size_alone() {
		// The fit is a cap, not a policy: the ordinary display must still get the
		// panel the design asks for, at every scale it can hold it at.
		let roomy = MonitorRect {
			x: 0,
			y: 0,
			width: 3840,
			height: 2160,
		};
		assert_eq!(fitted_logical_size(roomy, 2.0), (PANEL_WIDTH, PANEL_HEIGHT));
		assert_eq!(fitted_logical_size(PRIMARY, 1.0), (PANEL_WIDTH, PANEL_HEIGHT));

		// And a scale a display would never report cannot divide the size into an
		// infinity.
		assert_eq!(fitted_logical_size(PRIMARY, 0.0), (PANEL_WIDTH, PANEL_HEIGHT));
	}

	#[test]
	fn the_default_respects_a_monitor_that_does_not_start_at_the_origin() {
		let placed = default_position(SECOND, 1.0);
		assert!(placed.x >= SECOND.x);
		assert!(placed.x + PANEL_WIDTH as i32 <= SECOND.x + SECOND.width as i32);
	}
}
