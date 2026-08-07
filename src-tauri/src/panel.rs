//! The panel window as a unit: its label, its native backdrop and corner
//! rounding, and the reveal/hide pair every future call site must go through.
//!
//! This is the only module in the app that handles an `HWND`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::diagnostics;
use crate::store::settings::{PanelPosition, Settings, SettingsPatch};
use crate::{store, ShellError};
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewWindow};
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

/// The panel's fixed logical size, as declared in `tauri.conf.json`. The window
/// is `resizable: false`, so this cannot drift at runtime.
const PANEL_WIDTH: f64 = 390.0;
const PANEL_HEIGHT: f64 = 660.0;

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
/// The return type is deliberately `Box<dyn Error>` rather than `tauri::Result`:
/// this calls into `window_vibrancy` and `windows`, and neither
/// `window_vibrancy::Error` nor `windows::core::Error` has a `From` impl into
/// `tauri::Error`, so `?` would not compile. `Box<dyn Error>` is what `setup()`'s
/// closure already returns, so it propagates with no adapter.
pub fn apply_effects(
	window: &WebviewWindow,
	dark: Option<bool>,
) -> Result<(), Box<dyn std::error::Error>> {
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
	Ok(())
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
/// could not be reached. This is the tray's left-click behaviour, kept here so
/// that the window lookup stays in the module that owns the window.
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

/// The summon chord's behaviour: **three** states, not two.
///
/// A two-state toggle reads the middle case wrong. With the panel visible but
/// behind whatever the user is typing in, "toggle" hides a window they can see
/// and were reaching for — so they press again, and the second press finally
/// shows what the first should have. Visible-but-unfocused therefore raises
/// rather than hides, and only a panel that already has focus is hidden.
///
/// The hidden case is also where placement is checked, so a panel left on a
/// monitor that has since been unplugged comes back somewhere reachable instead
/// of being summoned into nowhere.
pub fn summon_or_log(app: &AppHandle) {
	with_panel(app, "summon", |window| {
		if is_visible(window) {
			if window.is_focused().unwrap_or(false) {
				return hide(window);
			}
			crate::capture::panel_revealed_by_user(app);
			return reveal(window);
		}

		crate::capture::panel_revealed_by_user(app);
		reveal_reachable(window)
	});
}

/// Whether the panel is currently visible, defaulting to `false` if it cannot be
/// determined — a failed query should not leave the tray toggle stuck.
fn is_visible(window: &WebviewWindow) -> bool {
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

/// Right-aligned with an inset, vertically centred.
///
/// A corner rather than the screen centre, because the panel is a side companion
/// to whatever the user is actually working in — centring would put it on top of
/// that.
fn default_position(monitor: MonitorRect, scale: f64) -> PanelPosition {
	let panel_width = (PANEL_WIDTH * scale).round() as i64;
	let panel_height = (PANEL_HEIGHT * scale).round() as i64;
	let inset = (DEFAULT_INSET * scale).round() as i64;

	let x = monitor.right() - panel_width - inset;
	let y = monitor.top() + (i64::from(monitor.height) - panel_height) / 2;
	PanelPosition {
		x: x.clamp(monitor.left(), monitor.right()) as i32,
		y: y.clamp(monitor.top(), monitor.bottom()) as i32,
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

/// The grab rectangle at the scale of whichever monitor the position lands on.
///
/// The saved position is physical, and on a 150% display the panel is half again
/// as wide as its logical size — so a logical-pixel grab rect would judge a
/// perfectly reachable position as lost.
fn grab_rect(window: &WebviewWindow, at: PanelPosition) -> GrabRect {
	let scale = window
		.monitor_from_point(f64::from(at.x), f64::from(at.y))
		.ok()
		.flatten()
		.or_else(|| current_monitor(window))
		.map_or(1.0, |monitor| monitor.scale_factor());
	GrabRect {
		width: (PANEL_WIDTH * scale).round() as i64,
		height: (HEADER_HEIGHT * scale).round() as i64,
	}
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

	const GRAB: GrabRect = GrabRect {
		width: 390,
		height: 48,
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

	const FALLBACK: PanelPosition = PanelPosition { x: 1506, y: 190 };

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

	#[test]
	fn the_shipped_default_is_pinned() {
		// The static's initial value, `tauri.conf.json`'s `alwaysOnTop` and the
		// store's default all have to agree, or the first launch runs with the window
		// in one band and the file naming the other.
		assert!(crate::store::settings::Settings::default().always_on_top);
	}

	#[test]
	fn the_default_respects_a_monitor_that_does_not_start_at_the_origin() {
		let placed = default_position(SECOND, 1.0);
		assert!(placed.x >= SECOND.x);
		assert!(placed.x + 390 <= SECOND.x + SECOND.width as i32);
	}
}
