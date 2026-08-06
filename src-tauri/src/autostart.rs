//! Launch at login.
//!
//! **The registry is the only source of truth**, and autostart is deliberately
//! absent from `settings.json`. Mirroring it there would create two states that
//! can disagree: a user who removes the Run entry with `msconfig` or a startup
//! manager would leave `settings.json` claiming autostart was on, with nothing
//! ever correcting it. The cost is one extra command call when the settings view
//! opens, which is why that view re-reads rather than trusting a cached value.
//!
//! What `is_enabled()` actually means, established by reading `auto-launch`
//! 0.5.0 rather than assumed: **Windows' effective startup approval**, not merely
//! the presence of a Run value. It requires the `Run` value *and* that
//! `Explorer\StartupApproved\Run` does not mark the entry disabled, so an entry
//! switched off in Task Manager reports `false` — which is the honest answer,
//! since nothing will launch.

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_autostart::ManagerExt;

use crate::{diagnostics, tray, ShellError};

/// Emitted **only** when the tray toggles autostart.
///
/// It cannot be folded into `settings-changed`: autostart is not in
/// `settings.json`, so a listener that responded by re-pulling `get_settings`
/// would learn nothing and an open settings view would keep showing the stale
/// toggle.
pub const AUTOSTART_CHANGED: &str = "autostart-changed";

#[derive(Serialize, Clone)]
struct AutostartChanged {
	enabled: bool,
}

fn read(app: &AppHandle) -> Result<bool, ShellError> {
	app.autolaunch().is_enabled().map_err(|err| {
		ShellError::Persist(format!(
			"Copper couldn't read whether it starts with Windows: {err}"
		))
	})
}

/// Writes the registry, then re-reads it.
///
/// The answer returned is what the registry now says, not what was asked for —
/// the registry is the source of truth, so reporting the request back would be
/// reporting an intention as a fact.
fn write(app: &AppHandle, enabled: bool) -> Result<bool, ShellError> {
	// Skipping a no-op write is not an optimisation: `disable()` deletes the Run
	// value, and deleting one that is not there fails. Asking for a state that
	// already holds must not be an error.
	if read(app)? != enabled {
		let result = if enabled {
			app.autolaunch().enable()
		} else {
			app.autolaunch().disable()
		};
		result.map_err(|err| {
			ShellError::Persist(format!(
				"Copper couldn't change whether it starts with Windows: {err}"
			))
		})?;
	}

	let actual = read(app)?;
	// Always, so a change made in the settings view reaches the tray checkmark.
	tray::report_autostart(app, actual);
	Ok(actual)
}

#[tauri::command]
pub async fn get_autostart_enabled(app: AppHandle) -> Result<bool, ShellError> {
	read(&app)
}

/// From the settings view. Returns the resulting state and **emits nothing** —
/// the caller already has the answer, and echoing it back could overwrite an
/// in-flight interaction.
#[tauri::command]
pub async fn set_autostart_enabled(enabled: bool, app: AppHandle) -> Result<bool, ShellError> {
	write(&app, enabled)
}

/// From the tray. The frontend did not initiate this, so an open settings view
/// has to be told.
pub fn toggle_from_tray(app: &AppHandle) {
	let current = read(app).unwrap_or(false);
	match write(app, !current) {
		Ok(enabled) => {
			if let Err(err) = app.emit(AUTOSTART_CHANGED, AutostartChanged { enabled }) {
				diagnostics::log_error(&format!(
					"[copper] autostart: could not announce the change: {err}"
				));
			}
		}
		Err(err) => diagnostics::log_error(&format!(
			"[copper] autostart: the tray toggle failed: {}",
			err.message()
		)),
	}
}

/// What the tray's checkmark should read at build time.
///
/// Failing to read the registry is not a reason to refuse to build a tray — the
/// tray is the recovery path — so an unreadable value shows as off and corrects
/// itself the first time the item is used.
pub fn initial_state(app: &AppHandle) -> bool {
	read(app).unwrap_or_else(|err| {
		diagnostics::log_error(&format!(
			"[copper] autostart: {} — the tray item starts unchecked",
			err.message()
		));
		false
	})
}
