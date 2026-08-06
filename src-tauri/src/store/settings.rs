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

/// Serialize only. Reading goes through [`RawSettings`], which repairs field by
/// field rather than failing the whole file over one bad value.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
	pub recents: Vec<String>,
	pub active_space: usize,
	pub panel_position: Option<PanelPosition>,
	pub shortcuts: Shortcuts,
	pub theme: String,
	pub sounds: bool,
	pub motion: String,
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

		let theme = match self.theme {
			Value::Null => defaults.theme,
			Value::String(theme) => theme,
			_ => {
				notices.push("\"theme\" was not a name and has been reset to \"system\".".into());
				defaults.theme
			}
		};

		// Absent from every `settings.json` written before task-012, so `Value::Null`
		// is the ordinary case here rather than the damaged one and must stay
		// silent — a notice would make an older file look broken.
		let sounds = match self.sounds {
			Value::Null => defaults.sounds,
			Value::Bool(on) => on,
			_ => {
				notices.push("\"sounds\" was not true or false and has been turned off.".into());
				defaults.sounds
			}
		};

		// Checked by name on the frontend, not here, exactly as `theme` is: a value
		// of the right *type* that names nothing is repairable locally, and only a
		// wrong type is worth a notice.
		let motion = match self.motion {
			Value::Null => defaults.motion,
			Value::String(motion) => motion,
			_ => {
				notices.push("\"motion\" was not a name and has been reset to \"auto\".".into());
				defaults.motion
			}
		};

		let mut settings = Settings {
			recents,
			active_space,
			panel_position,
			shortcuts,
			theme,
			sounds,
			motion,
		};
		settings.clamp();
		(settings, notices)
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
pub fn load(path: &Path) -> LoadedSettings {
	let bytes = match read_with_retry(path) {
		Ok(Some(bytes)) => bytes,
		Ok(None) => return LoadedSettings::defaults(Origin::Absent, None),
		Err(err) => {
			// Present but unreadable. Defaults let Copper start; leaving the file
			// alone lets the user get their recents list back once whatever is
			// holding it lets go.
			return LoadedSettings::defaults(
				Origin::Retained,
				Some(format!(
					"{} could not be read ({err}). Copper started from default settings and left \
					 the file untouched.",
					path.display()
				)),
			);
		}
	};

	// Read as bytes and decoded here rather than through `read_to_string`, so
	// invalid UTF-8 lands on the quarantine path with every other unusable file
	// instead of being mistaken for an I/O failure.
	let text = match String::from_utf8(bytes) {
		Ok(text) => text,
		Err(err) => return set_aside(path, &format!("it is not valid UTF-8 ({err})")),
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
			LoadedSettings {
				settings,
				notice,
				origin: Origin::Loaded,
			}
		}
		Err(err) => set_aside(path, &format!("it is not valid JSON ({err})")),
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
			 \"motion\": \"auto\"\n}\n"
		);
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
}
