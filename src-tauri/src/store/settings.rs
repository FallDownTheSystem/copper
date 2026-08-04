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

use super::atomic::write_atomic;
use super::error::{io_err, Result};
use super::format::{now_rfc3339, to_git_json};

/// Spec 6.4. Twenty is what fits a switcher without scrolling forever.
const MAX_RECENTS: usize = 20;

pub const FILE_NAME: &str = "settings.json";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
	pub recents: Vec<String>,
	#[serde(deserialize_with = "deserialise_active_space")]
	pub active_space: usize,
	pub panel_position: Option<PanelPosition>,
	pub shortcuts: Shortcuts,
	pub theme: String,
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
		}
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

/// Signed on the wire, clamped in memory (spec 6.1a).
fn deserialise_active_space<'de, D: Deserializer<'de>>(
	deserializer: D,
) -> std::result::Result<usize, D::Error> {
	let raw = i64::deserialize(deserializer)?;
	Ok(if raw < 0 { 0 } else { raw as usize })
}

impl Settings {
	/// Puts `active_space` back in range. Called after every load and after any
	/// change to `recents`.
	fn clamp(&mut self) {
		if self.active_space >= self.recents.len() {
			self.active_space = 0;
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

	/// Drops `path` from `recents`, reporting whether anything changed.
	///
	/// Leaves `active_space` alone — the caller knows which space is actually
	/// open and re-points it (spec 6.7).
	pub fn forget_recent(&mut self, path: &str) -> bool {
		let before = self.recents.len();
		self.recents.retain(|entry| !same_path(entry, path));
		self.recents.len() != before
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

/// Reads `settings.json`, recovering rather than failing.
///
/// Returns the settings plus an optional notice for `get_status` to surface.
/// It never emits: this runs during startup, where nothing is listening yet
/// (spec 8A.2), so the reason has to be *recorded* to reach the user at all.
pub fn load(path: &Path) -> (Settings, Option<String>) {
	let text = match std::fs::read_to_string(path) {
		Ok(text) => text,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), None),
		Err(err) => {
			return (
				Settings::default(),
				Some(format!(
					"{} could not be read ({err}). Copper started from default settings.",
					path.display()
				)),
			)
		}
	};

	match serde_json::from_str::<Settings>(&text) {
		Ok(mut settings) => {
			settings.clamp();
			(settings, None)
		}
		Err(err) => {
			let notice = match quarantine(path) {
				Ok(kept) => format!(
					"{} was not valid ({err}). It has been kept as {} and Copper started from \
					 default settings.",
					path.display(),
					kept.display()
				),
				Err(move_err) => format!(
					"{} was not valid ({err}) and could not be set aside ({move_err}). Copper \
					 started from default settings.",
					path.display()
				),
			};
			(Settings::default(), Some(notice))
		}
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
			 \"Ctrl+Shift+Space\"\n  },\n  \"theme\": \"system\"\n}\n"
		);
	}

	#[test]
	fn a_missing_file_yields_defaults_with_no_notice() {
		let dir = tempfile::tempdir().unwrap();
		let (settings, notice) = load(&dir.path().join(FILE_NAME));
		assert_eq!(settings, Settings::default());
		assert!(notice.is_none());
	}

	#[test]
	fn a_corrupt_file_is_set_aside_and_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), "{ this is not json");

		let (settings, notice) = load(&path);

		assert_eq!(settings, Settings::default());
		let notice = notice.expect("a corrupt file must produce a notice");
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

		let (settings, notice) = load(&path);

		assert!(notice.is_none(), "the file was treated as corrupt: {notice:?}");
		assert_eq!(settings.recents.len(), 2);
		assert_eq!(settings.active_space, 0);
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

		let (settings, notice) = load(&path);

		assert!(notice.is_none());
		assert_eq!(settings.recents.len(), 1);
		assert_eq!(settings.active_space, 0);
	}

	#[test]
	fn a_partial_file_fills_in_defaults_rather_than_being_quarantined() {
		let dir = tempfile::tempdir().unwrap();
		let path = write(dir.path(), r#"{"theme":"dark"}"#);

		let (settings, notice) = load(&path);

		assert!(notice.is_none());
		assert_eq!(settings.theme, "dark");
		assert_eq!(settings.shortcuts, Shortcuts::default());
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
		let (loaded, notice) = load(&path);

		assert!(notice.is_none());
		assert_eq!(loaded, settings);
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
	fn forget_recent_reports_whether_it_changed_anything() {
		let mut settings = Settings::default();
		settings.touch_recent("C:\\a.copper");
		settings.touch_recent("C:\\b.copper");

		assert!(settings.forget_recent("c:\\A.COPPER"));
		assert_eq!(settings.recents, ["C:\\b.copper"]);
		assert!(!settings.forget_recent("C:\\never-there.copper"));
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
}
