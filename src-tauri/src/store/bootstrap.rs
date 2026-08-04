//! First run and startup: config directories, settings, and choosing which
//! space to open.
//!
//! **Nothing here can emit an event, by construction.** It is handed a config
//! directory and nothing else — no `AppHandle`, no `EventSink` — so spec 8A.2's
//! rule that bootstrap never emits is structural rather than remembered. That
//! matters because Tauri events have no buffering and no replay: anything
//! emitted before the webview has registered its listeners is silently dropped,
//! which makes an emit here not merely useless but an invisible failure.
//! Startup failures are *recorded* as `startup_notice` and surface through
//! `get_status` on the frontend's mount-time pull.
//!
//! It also registers no watcher, which is the other half of spec 7.5: the
//! watcher callback resolves the store that this function is still in the
//! middle of producing.

use std::path::{Path, PathBuf};

use super::atomic;
use super::error::{io_err, Result};
use super::model::Space;
use super::settings::{self, Settings};
use super::{canonical, format, new_space, path_string, OpenSpace};

const SPACES_DIR: &str = "spaces";
const DEFAULT_SPACE_FILE: &str = "personal.copper";
const DEFAULT_SPACE_NAME: &str = "personal";

/// A fully loaded store, minus the two things bootstrap must not have: a way to
/// emit and a registered watcher.
pub struct Bootstrapped {
	pub settings: Settings,
	pub settings_path: PathBuf,
	pub spaces_dir: PathBuf,
	pub open: Option<OpenSpace>,
	pub startup_notice: Option<String>,
}

pub fn init(config_dir: &Path) -> Result<Bootstrapped> {
	std::fs::create_dir_all(config_dir).map_err(|err| io_err(config_dir, "create", &err))?;
	let spaces_dir = config_dir.join(SPACES_DIR);
	std::fs::create_dir_all(&spaces_dir).map_err(|err| io_err(&spaces_dir, "create", &err))?;

	let settings_path = config_dir.join(settings::FILE_NAME);
	let had_settings = settings_path.exists();
	let (mut settings, startup_notice) = settings::load(&settings_path);

	let open = resolve_space(&settings, &spaces_dir)?;

	let before = settings.clone();
	settings.touch_recent(&path_string(&open.path));
	// Writing only when something actually changed keeps a plain relaunch from
	// touching the file at all.
	if !had_settings || startup_notice.is_some() || settings != before {
		settings::save(&settings_path, &settings)?;
	}

	Ok(Bootstrapped {
		settings,
		settings_path,
		spaces_dir,
		open: Some(open),
		startup_notice,
	})
}

/// The recorded active space, then any other recents entry, then the default.
///
/// **Unavailable entries are never removed from `recents`** (spec 7.3): a path
/// in a repository that simply is not checked out right now must come back when
/// it is.
fn resolve_space(settings: &Settings, spaces_dir: &Path) -> Result<OpenSpace> {
	let mut candidates: Vec<&str> = Vec::new();
	if let Some(active) = settings.active_recent() {
		candidates.push(active);
	}
	for entry in &settings.recents {
		if !candidates.contains(&entry.as_str()) {
			candidates.push(entry);
		}
	}

	for candidate in candidates {
		if let Ok(open) = open_at(Path::new(candidate)) {
			return Ok(open);
		}
	}

	let default_path = spaces_dir.join(DEFAULT_SPACE_FILE);
	if default_path.exists() {
		// It exists, and no recents entry loaded — which includes the case where
		// this very file is the one that failed. Spec 7.6: treat it as
		// authoritative and fail with its own error rather than writing beside it.
		// Silently creating `personal-2.copper` would strand the user's real notes
		// in a file the app has stopped opening.
		return open_at(&default_path);
	}
	create_default(&default_path)
}

fn open_at(path: &Path) -> Result<OpenSpace> {
	OpenSpace::load(&canonical(path)?)
}

/// First run: a space at `<config>\spaces\personal.copper` with one section, so
/// the composer always has a valid destination and capture can never fail for
/// want of a target.
fn create_default(path: &Path) -> Result<OpenSpace> {
	let dir = atomic::parent_dir(path)?;
	let doc: Space = new_space(DEFAULT_SPACE_NAME);
	let text = format::to_git_json(&doc)?;
	atomic::prepare(dir, &text)?
		.commit_new(path)
		.map_err(|failure| io_err(path, "create", &failure.error))?;
	open_at(path)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_first_run_creates_the_directories_settings_and_a_default_space() {
		let dir = tempfile::tempdir().unwrap();
		let config = dir.path().join("Copper");

		let built = init(&config).unwrap();

		assert!(config.join(SPACES_DIR).is_dir());
		assert!(config.join(settings::FILE_NAME).is_file());
		assert!(built.startup_notice.is_none());

		let open = built.open.expect("a space is always open after bootstrap");
		assert_eq!(open.doc.name, DEFAULT_SPACE_NAME);
		assert_eq!(open.doc.sections.len(), 1);
		assert_eq!(built.settings.recents.len(), 1);
		assert_eq!(built.settings.active_space, 0);
	}

	#[test]
	fn a_second_run_reopens_the_same_space_without_rewriting_it() {
		let dir = tempfile::tempdir().unwrap();
		let config = dir.path().join("Copper");
		let first = init(&config).unwrap();
		let space_path = first.open.unwrap().path;
		let bytes = std::fs::read(&space_path).unwrap();
		let modified = std::fs::metadata(&space_path).unwrap().modified().unwrap();

		let second = init(&config).unwrap();

		assert_eq!(second.open.unwrap().path, space_path);
		assert_eq!(std::fs::read(&space_path).unwrap(), bytes);
		assert_eq!(
			std::fs::metadata(&space_path).unwrap().modified().unwrap(),
			modified,
			"startup rewrote a space document it did not create"
		);
	}

	#[test]
	fn a_recents_entry_that_no_longer_exists_falls_back_but_is_not_dropped() {
		let dir = tempfile::tempdir().unwrap();
		let config = dir.path().join("Copper");
		let built = init(&config).unwrap();
		let real = path_string(&built.open.unwrap().path);

		// activeSpace points at a file that was deleted; the real one is second.
		let missing = path_string(&dir.path().join("gone.copper"));
		let mut settings = built.settings;
		settings.recents = vec![missing.clone(), real.clone()];
		settings.active_space = 0;
		settings::save(&config.join(settings::FILE_NAME), &settings).unwrap();

		let reopened = init(&config).unwrap();

		assert_eq!(path_string(&reopened.open.unwrap().path), real);
		assert!(
			reopened.settings.recents.iter().any(|entry| entry == &missing),
			"an unavailable entry was pruned: {:?}",
			reopened.settings.recents
		);
		assert_eq!(reopened.settings.recents[0], real);
	}

	#[test]
	fn a_corrupt_settings_file_is_reported_and_startup_continues() {
		let dir = tempfile::tempdir().unwrap();
		let config = dir.path().join("Copper");
		std::fs::create_dir_all(&config).unwrap();
		std::fs::write(config.join(settings::FILE_NAME), "not json at all").unwrap();

		let built = init(&config).unwrap();

		assert!(built.startup_notice.is_some());
		assert!(built.open.is_some());
		assert_eq!(built.settings.recents.len(), 1);
	}

	/// Spec 7.6. The default path exists but cannot be read, and nothing else
	/// loads: refuse rather than write beside it.
	#[test]
	fn an_unreadable_default_space_fails_startup_rather_than_being_replaced() {
		let dir = tempfile::tempdir().unwrap();
		let config = dir.path().join("Copper");
		std::fs::create_dir_all(config.join(SPACES_DIR)).unwrap();
		let default_path = config.join(SPACES_DIR).join(DEFAULT_SPACE_FILE);
		std::fs::write(&default_path, "<<<<<<< HEAD\nnot json\n").unwrap();

		let Err(err) = init(&config) else {
			panic!("startup overwrote or ignored an unreadable default space");
		};

		assert_eq!(err.kind(), "parse");
		assert_eq!(
			std::fs::read_to_string(&default_path).unwrap(),
			"<<<<<<< HEAD\nnot json\n",
			"the unreadable default space was overwritten"
		);
	}

	#[test]
	fn bootstrap_cannot_emit() {
		// A9.19's Rust half. The signature carries no emit-capable handle, so this
		// is really a guard against someone reaching for one later.
		let source = include_str!("bootstrap.rs");
		let code = source
			.split("#[cfg(test)]")
			.next()
			.unwrap()
			.lines()
			.filter(|line| !line.trim_start().starts_with("//"))
			.collect::<Vec<_>>()
			.join("\n");
		assert!(
			!code.contains("emit"),
			"bootstrap gained an emit; spec 8A.2 forbids it"
		);
	}
}
