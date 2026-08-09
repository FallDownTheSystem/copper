//! `settings.json` — app-private, app-owned, and the only file the store writes
//! that is not user data.
//!
//! That distinction drives the one asymmetry worth knowing about: a corrupt
//! space document is *refused* (it is irreplaceable and may be under version
//! control), while a corrupt `settings.json` is set aside and rebuilt from
//! defaults (spec 6.5). Losing a panel position is an inconvenience; losing
//! notes is not.
//!
//! Because that recovery discards the whole file, individual fields must not be
//! able to trigger it. `activeSpace` is therefore decoded as a signed integer
//! and clamped rather than modelled as `usize` (spec 6.1a) — otherwise a
//! hand-edited `-1` fails deserialisation and costs the user their entire
//! recents list. It is the same rule the model applies to timestamps: a field
//! the store can repair locally must never make the surrounding document
//! unloadable.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::atomic::{self, write_atomic, Attempt};
use super::error::{io_err, Result};
use super::format::{now_rfc3339, to_git_json};

/// Spec 6.4. Twenty is what fits a switcher without scrolling forever.
const MAX_RECENTS: usize = 20;

pub const FILE_NAME: &str = "settings.json";

/// The application identifier, and therefore the name of the directory holding
/// [`FILE_NAME`].
///
/// The same string as `identifier` in `src-tauri/tauri.conf.json`, because that
/// is what Tauri derives `app_config_dir()` from. It is declared here rather than
/// in the crate that has a Tauri context, because the crate that *lacks* one —
/// `copper-cli` — is the one that has to reconstruct the path by hand, and two
/// literals in two crates is how the app and the CLI would come to disagree about
/// where a user's settings live.
pub const APP_IDENTIFIER: &str = "io.github.falldownthesystem.copper";

/// `%APPDATA%\io.github.falldownthesystem.copper`, or `None` when the variable is
/// not set.
///
/// What Tauri's `app_config_dir()` answers for the app, spelled out for a process
/// with no Tauri runtime to ask. `None` rather than a guess: a missing `APPDATA`
/// means there is no roaming profile to write into, and inventing one would put
/// the CLI's view of the settings somewhere the app will never look.
pub fn default_config_dir() -> Option<PathBuf> {
	let roaming = std::env::var_os("APPDATA")?;
	if roaming.is_empty() {
		return None;
	}
	Some(PathBuf::from(roaming).join(APP_IDENTIFIER))
}

/// The panel's shipped size, and the same fact `panel::PANEL_WIDTH` and
/// `tauri.conf.json` each declare. `panel`'s unit tests tie the three together;
/// the numbers live here as well because a *stored* default has to be readable
/// without the window module.
const DEFAULT_PANEL_WIDTH: f64 = 440.0;
const DEFAULT_PANEL_HEIGHT: f64 = 760.0;

/// The usable range of each dial, as `(min, max)`.
///
/// A tuple rather than a `RangeInclusive` because `RangeInclusive::new` is not
/// const, and these have to be constants: they are the one declaration of the
/// bounds, read by the repair below *and* by `panel::set_panel_size` through
/// [`clamp_panel_size`], so a hand-edited file and a command cannot disagree
/// about what a legal size is.
///
/// The vibrancy floor is a true 0: the dial the frontend shows runs 0–100 with
/// zero meaning an achromatic accent, and that is a choice the dial offers
/// rather than a state to repair away. The stored value stays a chroma
/// *multiplier* — every number a 0.1.1 file wrote keeps its meaning — and the
/// ceiling of 3 is where the dial's 100 lands: past every hue's P3 ceiling at
/// the tokens' lightnesses, so the top of the scale is bounded by the display's
/// gamut mapping rather than by an arbitrary cap here.
const VIBRANCY_BOUNDS: (f64, f64) = (0.0, 3.0);
/// Narrow enough that the composer's line length stays sane, wide enough for a
/// panel used as a primary window. The floor is also the drag floor
/// `panel::apply_resizable` installs, by construction rather than by coincidence.
pub const PANEL_WIDTH_BOUNDS: (f64, f64) = (360.0, 1200.0);
pub const PANEL_HEIGHT_BOUNDS: (f64, f64) = (480.0, 1600.0);

/// Serialize only. Reading goes through [`RawSettings`], which repairs field by
/// field rather than failing the whole file over one bad value.
///
/// `Eq` is deliberately absent where the rest of the store's shapes have it: the
/// size and vibrancy keys are `f64`, which is not `Eq`. Nothing here asks for
/// more than "did this change" — the store's write guard — so `PartialEq` is the
/// whole requirement, and deriving it by hand to keep `Eq` would be inventing a
/// total order over settings nobody needs.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
	pub recents: Vec<String>,
	pub active_space: usize,
	pub panel_position: Option<PanelPosition>,
	pub shortcuts: Shortcuts,
	pub theme: String,
	pub sounds: bool,
	pub motion: String,
	pub insertion_point: String,
	pub double_click: String,
	pub always_on_top: bool,
	pub show_created: bool,
	pub capture_notifications: bool,
	pub link_previews: bool,
	pub translucent: bool,
	pub neutral: String,
	pub accent: String,
	pub vibrancy: f64,
	pub resizable: bool,
	pub panel_width: f64,
	pub panel_height: f64,
}

/// Where a fresh note goes inside its section.
///
/// Narrowed from the stored string by name rather than deserialised as an enum,
/// the same split `theme` and `motion` use: the store repairs a wrong *type*, and
/// a value of the right type that names nothing collapses to the default on read.
/// Modelling it as a serde enum would make one hand-edited word quarantine the
/// whole file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InsertionPoint {
	#[default]
	Bottom,
	Top,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelPosition {
	pub x: i32,
	pub y: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Shortcuts {
	pub capture: String,
	pub summon: String,
}

impl Default for Settings {
	fn default() -> Self {
		Self {
			recents: Vec::new(),
			active_space: 0,
			panel_position: None,
			shortcuts: Shortcuts::default(),
			theme: "system".to_string(),
			// Capture is silent on success by design, and that decision is
			// preserved by shipping sound off rather than by leaving the tick
			// unimplemented. Turning this on by default would be a change to that
			// decision and should be made as one.
			sounds: false,
			motion: "auto".to_string(),
			// Appending is what every earlier build did, so the default is the
			// behaviour a user upgrading into this feature already has.
			insertion_point: "bottom".to_string(),
			double_click: "copy".to_string(),
			// The band the window was born in (`alwaysOnTop` in `tauri.conf.json`).
			// Defaulting to `false` would change what every existing install does on
			// its first launch after an upgrade, over a setting nobody had asked for.
			always_on_top: true,
			// Off, so an upgrade shows exactly the cards it showed before. The
			// timestamp itself has been recorded on every note since task-003 — only
			// its display is new — so turning this on reveals real history rather
			// than starting to collect it.
			show_created: false,
			// On, and the opposite way round to `sounds` on purpose. A capture that
			// lands while the panel is hidden produces nothing the user can see —
			// that is the whole point of a global capture — so without this they have
			// no confirmation at all that the double-tap did anything. `sounds` ships
			// off because it adds noise to a path that already has a surface; this
			// ships on because it *is* the surface.
			capture_notifications: true,
			// Off, and this is the one default in the file that is not about
			// preserving what an earlier build did — there was no earlier behaviour to
			// preserve. It ships off because turning it on is the only setting in
			// Copper whose "on" position sends anything to a third party: every fetch
			// tells whoever runs that host the URL, the reader's IP address and the
			// moment the note was read. A default that quietly starts doing that to
			// URLs the user pasted from somewhere private is not a default anyone can
			// consent to in advance.
			link_previews: false,
			// Off, so an upgrade shows exactly the panel every existing install
			// already shows. The window has always carried a Mica backdrop, but
			// `--surface` sits at 90% over it, so what the user has seen is a nearly
			// solid panel; turning this on swaps Mica for Acrylic and thins that
			// surface until the desktop blurs through. That is a different-looking
			// app, and nobody should meet it because they updated.
			translucent: false,
			// The panel's own warm grey and its own copper, named rather than left
			// blank so that "the shipped look" is a value the picker can select
			// rather than the absence of one. Both are narrowed on the frontend, so
			// a name nothing recognises renders as these two anyway.
			neutral: "warm".to_string(),
			accent: "copper".to_string(),
			// One, meaning the palette exactly as shipped. A multiplier rather than a
			// flag because "more colourful" has no single right amount, and a
			// multiplier's neutral position is the one number that changes nothing —
			// which is what an upgraded install has to render.
			vibrancy: 1.0,
			// Off, matching `resizable: false` in `tauri.conf.json`. The relationship
			// is `always_on_top`'s: the config declares what the window is *born*
			// with, this is the stored preference that may contradict it, and the
			// setting exists to let the user opt in rather than to change what an
			// upgraded install does before they touch it.
			resizable: false,
			// The size every build so far has shipped. What is stored is the panel's
			// **default** size, not its current one — a manual drag is session-only by
			// design, see the ruling on `set_panel_size` — so these move only when the
			// settings view writes them.
			panel_width: DEFAULT_PANEL_WIDTH,
			panel_height: DEFAULT_PANEL_HEIGHT,
		}
	}
}

/// Brings a size inside [`PANEL_WIDTH_BOUNDS`] / [`PANEL_HEIGHT_BOUNDS`].
///
/// Shared with `panel::set_panel_size` so the command and the repair below
/// cannot drift: a size the file would have been corrected for must not be
/// reachable by asking for it politely.
///
/// A non-finite value falls back to the default rather than clamping. `f64::clamp`
/// returns NaN for a NaN input rather than a bound, so clamping alone would let an
/// infinity or a NaN straight through to `set_size`; JSON cannot carry either, but
/// the command's parameters are `f64` and arrive from JavaScript, where both are
/// ordinary numbers.
pub fn clamp_panel_size(width: f64, height: f64) -> (f64, f64) {
	(
		clamp_or_default(width, PANEL_WIDTH_BOUNDS, DEFAULT_PANEL_WIDTH),
		clamp_or_default(height, PANEL_HEIGHT_BOUNDS, DEFAULT_PANEL_HEIGHT),
	)
}

fn clamp_or_default(value: f64, (min, max): (f64, f64), default: f64) -> f64 {
	if value.is_finite() {
		value.clamp(min, max)
	} else {
		default
	}
}

impl Default for Shortcuts {
	fn default() -> Self {
		Self {
			capture: "Shift Shift".to_string(),
			summon: "Ctrl+Shift+Space".to_string(),
		}
	}
}

/// Every field as raw JSON, so no single bad value can fail the parse.
///
/// Spec 6.1a's principle — a field the store can repair locally must never be
/// able to make the surrounding document unloadable — was written about
/// `activeSpace`, but nothing in the reasoning is specific to that field. A
/// wrong-typed `theme`, a `panelPosition` missing its `y`, or one non-string
/// entry in `recents` would otherwise send the whole file to the quarantine
/// path and cost the user their entire recents list. Only JSON that is not JSON
/// at all reaches quarantine now.
///
/// `shortcuts` is the case that will bite in Phase 7, when the settings view
/// starts writing chords the user typed.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawSettings {
	recents: Value,
	active_space: Value,
	panel_position: Value,
	shortcuts: Value,
	theme: Value,
	sounds: Value,
	motion: Value,
	insertion_point: Value,
	double_click: Value,
	always_on_top: Value,
	show_created: Value,
	capture_notifications: Value,
	link_previews: Value,
	translucent: Value,
	neutral: Value,
	accent: Value,
	vibrancy: Value,
	resizable: Value,
	panel_width: Value,
	panel_height: Value,
}

impl RawSettings {
	/// Returns the repaired settings and one notice per field that needed it.
	fn repair(self) -> (Settings, Vec<String>) {
		let defaults = Settings::default();
		let mut notices = Vec::new();

		let recents = match self.recents {
			Value::Null => defaults.recents,
			Value::Array(entries) => {
				let found = entries.len();
				let paths: Vec<String> = entries
					.into_iter()
					.filter_map(|entry| match entry {
						Value::String(path) => Some(path),
						_ => None,
					})
					.collect();
				if paths.len() != found {
					notices.push(format!(
						"{} of the {found} entries in \"recents\" were not paths and have been \
						 dropped.",
						found - paths.len()
					));
				}
				paths
			}
			_ => {
				notices.push("\"recents\" was not a list of paths and has been emptied.".into());
				defaults.recents
			}
		};

		// Out of range clamps silently, per spec 6.1a — the value is the right
		// kind of thing and just points nowhere. Only a value that is not a number
		// at all is worth telling the user about.
		let active_space = match &self.active_space {
			Value::Null => 0,
			Value::Number(number) => number
				.as_i64()
				.map_or(0, |index| if index < 0 { 0 } else { index as usize }),
			_ => {
				notices.push(
					"\"activeSpace\" was not a number; the first space has been made active."
						.into(),
				);
				0
			}
		};

		let panel_position = match self.panel_position {
			Value::Null => None,
			raw => match serde_json::from_value::<PanelPosition>(raw) {
				Ok(position) => Some(position),
				Err(_) => {
					notices.push(
						"\"panelPosition\" was not a point; the panel will open in its default \
						 position."
							.into(),
					);
					None
				}
			},
		};

		let shortcuts = repair_shortcuts(self.shortcuts, &mut notices);

		let theme = repair_named(self.theme, "theme", defaults.theme, &mut notices);

		let sounds = repair_flag(self.sounds, "sounds", defaults.sounds, &mut notices);
		let always_on_top = repair_flag(
			self.always_on_top,
			"alwaysOnTop",
			defaults.always_on_top,
			&mut notices,
		);
		let show_created = repair_flag(
			self.show_created,
			"showCreated",
			defaults.show_created,
			&mut notices,
		);
		let capture_notifications = repair_flag(
			self.capture_notifications,
			"captureNotifications",
			defaults.capture_notifications,
			&mut notices,
		);
		// Repaired to `false` like every other unreadable value here, and that
		// direction is not incidental: a `"linkPreviews": "yes"` someone hand-edited
		// must not be read as consent to start fetching.
		let link_previews = repair_flag(
			self.link_previews,
			"linkPreviews",
			defaults.link_previews,
			&mut notices,
		);

		let translucent = repair_flag(
			self.translucent,
			"translucent",
			defaults.translucent,
			&mut notices,
		);

		let motion = repair_named(self.motion, "motion", defaults.motion, &mut notices);
		let insertion_point = repair_named(
			self.insertion_point,
			"insertionPoint",
			defaults.insertion_point,
			&mut notices,
		);
		let double_click = repair_named(
			self.double_click,
			"doubleClick",
			defaults.double_click,
			&mut notices,
		);
		let neutral = repair_named(self.neutral, "neutral", defaults.neutral, &mut notices);
		let accent = repair_named(self.accent, "accent", defaults.accent, &mut notices);

		let resizable = repair_flag(
			self.resizable,
			"resizable",
			defaults.resizable,
			&mut notices,
		);

		let vibrancy = repair_number(
			self.vibrancy,
			"vibrancy",
			defaults.vibrancy,
			VIBRANCY_BOUNDS,
			&mut notices,
		);
		let panel_width = repair_number(
			self.panel_width,
			"panelWidth",
			defaults.panel_width,
			PANEL_WIDTH_BOUNDS,
			&mut notices,
		);
		let panel_height = repair_number(
			self.panel_height,
			"panelHeight",
			defaults.panel_height,
			PANEL_HEIGHT_BOUNDS,
			&mut notices,
		);

		let mut settings = Settings {
			recents,
			active_space,
			panel_position,
			shortcuts,
			theme,
			sounds,
			motion,
			insertion_point,
			double_click,
			always_on_top,
			show_created,
			capture_notifications,
			link_previews,
			translucent,
			neutral,
			accent,
			vibrancy,
			resizable,
			panel_width,
			panel_height,
		};
		settings.clamp();
		(settings, notices)
	}
}

/// A preference stored as a bare name and narrowed on the frontend — `theme`,
/// `motion`, `insertionPoint`, `doubleClick`, `neutral`, `accent`.
///
/// Only the *type* is repaired here. A `Value::Null` is the ordinary case for a
/// key written before its feature existed and must stay silent, or an older file
/// looks broken; a string that names nothing collapses to the default wherever it
/// is read, so it is not damage either.
fn repair_named(raw: Value, key: &str, default: String, notices: &mut Vec<String>) -> String {
	match raw {
		Value::Null => default,
		Value::String(name) => name,
		_ => {
			notices.push(format!(
				"\"{key}\" was not a name and has been reset to \"{default}\"."
			));
			default
		}
	}
}

/// A preference stored as a number with a usable range — `vibrancy`,
/// `panelWidth`, `panelHeight`.
///
/// The one repair here that goes further than [`repair_named`] and [`repair_flag`]
/// do, and the reason is that nothing downstream narrows these. A palette name
/// nothing recognises collapses to the shipped value wherever it is read, so it is
/// not damage; a `panelWidth` of 40000 is applied to the window exactly as written
/// and puts the panel where it cannot be used. So an out-of-range number is
/// clamped **and reported** rather than either kept or silently corrected — the
/// user asked for a size they did not get, and the notice is the only place that
/// can say so.
///
/// The `Value::Null` arm carries the same weight it does in the other two: an
/// absent key is the ordinary case for a file written before the feature existed,
/// and a notice there would make it look damaged.
fn repair_number(
	raw: Value,
	key: &str,
	default: f64,
	(min, max): (f64, f64),
	notices: &mut Vec<String>,
) -> f64 {
	let value = match raw {
		Value::Null => return default,
		Value::Number(number) => number.as_f64().filter(|value| value.is_finite()),
		_ => None,
	};
	let Some(value) = value else {
		notices.push(format!(
			"\"{key}\" was not a number and has been reset to {default}."
		));
		return default;
	};
	if value < min || value > max {
		let clamped = value.clamp(min, max);
		notices.push(format!(
			"\"{key}\" was outside {min}–{max} and has been brought back to {clamped}."
		));
		return clamped;
	}
	value
}

/// A preference stored as a plain boolean — `sounds`, `alwaysOnTop`,
/// `translucent`, `resizable`.
///
/// The `Value::Null` arm carries the same weight it does in [`repair_named`]: it
/// is the ordinary case for a key written before its feature existed, and a
/// notice there would make an older `settings.json` look damaged when the
/// recovery path discards the whole file.
fn repair_flag(raw: Value, key: &str, default: bool, notices: &mut Vec<String>) -> bool {
	match raw {
		Value::Null => default,
		Value::Bool(on) => on,
		_ => {
			notices.push(format!(
				"\"{key}\" was not true or false and has been reset to {default}."
			));
			default
		}
	}
}

/// Repaired one chord at a time, so a bad `capture` does not also cost the user
/// their `summon`.
fn repair_shortcuts(raw: Value, notices: &mut Vec<String>) -> Shortcuts {
	let mut shortcuts = Shortcuts::default();
	let map = match raw {
		Value::Null => return shortcuts,
		Value::Object(map) => map,
		_ => {
			notices
				.push("\"shortcuts\" was not a pair of chords; the defaults have been restored."
					.into());
			return shortcuts;
		}
	};

	for (key, slot) in [
		("capture", &mut shortcuts.capture),
		("summon", &mut shortcuts.summon),
	] {
		match map.get(key) {
			None | Some(Value::Null) => {}
			Some(Value::String(chord)) => slot.clone_from(chord),
			Some(_) => notices.push(format!(
				"\"shortcuts.{key}\" was not a chord; the default has been restored."
			)),
		}
	}
	shortcuts
}

impl Settings {
	/// Puts `active_space` back in range. Called after every load and after any
	/// change to `recents`.
	fn clamp(&mut self) {
		if self.active_space >= self.recents.len() {
			self.active_space = 0;
		}
	}

	/// Where the next note goes.
	///
	/// Read on the store side rather than taken as a command parameter, which is
	/// what keeps the capture path — whose only caller is Rust — consistent with
	/// the composer's without a second place to remember.
	pub fn insertion(&self) -> InsertionPoint {
		if self.insertion_point == "top" {
			InsertionPoint::Top
		} else {
			InsertionPoint::Bottom
		}
	}

	/// The path `active_space` points at, if there is one.
	pub fn active_recent(&self) -> Option<&str> {
		self.recents.get(self.active_space).map(String::as_str)
	}

	/// Moves `path` to the front of `recents` and makes it active (spec 6.4).
	///
	/// `path` must already be canonicalised with the verbatim prefix stripped —
	/// see `store::canonical`.
	pub fn touch_recent(&mut self, path: &str) {
		self.recents.retain(|entry| !same_path(entry, path));
		self.recents.insert(0, path.to_string());
		self.recents.truncate(MAX_RECENTS);
		self.active_space = 0;
	}

	/// Drops `path` from `recents`.
	///
	/// Leaves `active_space` alone — the caller knows which space is actually
	/// open and re-points it (spec 6.7).
	pub fn forget_recent(&mut self, path: &str) {
		self.recents.retain(|entry| !same_path(entry, path));
	}

	/// Re-points `active_space` at `path`, or clamps to 0 when it is gone.
	pub fn point_at(&mut self, path: Option<&str>) {
		self.active_space = path
			.and_then(|path| {
				self.recents
					.iter()
					.position(|entry| same_path(entry, path))
			})
			.unwrap_or(0);
		self.clamp();
	}

	pub fn apply_patch(&mut self, patch: SettingsPatch) {
		// Three cases, not two: absent leaves the position alone, an explicit null
		// clears it, an object sets it (spec 6.3a). A panel restored onto a monitor
		// that is no longer attached needs the clearing case to exist.
		if let Some(position) = patch.panel_position {
			self.panel_position = position;
		}
		if let Some(shortcuts) = patch.shortcuts {
			self.shortcuts = shortcuts;
		}
		if let Some(theme) = patch.theme {
			self.theme = theme;
		}
		if let Some(sounds) = patch.sounds {
			self.sounds = sounds;
		}
		if let Some(motion) = patch.motion {
			self.motion = motion;
		}
		if let Some(insertion_point) = patch.insertion_point {
			self.insertion_point = insertion_point;
		}
		if let Some(double_click) = patch.double_click {
			self.double_click = double_click;
		}
		if let Some(always_on_top) = patch.always_on_top {
			self.always_on_top = always_on_top;
		}
		if let Some(show_created) = patch.show_created {
			self.show_created = show_created;
		}
		if let Some(capture_notifications) = patch.capture_notifications {
			self.capture_notifications = capture_notifications;
		}
		if let Some(link_previews) = patch.link_previews {
			self.link_previews = link_previews;
		}
		if let Some(translucent) = patch.translucent {
			self.translucent = translucent;
		}
		if let Some(neutral) = patch.neutral {
			self.neutral = neutral;
		}
		if let Some(accent) = patch.accent {
			self.accent = accent;
		}
		if let Some(vibrancy) = patch.vibrancy {
			self.vibrancy = vibrancy;
		}
		if let Some(resizable) = patch.resizable {
			self.resizable = resizable;
		}
		// Written as sent, unclamped, exactly as `theme` and the palette names are.
		// The bounds are enforced where a value can actually reach the window — on the
		// way in through [`RawSettings::repair`], and on the way out through
		// `panel::set_panel_size`, which is the only caller that resizes anything. A
		// clamp here as well would silently rewrite what the settings view asked for
		// without a notice, which is the one thing the repair path is careful not to do.
		if let Some(panel_width) = patch.panel_width {
			self.panel_width = panel_width;
		}
		if let Some(panel_height) = patch.panel_height {
			self.panel_height = panel_height;
		}
	}
}

/// `recents` and `active_space` are deliberately absent: they change only as a
/// consequence of opening a space or of an explicit `remove_recent` (spec 6.3).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
	#[serde(default, deserialize_with = "deserialise_double_option")]
	pub panel_position: Option<Option<PanelPosition>>,
	#[serde(default)]
	pub shortcuts: Option<Shortcuts>,
	#[serde(default)]
	pub theme: Option<String>,
	#[serde(default)]
	pub sounds: Option<bool>,
	#[serde(default)]
	pub motion: Option<String>,
	#[serde(default)]
	pub insertion_point: Option<String>,
	#[serde(default)]
	pub double_click: Option<String>,
	#[serde(default)]
	pub always_on_top: Option<bool>,
	#[serde(default)]
	pub show_created: Option<bool>,
	#[serde(default)]
	pub capture_notifications: Option<bool>,
	#[serde(default)]
	pub link_previews: Option<bool>,
	/// Reachable through the patch as well as through `set_translucency`, which is
	/// the arrangement `always_on_top` already has: the dedicated command owns the
	/// window half, and the key still has to exist here or `apply_patch` could not
	/// restore it when that command's own write fails.
	#[serde(default)]
	pub translucent: Option<bool>,
	#[serde(default)]
	pub neutral: Option<String>,
	#[serde(default)]
	pub accent: Option<String>,
	/// The only one of the four with no Rust side at all: it is a chroma multiplier
	/// the frontend applies to its own palette, so the ordinary patch path is the
	/// whole feature and a dedicated command would have nothing to do.
	#[serde(default)]
	pub vibrancy: Option<f64>,
	/// Reachable through the patch as well as through the dedicated command, the
	/// arrangement `translucent` and `always_on_top` already have: the command owns
	/// the window half, and the key still has to exist here or [`Settings::apply_patch`]
	/// could not restore it when that command's own write fails.
	#[serde(default)]
	pub resizable: Option<bool>,
	#[serde(default)]
	pub panel_width: Option<f64>,
	#[serde(default)]
	pub panel_height: Option<f64>,
}

/// Distinguishes "key absent" from "key present and null".
///
/// A plain `Option<T>` collapses those two into `None`, which would make the
/// panel position unclearable.
fn deserialise_double_option<'de, D, T>(
	deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error>
where
	D: Deserializer<'de>,
	T: Deserialize<'de>,
{
	Option::deserialize(deserializer).map(Some)
}

/// Windows paths are case-insensitive, so recents dedupe must be too.
///
/// ASCII-only folding: the cases this has to catch are drive letters and
/// user-typed path segments, and full Unicode case folding would need a
/// dependency to do correctly rather than approximately.
fn same_path(a: &str, b: &str) -> bool {
	a.eq_ignore_ascii_case(b)
}

/// What state the file was left in — and therefore whether writing the returned
/// settings back over it is safe.
///
/// This distinction is the whole point of the type. "Recovered from a corrupt
/// file" and "gave up on an unreadable file" produce identical settings and an
/// equally alarming notice, but only the first has moved the original out of
/// harm's way. Without a way to tell them apart, a caller that saves after a
/// notice destroys the very file it was warning about — which is exactly what
/// happened here before this type existed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
	/// Read and understood. Saving overwrites data Copper produced itself.
	Loaded,
	/// There was no file. Saving creates it.
	Absent,
	/// The file was unusable and has been renamed out of the way.
	Quarantined,
	/// The file is unusable and is **still there**. Saving would destroy it.
	Retained,
}

impl Origin {
	/// Whether the returned settings may be written back to the same path.
	pub fn may_overwrite(self) -> bool {
		!matches!(self, Self::Retained)
	}

	/// Whether a file has to be written even if nothing has changed.
	pub fn needs_a_file(self) -> bool {
		matches!(self, Self::Absent | Self::Quarantined)
	}
}

pub struct LoadedSettings {
	pub settings: Settings,
	/// For `get_status` to surface on the frontend's mount-time pull.
	pub notice: Option<String>,
	pub origin: Origin,
}

impl LoadedSettings {
	fn defaults(origin: Origin, notice: Option<String>) -> Self {
		Self {
			settings: Settings::default(),
			notice,
			origin,
		}
	}
}

/// Reads `settings.json`, recovering rather than failing — but never at the cost
/// of the file it is recovering from.
///
/// It never emits: this runs during startup, where nothing is listening yet
/// (spec 8A.2), so the reason has to be *recorded* to reach the user at all.
///
/// **This is not a pure read.** An unusable file is renamed out of the way, which
/// is the right answer for the app — it is what lets the next save succeed — and
/// the wrong answer for anything merely *listing* what is in there. That caller
/// wants [`load_read_only`].
pub fn load(path: &Path) -> LoadedSettings {
	match decode(path) {
		Ok(loaded) => loaded,
		Err(why) => set_aside(path, &why),
	}
}

/// [`load`] with the quarantine step removed: reads and decodes, and touches
/// nothing.
///
/// The CLI's whole read surface goes through this. `copper space list` is a
/// listing command, and a listing command that renames the user's `settings.json`
/// because it happens to hold a stray comma would be destroying data it was only
/// asked to print — while the app, which never ran, is the only thing that could
/// have told them.
///
/// An unusable file therefore reports [`Origin::Retained`], which already means
/// exactly this: the file is still there and writing over it would destroy it.
/// The caller gets defaults and a notice naming the reason, and the file keeps
/// both its name and its bytes.
pub fn load_read_only(path: &Path) -> LoadedSettings {
	match decode(path) {
		Ok(loaded) => loaded,
		Err(why) => LoadedSettings::defaults(
			Origin::Retained,
			Some(format!(
				"{} could not be used because {why}. It has been left untouched.",
				path.display()
			)),
		),
	}
}

/// The read-and-decode half both loaders share.
///
/// `Err(why)` is the one outcome they answer differently — a file that is present
/// and readable and still not settings — so it is the only thing this returns as
/// an error. Everything else (absent, held open by something else, present and
/// repairable) has one answer, and it is side-effect free in both.
fn decode(path: &Path) -> std::result::Result<LoadedSettings, String> {
	let bytes = match read_with_retry(path) {
		Ok(Some(bytes)) => bytes,
		Ok(None) => return Ok(LoadedSettings::defaults(Origin::Absent, None)),
		Err(err) => {
			// Present but unreadable. Defaults let Copper start; leaving the file
			// alone lets the user get their recents list back once whatever is
			// holding it lets go.
			return Ok(LoadedSettings::defaults(
				Origin::Retained,
				Some(format!(
					"{} could not be read ({err}). Copper started from default settings and left \
					 the file untouched.",
					path.display()
				)),
			));
		}
	};

	// Read as bytes and decoded here rather than through `read_to_string`, so
	// invalid UTF-8 lands on the quarantine path with every other unusable file
	// instead of being mistaken for an I/O failure.
	let text = match String::from_utf8(bytes) {
		Ok(text) => text,
		Err(err) => return Err(format!("it is not valid UTF-8 ({err})")),
	};

	match serde_json::from_str::<RawSettings>(&text) {
		Ok(raw) => {
			let (settings, notices) = raw.repair();
			let notice = (!notices.is_empty()).then(|| {
				format!(
					"{} held values Copper could not use:\n{}",
					path.display(),
					notices.join("\n")
				)
			});
			Ok(LoadedSettings {
				settings,
				notice,
				origin: Origin::Loaded,
			})
		}
		Err(err) => Err(format!("it is not valid JSON ({err})")),
	}
}

/// Reads the file, retrying a sharing violation.
///
/// `Ok(None)` means there is no file, which is the ordinary first-run case and
/// not a failure. The retry matters because startup is exactly when a transient
/// lock is likeliest — antivirus, the search indexer and OneDrive all wake with
/// everything else — and without it a file held for 50 ms would be reported to
/// the user as unreadable settings.
fn read_with_retry(path: &Path) -> Result<Option<Vec<u8>>> {
	atomic::with_backoff(|| match std::fs::read(path) {
		Ok(bytes) => Attempt::Done(Some(bytes)),
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Attempt::Done(None),
		Err(err) if atomic::is_sharing_violation(&err) => {
			Attempt::Transient(io_err(path, "read", &err))
		}
		Err(err) => Attempt::Failed(io_err(path, "read", &err)),
	})
}

fn set_aside(path: &Path, why: &str) -> LoadedSettings {
	match quarantine(path) {
		Ok(kept) => LoadedSettings::defaults(
			Origin::Quarantined,
			Some(format!(
				"{} could not be used because {why}. It has been kept as {} and Copper started \
				 from default settings.",
				path.display(),
				kept.display()
			)),
		),
		Err(err) => LoadedSettings::defaults(
			Origin::Retained,
			Some(format!(
				"{} could not be used because {why}, and it could not be set aside ({err}). \
				 Copper started from default settings and left the file untouched.",
				path.display()
			)),
		),
	}
}

/// Renames the unreadable file out of the way so the next save has a clear path.
fn quarantine(path: &Path) -> Result<PathBuf> {
	// Colons are legal in RFC3339 and illegal in a Windows filename.
	let stamp = now_rfc3339().replace(':', "-");
	let mut name = path.file_name().unwrap_or_default().to_os_string();
	name.push(format!(".corrupt-{stamp}"));
	let target = path.with_file_name(name);
	std::fs::rename(path, &target).map_err(|err| io_err(path, "set aside", &err))?;
	Ok(target)
}

pub fn save(path: &Path, settings: &Settings) -> Result<()> {
	write_atomic(path, &to_git_json(settings)?)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn write(dir: &Path, body: &str) -> PathBuf {
		let path = dir.join(FILE_NAME);
		std::fs::write(&path, body).unwrap();
		path
	}

	fn siblings(dir: &Path) -> Vec<String> {
		let mut names: Vec<String> = std::fs::read_dir(dir)
			.unwrap()
			.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
			.collect();
		names.sort();
		names
	}

	#[test]
	fn defaults_match_the_specified_shape() {
		let text = to_git_json(&Settings::default()).unwrap();
		assert_eq!(
			text,
			"{\n  \"recents\": [],\n  \"activeSpace\": 0,\n  \"panelPosition\": null,\n  \
			 \"shortcuts\": {\n    \"capture\": \"Shift Shift\",\n    \"summon\": \
			 \"Ctrl+Shift+Space\"\n  },\n  \"theme\": \"system\",\n  \"sounds\": false,\n  \
			 \"motion\": \"auto\",\n  \"insertionPoint\": \"bottom\",\n  \"doubleClick\": \
			 \"copy\",\n  \"alwaysOnTop\": true,\n  \"showCreated\": false,\n  \
			 \"captureNotifications\": true,\n  \"linkPreviews\": false,\n  \"translucent\": \
			 false,\n  \"neutral\": \"warm\",\n  \"accent\": \"copper\",\n  \"vibrancy\": 1.0,\n  \
			 \"resizable\": false,\n  \"panelWidth\": 440.0,\n  \"panelHeight\": 760.0\n}\n"
		);
	}

	/// The three numbers are `f64`, so the pin above is also a claim about how they
	/// are *spelled* on disk. `440.0` rather than `440` is what serde writes and what
	/// every existing `settings.json` will therefore contain; a future change that
	/// made them integers would rewrite the file for every user on first launch and
	/// break the pin in a way that is easy to "fix" by editing the expected string.
	#[test]
	fn the_numeric_keys_are_written_as_floats() {
		let text = to_git_json(&Settings::default()).unwrap();
		assert!(text.contains("\"vibrancy\": 1.0"), "{text}");
		assert!(text.contains("\"panelWidth\": 440.0"), "{text}");
		assert!(text.contains("\"panelHeight\": 760.0"), "{text}");
	}

	/// The window round's four keys join the guarantee every key added since
	/// task-012 holds: a `settings.json` written by an earlier build has none of
	/// them, and reading one must be indistinguishable from reading a current file.
	/// The visible cost of getting these backwards is a panel that comes back a
	/// different size, or one that has suddenly grown a drag handle, because the
	/// user updated.
	#[test]
	fn a_file_without_the_window_keys_reads_as_the_shipped_panel() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark","translucent":true}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.notice.is_none(), "absence was reported as damage: {:?}", loaded.notice);
		assert_eq!(siblings(dir.path()), [FILE_NAME], "the file was set aside");
		assert_eq!(loaded.settings.vibrancy, 1.0, "an absent key must not shift the palette");
		assert!(!loaded.settings.resizable, "an absent key must not make the panel draggable");
		assert_eq!(loaded.settings.panel_width, DEFAULT_PANEL_WIDTH);
		assert_eq!(loaded.settings.panel_height, DEFAULT_PANEL_HEIGHT);
		// The rest of the file survived rather than being defaulted alongside them.
		assert_eq!(loaded.settings.theme, "dark");
		assert!(loaded.settings.translucent);
	}

	#[test]
	fn wrong_typed_window_values_are_repaired_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(
			dir.path(),
			r#"{"vibrancy":"lots","resizable":"sure","panelWidth":"wide","panelHeight":[760]}"#,
		);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert_eq!(loaded.settings.vibrancy, 1.0);
		assert!(!loaded.settings.resizable);
		assert_eq!(loaded.settings.panel_width, DEFAULT_PANEL_WIDTH);
		assert_eq!(loaded.settings.panel_height, DEFAULT_PANEL_HEIGHT);
		let notice = loaded.notice.expect("repairs must be reported");
		for expected in ["vibrancy", "resizable", "panelWidth", "panelHeight"] {
			assert!(notice.contains(expected), "{expected} unreported in: {notice}");
		}
	}

	/// The half that separates these keys from `neutral` and `accent`: a number of
	/// the right type that is out of range is *not* narrowed anywhere downstream, so
	/// it has to be corrected here — and reported, because the user asked for a size
	/// they are not going to get.
	#[test]
	fn out_of_range_window_values_are_clamped_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(
			dir.path(),
			r#"{"vibrancy":9,"panelWidth":40000,"panelHeight":10}"#,
		);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert_eq!(loaded.settings.vibrancy, VIBRANCY_BOUNDS.1);
		assert_eq!(loaded.settings.panel_width, PANEL_WIDTH_BOUNDS.1);
		assert_eq!(loaded.settings.panel_height, PANEL_HEIGHT_BOUNDS.0);
		let notice = loaded.notice.expect("clamps must be reported");
		for expected in ["vibrancy", "panelWidth", "panelHeight"] {
			assert!(notice.contains(expected), "{expected} unreported in: {notice}");
		}
	}

	#[test]
	fn in_range_window_values_survive_a_load_unreported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(
			dir.path(),
			r#"{"vibrancy":1.4,"resizable":true,"panelWidth":620,"panelHeight":900}"#,
		);

		let loaded = load(&path);

		assert!(loaded.notice.is_none(), "a usable value was reported: {:?}", loaded.notice);
		assert_eq!(loaded.settings.vibrancy, 1.4);
		assert!(loaded.settings.resizable);
		// An integer in the file is a perfectly good `f64`; only the *written* form
		// is pinned to a decimal.
		assert_eq!(loaded.settings.panel_width, 620.0);
		assert_eq!(loaded.settings.panel_height, 900.0);
	}

	#[test]
	fn the_size_clamp_is_the_one_the_repair_uses() {
		assert_eq!(
			clamp_panel_size(DEFAULT_PANEL_WIDTH, DEFAULT_PANEL_HEIGHT),
			(DEFAULT_PANEL_WIDTH, DEFAULT_PANEL_HEIGHT),
			"the shipped size must survive its own clamp"
		);
		assert_eq!(
			clamp_panel_size(40000.0, 10.0),
			(PANEL_WIDTH_BOUNDS.1, PANEL_HEIGHT_BOUNDS.0)
		);
		assert_eq!(
			clamp_panel_size(0.0, f64::MAX),
			(PANEL_WIDTH_BOUNDS.0, PANEL_HEIGHT_BOUNDS.1)
		);
		// The case `f64::clamp` alone gets wrong: it returns NaN rather than a bound,
		// and a NaN reaching `set_size` is a window of no size at all.
		assert_eq!(
			clamp_panel_size(f64::NAN, f64::INFINITY),
			(DEFAULT_PANEL_WIDTH, DEFAULT_PANEL_HEIGHT)
		);
	}

	#[test]
	fn a_patch_sets_the_window_keys_independently() {
		let mut settings = Settings::default();

		settings.apply_patch(patch(r#"{"vibrancy":1.6}"#));
		assert_eq!(settings.vibrancy, 1.6);
		assert!(!settings.resizable, "an absent key must leave the stored value alone");
		assert_eq!(settings.panel_width, DEFAULT_PANEL_WIDTH);

		settings.apply_patch(patch(r#"{"resizable":true}"#));
		assert_eq!(settings.vibrancy, 1.6, "a resizable patch must not reset the vibrancy");
		assert!(settings.resizable);

		settings.apply_patch(patch(r#"{"panelWidth":620,"panelHeight":900}"#));
		assert!(settings.resizable, "a size patch must not fix the panel again");
		assert_eq!(settings.panel_width, 620.0);
		assert_eq!(settings.panel_height, 900.0);
		assert_eq!(settings.accent, "copper", "a size patch must not touch the palette");
	}

	/// The appearance keys join the guarantee every key added since task-012
	/// holds: a `settings.json` written by an earlier build has none of them, and
	/// reading one must be indistinguishable from reading a current file. The
	/// stakes are the recents list — the recovery path discards the whole file —
	/// and the visible cost of getting `translucent` backwards would be every
	/// existing install waking up with a see-through panel.
	#[test]
	fn a_file_without_the_appearance_keys_reads_as_the_shipped_look() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark","linkPreviews":true}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.notice.is_none(), "absence was reported as damage: {:?}", loaded.notice);
		assert_eq!(siblings(dir.path()), [FILE_NAME], "the file was set aside");
		assert!(!loaded.settings.translucent, "an absent key must not thin the panel");
		assert_eq!(loaded.settings.neutral, "warm");
		assert_eq!(loaded.settings.accent, "copper");
		// The rest of the file survived rather than being defaulted alongside them.
		assert_eq!(loaded.settings.theme, "dark");
		assert!(loaded.settings.link_previews);
	}

	#[test]
	fn wrong_typed_appearance_values_are_repaired_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"translucent":"a bit","neutral":7,"accent":false}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(!loaded.settings.translucent);
		assert_eq!(loaded.settings.neutral, "warm");
		assert_eq!(loaded.settings.accent, "copper");
		let notice = loaded.notice.expect("repairs must be reported");
		for expected in ["translucent", "neutral", "accent"] {
			assert!(notice.contains(expected), "{expected} unreported in: {notice}");
		}
	}

	/// A palette name nothing recognises is not damage — the frontend collapses it
	/// to the shipped value on read, exactly as it does for `theme` — so it must
	/// survive the load rather than being repaired here.
	#[test]
	fn an_unrecognised_palette_name_survives_a_load_unreported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"neutral":"chartreuse","accent":"gold"}"#);

		let loaded = load(&path);

		assert!(loaded.notice.is_none(), "a name is not a type error: {:?}", loaded.notice);
		assert_eq!(loaded.settings.neutral, "chartreuse");
		assert_eq!(loaded.settings.accent, "gold");
	}

	#[test]
	fn a_patch_sets_the_appearance_keys_independently() {
		let mut settings = Settings::default();

		settings.apply_patch(patch(r#"{"accent":"teal"}"#));
		assert_eq!(settings.accent, "teal");
		assert_eq!(settings.neutral, "warm", "an absent key must leave the stored value alone");
		assert!(!settings.translucent);

		settings.apply_patch(patch(r#"{"translucent":true}"#));
		assert_eq!(settings.accent, "teal", "a translucency patch must not reset the accent");
		assert!(settings.translucent);

		settings.apply_patch(patch(r#"{"neutral":"slate"}"#));
		assert!(settings.translucent, "a tone patch must not make the panel solid again");
		assert_eq!(settings.neutral, "slate");
	}

	/// Task-020's key takes the `showCreated` shape — an absent key reads as
	/// *off* — and the reason is sharper than it is for any other flag here. Every
	/// other absent key is a preference nobody expressed; this one is **consent to
	/// make network requests on the user's behalf**, and reading its absence as
	/// anything but "no" would turn an upgrade into the moment Copper silently
	/// started telling third parties which of their pages a note mentions.
	#[test]
	fn a_file_without_the_link_previews_key_reads_as_off_without_a_notice() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark","captureNotifications":false}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.notice.is_none(), "absence was reported as damage: {:?}", loaded.notice);
		assert_eq!(siblings(dir.path()), [FILE_NAME], "the file was set aside");
		assert!(!loaded.settings.link_previews, "an absent key must not enable fetching");
		// The rest of the file survived rather than being defaulted alongside it.
		assert_eq!(loaded.settings.theme, "dark");
		assert!(!loaded.settings.capture_notifications);
	}

	/// A value nobody can read is not consent. The repair direction is the whole
	/// point of this test — repairing *to* `true` would be a hand-edited typo
	/// switching the network on.
	#[test]
	fn a_wrong_typed_link_previews_is_repaired_to_off_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"linkPreviews":"yes please"}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(!loaded.settings.link_previews);
		let notice = loaded.notice.expect("repairs must be reported");
		assert!(notice.contains("linkPreviews"), "{notice}");
	}

	#[test]
	fn an_explicit_true_link_previews_survives_a_load() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"linkPreviews":true}"#);

		let loaded = load(&path);

		assert!(loaded.notice.is_none());
		assert!(loaded.settings.link_previews);
	}

	#[test]
	fn a_patch_sets_link_previews_without_touching_its_neighbours() {
		let mut settings = Settings::default();

		settings.apply_patch(patch(r#"{"linkPreviews":true}"#));
		assert!(settings.link_previews);
		assert!(settings.capture_notifications, "an absent key must leave the stored value alone");

		settings.apply_patch(patch(r#"{"showCreated":true}"#));
		assert!(settings.link_previews, "a showCreated patch must not turn previews off");
		assert!(settings.show_created);
	}

	/// Task-018's key takes the `alwaysOnTop` shape rather than the `showCreated`
	/// one: its default is `true`, so an absent key must read as *on*. Getting this
	/// backwards would silence capture notifications for every existing install the
	/// first time it upgraded, over a setting nobody had touched.
	#[test]
	fn a_file_without_the_capture_notifications_key_reads_as_on_without_a_notice() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark","showCreated":true}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.notice.is_none(), "absence was reported as damage: {:?}", loaded.notice);
		assert_eq!(siblings(dir.path()), [FILE_NAME], "the file was set aside");
		assert!(
			loaded.settings.capture_notifications,
			"an absent key must leave capture notifications on"
		);
		// The rest of the file survived rather than being defaulted alongside it.
		assert_eq!(loaded.settings.theme, "dark");
		assert!(loaded.settings.show_created);
	}

	#[test]
	fn a_wrong_typed_capture_notifications_is_repaired_to_on_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"captureNotifications":"loudly"}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.settings.capture_notifications);
		let notice = loaded.notice.expect("repairs must be reported");
		assert!(notice.contains("captureNotifications"), "{notice}");
	}

	#[test]
	fn an_explicit_false_capture_notifications_survives_a_load() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"captureNotifications":false}"#);

		let loaded = load(&path);

		assert!(loaded.notice.is_none());
		assert!(!loaded.settings.capture_notifications);
	}

	#[test]
	fn a_patch_sets_capture_notifications_without_touching_its_neighbours() {
		let mut settings = Settings::default();

		settings.apply_patch(patch(r#"{"captureNotifications":false}"#));
		assert!(!settings.capture_notifications);
		assert!(settings.always_on_top, "an absent key must leave the stored value alone");

		settings.apply_patch(patch(r#"{"showCreated":true}"#));
		assert!(
			!settings.capture_notifications,
			"a showCreated patch must not turn notifications back on"
		);
		assert!(settings.show_created);
	}

	/// The pin joins `sounds`, `motion` and task-013's two keys in the same
	/// guarantee — with the twist that its default is `true`, so an absent key must
	/// read as *on* rather than as the `false` a bare `Default` would give.
	#[test]
	fn a_file_without_the_always_on_top_key_reads_as_pinned_without_a_notice() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark","sounds":true}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.notice.is_none(), "absence was reported as damage: {:?}", loaded.notice);
		assert!(loaded.settings.always_on_top, "an absent pin must keep the window topmost");
	}

	#[test]
	fn a_wrong_typed_always_on_top_is_repaired_to_pinned_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"alwaysOnTop":"yes"}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.settings.always_on_top);
		let notice = loaded.notice.expect("repairs must be reported");
		assert!(notice.contains("alwaysOnTop"), "{notice}");
	}

	#[test]
	fn an_explicit_false_always_on_top_survives_a_load() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"alwaysOnTop":false}"#);

		let loaded = load(&path);

		assert!(loaded.notice.is_none());
		assert!(!loaded.settings.always_on_top);
	}

	#[test]
	fn a_patch_sets_always_on_top_without_touching_its_neighbours() {
		let mut settings = Settings::default();

		settings.apply_patch(patch(r#"{"alwaysOnTop":false}"#));
		assert!(!settings.always_on_top);
		assert_eq!(settings.theme, "system", "an absent key must leave the stored value alone");

		settings.apply_patch(patch(r#"{"theme":"dark"}"#));
		assert!(!settings.always_on_top, "a theme patch must not re-pin the window");
	}

	/// Task-016's one key joins the same guarantee `sounds`, `motion`, task-013's
	/// pair and the pin all hold: a `settings.json` written by any earlier build
	/// lacks it, and reading one must be indistinguishable from reading a current
	/// file — the documented default, no notice, and above all no `.corrupt-`
	/// rename over a feature the user never enabled.
	#[test]
	fn a_file_without_the_show_created_key_reads_as_hidden_without_a_notice() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark","alwaysOnTop":false}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.notice.is_none(), "absence was reported as damage: {:?}", loaded.notice);
		assert_eq!(siblings(dir.path()), [FILE_NAME], "the file was set aside");
		assert!(!loaded.settings.show_created, "the timestamp line must ship hidden");
		// The rest of the file survived rather than being defaulted alongside it.
		assert_eq!(loaded.settings.theme, "dark");
		assert!(!loaded.settings.always_on_top);
	}

	#[test]
	fn a_wrong_typed_show_created_is_repaired_to_hidden_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"showCreated":"sometimes"}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(!loaded.settings.show_created);
		let notice = loaded.notice.expect("repairs must be reported");
		assert!(notice.contains("showCreated"), "{notice}");
	}

	#[test]
	fn a_patch_sets_show_created_without_touching_its_neighbours() {
		let mut settings = Settings::default();

		settings.apply_patch(patch(r#"{"showCreated":true}"#));
		assert!(settings.show_created);
		assert!(settings.always_on_top, "an absent key must leave the stored value alone");

		settings.apply_patch(patch(r#"{"alwaysOnTop":false}"#));
		assert!(settings.show_created, "a pin patch must not hide the timestamp again");
		assert!(!settings.always_on_top);
	}

	/// Task-013's two keys join `sounds` and `motion` in the same guarantee: a
	/// file written before they existed must read as documented defaults, silently.
	#[test]
	fn a_file_without_the_task_013_keys_reads_as_defaults_without_a_notice() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark","motion":"off"}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.notice.is_none(), "absence was reported as damage: {:?}", loaded.notice);
		assert_eq!(loaded.settings.insertion_point, "bottom");
		assert_eq!(loaded.settings.double_click, "copy");
		assert_eq!(loaded.settings.insertion(), InsertionPoint::Bottom);
	}

	#[test]
	fn insertion_is_narrowed_by_name_and_anything_unrecognised_appends() {
		let mut settings = Settings::default();
		assert_eq!(settings.insertion(), InsertionPoint::Bottom);

		settings.insertion_point = "top".to_string();
		assert_eq!(settings.insertion(), InsertionPoint::Top);

		// A value of the right type that names nothing is not damage — it collapses
		// to the default here rather than being repaired on load.
		settings.insertion_point = "sideways".to_string();
		assert_eq!(settings.insertion(), InsertionPoint::Bottom);
	}

	#[test]
	fn wrong_typed_task_013_values_are_repaired_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"insertionPoint":3,"doubleClick":true}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert_eq!(loaded.settings.insertion_point, "bottom");
		assert_eq!(loaded.settings.double_click, "copy");
		let notice = loaded.notice.expect("repairs must be reported");
		for expected in ["insertionPoint", "doubleClick"] {
			assert!(notice.contains(expected), "{expected} unreported in: {notice}");
		}
	}

	#[test]
	fn a_patch_sets_the_task_013_keys_independently() {
		let mut settings = Settings::default();

		settings.apply_patch(patch(r#"{"insertionPoint":"top"}"#));
		assert_eq!(settings.insertion_point, "top");
		assert_eq!(settings.double_click, "copy", "an absent key must leave the stored value alone");

		settings.apply_patch(patch(r#"{"doubleClick":"edit"}"#));
		assert_eq!(settings.insertion_point, "top", "a doubleClick patch must not clear insertionPoint");
		assert_eq!(settings.double_click, "edit");
	}

	/// Task-012 AC13. A `settings.json` written by any earlier build has neither
	/// key, and reading one must be indistinguishable from reading a current file:
	/// documented defaults, no notice, and above all no `.corrupt-` rename — the
	/// recovery path discards the whole file, so a merely *absent* key reaching it
	/// would cost the user their recents list over a feature they never enabled.
	#[test]
	fn a_file_without_the_sound_and_motion_keys_is_not_treated_as_corrupt() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(
			dir.path(),
			r#"{"recents":["C:\\a.copper"],"activeSpace":0,"panelPosition":null,
			   "shortcuts":{"capture":"Shift Shift","summon":"Ctrl+Shift+Space"},"theme":"dark"}"#,
		);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.notice.is_none(), "absence was reported as damage: {:?}", loaded.notice);
		assert_eq!(siblings(dir.path()), [FILE_NAME], "the file was set aside");
		assert!(!loaded.settings.sounds, "sound must default to off");
		assert_eq!(loaded.settings.motion, "auto");
		// The rest of the file survived rather than being defaulted alongside them.
		assert_eq!(loaded.settings.theme, "dark");
		assert_eq!(loaded.settings.recents, ["C:\\a.copper"]);
	}

	#[test]
	fn wrong_typed_sound_and_motion_values_are_repaired_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"sounds":"yes","motion":7,"theme":"light"}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(!loaded.settings.sounds);
		assert_eq!(loaded.settings.motion, "auto");
		assert_eq!(loaded.settings.theme, "light");
		let notice = loaded.notice.expect("repairs must be reported");
		for expected in ["sounds", "motion"] {
			assert!(notice.contains(expected), "{expected} unreported in: {notice}");
		}
	}

	#[test]
	fn a_patch_sets_sounds_and_motion_independently() {
		let mut settings = Settings::default();

		settings.apply_patch(patch(r#"{"sounds":true}"#));
		assert!(settings.sounds);
		assert_eq!(settings.motion, "auto", "an absent key must leave the stored value alone");

		settings.apply_patch(patch(r#"{"motion":"off"}"#));
		assert!(settings.sounds, "a motion patch must not clear sounds");
		assert_eq!(settings.motion, "off");
	}

	#[test]
	fn a_missing_file_yields_defaults_with_no_notice() {
		let dir = tempfile::tempdir().unwrap();
		let loaded = load(&dir.path().join(FILE_NAME));
		assert_eq!(loaded.settings, Settings::default());
		assert!(loaded.notice.is_none());
		assert_eq!(loaded.origin, Origin::Absent);
	}

	#[test]
	fn a_corrupt_file_is_set_aside_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), "{ this is not json");

		let loaded = load(&path);

		assert_eq!(loaded.settings, Settings::default());
		assert_eq!(loaded.origin, Origin::Quarantined);
		let notice = loaded.notice.expect("a corrupt file must produce a notice");
		assert!(notice.contains("corrupt-"), "{notice}");
		assert!(!path.exists(), "the corrupt file was left in place");
		let kept = siblings(dir.path());
		assert_eq!(kept.len(), 1);
		assert!(kept[0].starts_with("settings.json.corrupt-"), "{kept:?}");
	}

	/// Spec 6.1a / A9.38. The regression test for modelling `activeSpace` as
	/// `usize`: that costs the whole recents list over one character.
	#[test]
	fn a_negative_active_space_clamps_without_discarding_recents() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(
			dir.path(),
			r#"{"recents":["C:\\a.copper","C:\\b.copper"],"activeSpace":-1,"panelPosition":null,
			   "shortcuts":{"capture":"Shift Shift","summon":"Ctrl+Shift+Space"},"theme":"system"}"#,
		);

		let loaded = load(&path);

		assert!(loaded.notice.is_none(), "the file was treated as corrupt: {:?}", loaded.notice);
		assert_eq!(loaded.origin, Origin::Loaded);
		assert_eq!(loaded.settings.recents.len(), 2);
		assert_eq!(loaded.settings.active_space, 0);
		assert_eq!(siblings(dir.path()), [FILE_NAME]);
	}

	#[test]
	fn an_out_of_range_active_space_clamps() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(
			dir.path(),
			r#"{"recents":["C:\\a.copper"],"activeSpace":9,"panelPosition":null,
			   "shortcuts":{"capture":"Shift Shift","summon":"Ctrl+Shift+Space"},"theme":"system"}"#,
		);

		let loaded = load(&path);

		assert!(loaded.notice.is_none());
		assert_eq!(loaded.settings.recents.len(), 1);
		assert_eq!(loaded.settings.active_space, 0);
	}

	/// Invalid UTF-8 is an unusable *file*, not an I/O failure, and has to reach
	/// the same quarantine path as invalid JSON. Reading through
	/// `read_to_string` classified it as a read error instead, which left the
	/// original in place while reporting a notice — and the caller then wrote
	/// defaults over it.
	#[test]
	fn invalid_utf8_is_quarantined_like_invalid_json() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join(FILE_NAME);
		std::fs::write(&path, [0x7b, 0x22, 0xff, 0xfe, 0x22, 0x7d]).unwrap();

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Quarantined);
		assert!(loaded.origin.may_overwrite());
		assert!(!path.exists(), "the unusable file was left in place");
		let notice = loaded.notice.unwrap();
		assert!(notice.contains("UTF-8"), "{notice}");
		assert!(notice.contains("corrupt-"), "{notice}");
	}

	/// The other half of the same defect: when the original could *not* be moved
	/// out of the way, the caller must be told not to write over it.
	#[test]
	fn a_file_that_cannot_be_set_aside_is_reported_as_retained() {
		use std::os::windows::fs::OpenOptionsExt;

		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), "{ not json");
		// Readable but not renameable, so the parse fails and the quarantine does
		// too.
		let _held = std::fs::OpenOptions::new()
			.read(true)
			.share_mode(0x0000_0001) // FILE_SHARE_READ
			.open(&path)
			.unwrap();

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Retained);
		assert!(
			!loaded.origin.may_overwrite(),
			"a file that could not be set aside must not be overwritten"
		);
		assert!(path.exists());
		assert!(loaded.notice.unwrap().contains("left the file untouched"));
	}

	#[test]
	fn a_file_that_cannot_be_read_is_reported_as_retained() {
		use std::os::windows::fs::OpenOptionsExt;

		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark"}"#);
		// No sharing at all, so even opening it fails with a sharing violation.
		let _held = std::fs::OpenOptions::new()
			.read(true)
			.share_mode(0)
			.open(&path)
			.unwrap();

		let started = std::time::Instant::now();
		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Retained);
		assert!(!loaded.origin.may_overwrite());
		assert_eq!(loaded.settings, Settings::default());
		assert!(path.exists());
		// It retried rather than giving up on first sight: a lock at startup is
		// most likely an indexer or a scanner that will let go shortly.
		assert!(
			started.elapsed() >= std::time::Duration::from_millis(400),
			"the read was not retried: took {:?}",
			started.elapsed()
		);
	}

	// --- per-field repair (spec 6.1a generalised) ---

	/// The four cases that previously cost the user their whole recents list
	/// because one value had the wrong type.
	#[test]
	fn one_bad_field_is_repaired_rather_than_quarantining_the_file() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(
			dir.path(),
			r#"{"recents":["C:\\a.copper", 7, "C:\\b.copper"],
			   "activeSpace":1,
			   "panelPosition":{"x":10},
			   "shortcuts":{"capture":false,"summon":"Alt+Space"},
			   "theme":42}"#,
		);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded, "a repairable file was quarantined");
		assert_eq!(siblings(dir.path()), [FILE_NAME]);

		// The two real paths survived; the number did not.
		assert_eq!(loaded.settings.recents, ["C:\\a.copper", "C:\\b.copper"]);
		assert_eq!(loaded.settings.active_space, 1);
		// A half-written point is not a point.
		assert_eq!(loaded.settings.panel_position, None);
		// The bad chord reverted; the good one beside it did not.
		assert_eq!(loaded.settings.shortcuts.capture, Shortcuts::default().capture);
		assert_eq!(loaded.settings.shortcuts.summon, "Alt+Space");
		assert_eq!(loaded.settings.theme, "system");

		let notice = loaded.notice.expect("repairs must be reported");
		for expected in ["recents", "panelPosition", "shortcuts.capture", "theme"] {
			assert!(notice.contains(expected), "{expected} unreported in: {notice}");
		}
	}

	#[test]
	fn a_wholly_wrong_shortcuts_value_restores_both_defaults() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"shortcuts":"Ctrl+K"}"#);

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert_eq!(loaded.settings.shortcuts, Shortcuts::default());
		assert!(loaded.notice.unwrap().contains("shortcuts"));
	}

	#[test]
	fn a_partial_file_fills_in_defaults_rather_than_being_quarantined() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark"}"#);

		let loaded = load(&path);

		assert!(loaded.notice.is_none());
		assert_eq!(loaded.settings.theme, "dark");
		assert_eq!(loaded.settings.shortcuts, Shortcuts::default());
	}

	#[test]
	fn round_trips_through_disk() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join(FILE_NAME);
		let mut settings = Settings {
			panel_position: Some(PanelPosition { x: 2140, y: 180 }),
			..Default::default()
		};
		settings.touch_recent("C:\\notes.copper");

		save(&path, &settings).unwrap();
		let loaded = load(&path);

		assert!(loaded.notice.is_none());
		assert_eq!(loaded.settings, settings);
	}

	#[test]
	fn touch_recent_dedupes_case_insensitively_and_caps_the_list() {
		let mut settings = Settings::default();
		for index in 0..25 {
			settings.touch_recent(&format!("C:\\space{index}.copper"));
		}
		assert_eq!(settings.recents.len(), MAX_RECENTS);
		assert_eq!(settings.recents[0], "C:\\space24.copper");

		settings.touch_recent("c:\\SPACE20.COPPER");
		assert_eq!(settings.recents[0], "c:\\SPACE20.COPPER");
		assert_eq!(
			settings.recents.iter().filter(|e| e.eq_ignore_ascii_case("C:\\space20.copper")).count(),
			1
		);
		assert_eq!(settings.active_space, 0);
	}

	#[test]
	fn forget_recent_drops_the_entry_however_it_is_spelled() {
		let mut settings = Settings::default();
		settings.touch_recent("C:\\a.copper");
		settings.touch_recent("C:\\b.copper");

		settings.forget_recent("c:\\A.COPPER");
		assert_eq!(settings.recents, ["C:\\b.copper"]);

		// A path that was never there leaves the list alone rather than failing:
		// the desired end state already holds.
		settings.forget_recent("C:\\never-there.copper");
		assert_eq!(settings.recents, ["C:\\b.copper"]);
	}

	#[test]
	fn point_at_finds_the_path_or_clamps_to_zero() {
		let mut settings = Settings::default();
		settings.touch_recent("C:\\a.copper");
		settings.touch_recent("C:\\b.copper");

		settings.point_at(Some("C:\\a.copper"));
		assert_eq!(settings.active_space, 1);

		settings.point_at(Some("C:\\gone.copper"));
		assert_eq!(settings.active_space, 0);

		settings.point_at(None);
		assert_eq!(settings.active_space, 0);
	}

	// --- patch ---

	fn patch(json: &str) -> SettingsPatch {
		serde_json::from_str(json).unwrap()
	}

	#[test]
	fn an_absent_panel_position_leaves_the_stored_one_alone() {
		let mut settings = Settings {
			panel_position: Some(PanelPosition { x: 1, y: 2 }),
			..Default::default()
		};

		settings.apply_patch(patch(r#"{"theme":"dark"}"#));

		assert_eq!(settings.panel_position, Some(PanelPosition { x: 1, y: 2 }));
		assert_eq!(settings.theme, "dark");
	}

	#[test]
	fn an_explicit_null_panel_position_clears_it() {
		let mut settings = Settings {
			panel_position: Some(PanelPosition { x: 1, y: 2 }),
			..Default::default()
		};

		settings.apply_patch(patch(r#"{"panelPosition":null}"#));

		assert_eq!(settings.panel_position, None);
	}

	#[test]
	fn an_object_panel_position_sets_it() {
		let mut settings = Settings::default();
		settings.apply_patch(patch(r#"{"panelPosition":{"x":10,"y":20}}"#));
		assert_eq!(settings.panel_position, Some(PanelPosition { x: 10, y: 20 }));
	}

	#[test]
	fn a_patch_cannot_reach_recents_or_active_space() {
		let mut settings = Settings::default();
		settings.touch_recent("C:\\a.copper");

		settings.apply_patch(patch(r#"{"recents":[],"activeSpace":7,"theme":"light"}"#));

		assert_eq!(settings.recents, ["C:\\a.copper"]);
		assert_eq!(settings.active_space, 0);
		assert_eq!(settings.theme, "light");
	}

	// --- the read-only loader -------------------------------------------------

	/// The pair of assertions every `load_read_only` case below makes, because the
	/// point of the function is what it does *not* do: the file is still named what
	/// it was named, and still holds what it held.
	fn untouched(dir: &Path, path: &Path, before: &[u8]) {
		assert_eq!(siblings(dir), [FILE_NAME], "a file was created or renamed");
		assert_eq!(std::fs::read(path).unwrap(), before, "the file's bytes changed");
	}

	#[test]
	fn the_read_only_loader_reports_an_absent_file_without_creating_one() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join(FILE_NAME);

		let loaded = load_read_only(&path);

		assert_eq!(loaded.origin, Origin::Absent);
		assert!(loaded.notice.is_none());
		assert_eq!(loaded.settings, Settings::default());
		assert!(siblings(dir.path()).is_empty(), "a listing wrote a file");
	}

	#[test]
	fn the_read_only_loader_reads_a_valid_file_exactly_as_load_does() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark","recents":["C:\\a.copper"]}"#);
		let before = std::fs::read(&path).unwrap();

		let loaded = load_read_only(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert!(loaded.notice.is_none());
		assert_eq!(loaded.settings.theme, "dark");
		assert_eq!(loaded.settings.recents, ["C:\\a.copper"]);
		untouched(dir.path(), &path, &before);
	}

	/// A field-level repair is in-memory in both loaders — `load` only ever writes
	/// through its *caller* — so the read-only one has to produce the same repaired
	/// settings and the same notice, and still leave the file alone.
	#[test]
	fn the_read_only_loader_repairs_fields_in_memory_only() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":7,"recents":["C:\\a.copper"]}"#);
		let before = std::fs::read(&path).unwrap();

		let loaded = load_read_only(&path);

		assert_eq!(loaded.origin, Origin::Loaded);
		assert_eq!(loaded.settings.theme, "system");
		assert_eq!(loaded.settings.recents, ["C:\\a.copper"], "a repair cost the recents list");
		assert!(loaded.notice.expect("repairs are reported").contains("theme"));
		untouched(dir.path(), &path, &before);
	}

	/// The case the function exists for. `load` renames this file; a listing
	/// command must not.
	#[test]
	fn the_read_only_loader_leaves_invalid_json_where_it_is() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), "{not json at all");
		let before = std::fs::read(&path).unwrap();

		let loaded = load_read_only(&path);

		assert_eq!(loaded.origin, Origin::Retained, "saving over this would destroy it");
		assert_eq!(loaded.settings, Settings::default());
		assert!(loaded.notice.expect("the reason is reported").contains("JSON"));
		untouched(dir.path(), &path, &before);
	}

	#[test]
	fn the_read_only_loader_leaves_invalid_utf8_where_it_is() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join(FILE_NAME);
		// A lone continuation byte: valid JSON shape, not valid UTF-8.
		let before = b"{\"theme\":\"\xff\"}".to_vec();
		std::fs::write(&path, &before).unwrap();

		let loaded = load_read_only(&path);

		assert_eq!(loaded.origin, Origin::Retained);
		assert!(loaded.notice.expect("the reason is reported").contains("UTF-8"));
		untouched(dir.path(), &path, &before);
	}

	/// The contrast, stated once: `load` on the same bytes *does* rename. Without
	/// this the pair above would still pass if `load_read_only` were simply what
	/// `load` had always been.
	#[test]
	fn the_ordinary_loader_still_sets_an_invalid_file_aside() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), "{not json at all");

		let loaded = load(&path);

		assert_eq!(loaded.origin, Origin::Quarantined);
		assert!(!path.exists(), "the file kept its name");
		assert_eq!(siblings(dir.path()).len(), 1, "the file was not kept at all");
	}

	// --- where the config directory is ----------------------------------------

	/// Restores `APPDATA` however the test ends.
	///
	/// Tests in one binary share a process and its environment, so this must put
	/// the variable back rather than leaving a value behind for whatever runs next
	/// on another thread. Nothing else in this crate reads `APPDATA`, which is what
	/// keeps the window this opens harmless.
	struct ScopedAppData(Option<std::ffi::OsString>);

	impl ScopedAppData {
		fn set(value: &str) -> Self {
			let previous = std::env::var_os("APPDATA");
			std::env::set_var("APPDATA", value);
			Self(previous)
		}
	}

	impl Drop for ScopedAppData {
		fn drop(&mut self) {
			match self.0.take() {
				Some(previous) => std::env::set_var("APPDATA", previous),
				None => std::env::remove_var("APPDATA"),
			}
		}
	}

	#[test]
	fn the_config_dir_is_appdata_joined_with_the_identifier() {
		let _guard = ScopedAppData::set(r"C:\Users\test\AppData\Roaming");

		assert_eq!(
			default_config_dir(),
			Some(PathBuf::from(
				r"C:\Users\test\AppData\Roaming\io.github.falldownthesystem.copper"
			))
		);
	}

	/// The identifier is `tauri.conf.json`'s, and this is the assertion that says
	/// so out loud. A change on either side that does not change the other moves
	/// the CLI's idea of the config directory away from the app's, and the symptom
	/// would be an empty recents list rather than an error.
	#[test]
	fn the_identifier_matches_the_bundle_identifier() {
		assert_eq!(APP_IDENTIFIER, "io.github.falldownthesystem.copper");
	}
}
