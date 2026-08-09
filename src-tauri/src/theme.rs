//! The `theme` setting — `system`, `light` or `dark` — applied to both halves of
//! the window.
//!
//! Both halves is the point. Setting the webview's appearance alone leaves the
//! Mica/Acrylic backdrop following Windows, so a dark panel over a light system
//! sits in a light frame. The two are applied together here and nowhere else.

use std::sync::atomic::{AtomicU8, Ordering};

use tauri::{AppHandle, Manager, Theme, WebviewWindow};

use copper_core::store::settings::{Settings, SettingsPatch};
use crate::{diagnostics, panel, store, ShellError};

/// task-003 types `theme` as a bare `String`, deliberately — a hand-edited value
/// must not make `settings.json` unloadable — so validating it is this task's
/// job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Preference {
	System = 0,
	Light = 1,
	Dark = 2,
}

impl Preference {
	pub fn parse(value: &str) -> Option<Self> {
		match value {
			"system" => Some(Self::System),
			"light" => Some(Self::Light),
			"dark" => Some(Self::Dark),
			_ => None,
		}
	}

	fn from_code(code: u8) -> Self {
		match code {
			1 => Self::Light,
			2 => Self::Dark,
			_ => Self::System,
		}
	}

	/// `None` means "follow the OS", for both `set_theme` and `window-vibrancy`.
	fn native(self) -> Option<Theme> {
		match self {
			Self::System => None,
			Self::Light => Some(Theme::Light),
			Self::Dark => Some(Theme::Dark),
		}
	}

	fn dark(self) -> Option<bool> {
		match self {
			Self::System => None,
			Self::Light => Some(false),
			Self::Dark => Some(true),
		}
	}
}

/// The live preference, so the `ThemeChanged` handler knows whether it applies.
///
/// An atomic rather than managed state because the handler runs on the main
/// thread inside the window-event callback, and reading one byte there is
/// cheaper and simpler than resolving state through the manager.
static PREFERENCE: AtomicU8 = AtomicU8::new(Preference::System as u8);

fn current() -> Preference {
	Preference::from_code(PREFERENCE.load(Ordering::Relaxed))
}

/// The tint the backdrop is currently meant to have, in `window-vibrancy`'s own
/// vocabulary — `None` for "follow the OS".
///
/// Exported for `panel::set_translucency`, which changes the *material* and must
/// therefore re-apply the backdrop without changing its appearance. Reading the
/// preference back out of this module is what stops that call from having to
/// guess: passing `None` there would silently drop an explicit light or dark
/// choice every time the user toggled translucency.
pub fn backdrop_dark() -> Option<bool> {
	current().dark()
}

/// Applies a preference to the window, backdrop included.
fn apply(window: &WebviewWindow, preference: Preference) {
	PREFERENCE.store(preference as u8, Ordering::Relaxed);
	if let Err(err) = window.set_theme(preference.native()) {
		diagnostics::log_error(&format!("[copper] theme: could not set the window theme: {err}"));
	}
	if let Err(err) = panel::apply_effects(window, preference.dark()) {
		diagnostics::log_error(&format!("[copper] theme: could not re-tint the backdrop: {err}"));
	}
}

/// Startup. Never fails the launch: a theme that could not be applied costs the
/// user the right tint, and returning `Err` from `setup()` would cost them the
/// whole app.
pub fn install(app: &AppHandle, window: &WebviewWindow) {
	let stored = store::commands::settings(app).theme;
	let preference = Preference::parse(&stored).unwrap_or_else(|| {
		diagnostics::log_error(&format!(
			"[copper] theme: {stored:?} is not a theme; following the system instead"
		));
		Preference::System
	});
	apply(window, preference);
}

/// Re-tints the backdrop when Windows changes appearance.
///
/// `WindowEvent::ThemeChanged` fires **only** while the window is following the
/// system theme, which maps exactly onto the `system` preference — but the guard
/// stays, because an event arriving after an explicit choice would otherwise
/// undo it.
pub fn on_system_theme_changed(window: &WebviewWindow) {
	if current() != Preference::System {
		return;
	}
	if let Err(err) = panel::apply_effects(window, None) {
		diagnostics::log_error(&format!(
			"[copper] theme: could not re-tint the backdrop after a system change: {err}"
		));
	}
}

/// Applies first, persists second, and undoes the application if the write
/// fails.
///
/// Applying is instant and reversible; leaving the window dark while the file
/// says light is a contradiction the user meets again on the next launch, when
/// the file wins.
#[tauri::command]
pub async fn set_theme_preference(theme: String, app: AppHandle) -> Result<Settings, ShellError> {
	let Some(preference) = Preference::parse(&theme) else {
		return Err(ShellError::Invalid(format!(
			"{theme:?} is not a theme. Choose system, light or dark."
		)));
	};

	let Some(window) = app.get_webview_window(panel::PANEL_LABEL) else {
		return Err(ShellError::Invalid(
			"The panel window is not available.".to_owned(),
		));
	};

	let previous = current();
	apply(&window, preference);

	let patch = SettingsPatch {
		theme: Some(theme),
		..SettingsPatch::default()
	};
	match store::commands::patch_settings(&app, patch) {
		Ok(settings) => Ok(settings),
		Err(err) => {
			apply(&window, previous);
			Err(ShellError::Persist(format!(
				"Copper couldn't save the theme: {}",
				err.message()
			)))
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_the_three_named_themes_parse() {
		assert_eq!(Preference::parse("system"), Some(Preference::System));
		assert_eq!(Preference::parse("light"), Some(Preference::Light));
		assert_eq!(Preference::parse("dark"), Some(Preference::Dark));
		// Case-sensitive on purpose: these are wire values, not user input.
		assert_eq!(Preference::parse("Dark"), None);
		assert_eq!(Preference::parse(""), None);
		assert_eq!(Preference::parse("auto"), None);
	}

	#[test]
	fn the_shipped_default_parses() {
		// The store's default and this module's accepted set have to agree, or a
		// fresh install logs a complaint about its own default on every launch.
		assert!(Preference::parse(&Settings::default().theme).is_some());
	}

	#[test]
	fn system_means_follow_the_os_on_both_halves() {
		assert_eq!(Preference::System.native(), None);
		assert_eq!(Preference::System.dark(), None);
		// And the explicit choices force both, which is what task-002's hardcoded
		// `None` made impossible.
		assert_eq!(Preference::Dark.dark(), Some(true));
		assert_eq!(Preference::Light.dark(), Some(false));
	}

	#[test]
	fn the_atomic_round_trips_every_preference() {
		for preference in [Preference::System, Preference::Light, Preference::Dark] {
			assert_eq!(Preference::from_code(preference as u8), preference);
		}
		// An out-of-range byte can only come from a bug; following the system is the
		// answer that cannot look broken.
		assert_eq!(Preference::from_code(9), Preference::System);
	}
}
