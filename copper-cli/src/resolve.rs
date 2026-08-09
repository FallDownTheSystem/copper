//! Which space, and which section or note inside it.
//!
//! Two unrelated jobs that share one property: both have to answer *before* any
//! `ops::` call, because every `ops` function takes an id and none of them take a
//! name or a prefix.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use copper_core::entry;
use copper_core::spaces::paths;
use copper_core::store::error::{Result, StoreError};
use copper_core::store::events::NullSink;
use copper_core::store::model::Space;
use copper_core::store::{self, settings, Store};
use serde::{Deserialize, Serialize};

/// The environment variable in the middle of the resolution chain.
pub const SPACE_ENV: &str = "COPPER_SPACE";

/// The CLI's own state file, beside the app's `settings.json`.
///
/// A separate file rather than a key in `settings.json`, and the reason is not
/// tidiness: `settings::save` is last-writer-wins with no compare-and-swap, and a
/// running app holds its whole `Settings` in memory and rewrites the file
/// wholesale on the next panel drag. A CLI selection stored there would be
/// silently reverted. This file has exactly one writer.
pub const STATE_FILE_NAME: &str = "cli-state.json";

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct CliState {
	/// An absolute, resolved path. Stored resolved so that a later invocation
	/// from a different working directory means the same space.
	#[serde(default)]
	pub space: Option<String>,
}

// --- where things live -------------------------------------------------------

/// `%APPDATA%\io.github.falldownthesystem.copper`.
///
/// The same directory Tauri's `app_config_dir()` gives the app, reconstructed
/// from the identifier `copper-core` owns rather than from a literal here.
pub fn config_dir() -> Result<PathBuf> {
	settings::default_config_dir().ok_or_else(|| {
		StoreError::Unavailable(
			"APPDATA is not set, so Copper's settings directory cannot be located".into(),
		)
	})
}

pub fn settings_path() -> Result<PathBuf> {
	Ok(config_dir()?.join(settings::FILE_NAME))
}

pub fn state_path() -> Result<PathBuf> {
	Ok(config_dir()?.join(STATE_FILE_NAME))
}

/// Reads the CLI's selection, tolerating a file that is not one.
///
/// A corrupt state file falls through the chain rather than failing the command,
/// the same tolerance `settings::load` applies to `settings.json` — and for a
/// stronger reason here, because this file holds one recoverable string and
/// nothing a user would mourn.
///
/// **Corrupt means semantically corrupt, not just unparseable.** `save_state`
/// only ever writes a rooted path, so a value that is empty or relative did not
/// come from here, and honouring it would be worse than ignoring it: `""`
/// resolves to the working directory rather than falling through, and a relative
/// entry would name a different file from every directory the user ran in — a
/// selection that is supposed to be durable quietly becoming one that is not.
pub fn load_state() -> CliState {
	let Ok(path) = state_path() else {
		return CliState::default();
	};
	let Ok(text) = std::fs::read_to_string(&path) else {
		return CliState::default();
	};
	let state: CliState = serde_json::from_str(&text).unwrap_or_default();
	// `Path::is_absolute`, **not** `paths::is_rooted`. The two answer different
	// questions and only one of them is the contract here: `is_rooted` is true for
	// `C:foo` and `\foo` as well, because its job is to stop `join` from silently
	// discarding a base — but neither of those is durable. `C:foo` resolves
	// against drive C's own current directory and `\foo` against whatever the
	// current drive is, so both name a different file depending on where the shell
	// happens to be. `save_state` only ever writes a canonicalised path, so
	// anything that fails this test did not come from here.
	match &state.space {
		Some(entry) if Path::new(entry).is_absolute() => state,
		_ => CliState::default(),
	}
}

/// Writes the CLI's selection atomically.
///
/// `write_atomic` rather than a plain write, for the reason `atomic.rs` gives:
/// the temp-file-and-rename means a crash mid-write leaves the previous state
/// rather than a truncated file. There is no compare-and-swap because there is no
/// second writer to swap against — this file is the CLI's alone.
pub fn save_state(state: &CliState) -> Result<()> {
	let path = state_path()?;
	if let Some(dir) = path.parent() {
		std::fs::create_dir_all(dir)
			.map_err(|err| copper_core::store::error::io_err(dir, "create", &err))?;
	}
	let text = serde_json::to_string_pretty(state)
		.map_err(|err| StoreError::Io(format!("could not encode {}: {err}", path.display())))?;
	store::atomic::write_atomic(&path, &format!("{text}\n"))
}

/// Removes the state file. A file that was never there is already the desired
/// end state, so its absence is not an error.
pub fn clear_state() -> Result<()> {
	let path = state_path()?;
	match std::fs::remove_file(&path) {
		Ok(()) => Ok(()),
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
		Err(err) => Err(copper_core::store::error::io_err(&path, "remove", &err)),
	}
}

// --- the resolution chain ------------------------------------------------------

/// A path as given, made absolute against the invocation's working directory.
///
/// `is_rooted` rather than `Path::is_absolute`, which is false for both `C:x` and
/// `\x` on Windows — and `PathBuf::join` would silently discard the base for
/// either, producing a path that resolves somewhere the user did not name.
pub fn absolute(path: &Path) -> Result<PathBuf> {
	if paths::is_rooted(path) {
		return Ok(path.to_path_buf());
	}
	let cwd = std::env::current_dir()
		.map_err(|err| StoreError::Io(format!("could not read the working directory: {err}")))?;
	Ok(cwd.join(path))
}

/// The space this invocation works on (spec 5).
///
/// Four sources, in order, each one an explicit statement that outranks the ones
/// below it: the flag is about this command, the variable is about this shell,
/// the state file is about this CLI, and the app's active space is a last resort
/// that lets `copper note add` work with no setup at all.
///
/// The app's half is read **fresh every invocation and never written**, through
/// the decode-only loader — a listing command that quarantined `settings.json`
/// or promoted a recents entry would be changing the app's state to answer a
/// question about it.
pub fn space(flag: Option<&Path>) -> Result<PathBuf> {
	if let Some(path) = flag {
		return absolute(path);
	}

	if let Some(value) = std::env::var_os(SPACE_ENV) {
		if !value.is_empty() {
			return absolute(Path::new(&value));
		}
	}

	if let Some(selected) = load_state().space {
		return absolute(Path::new(&selected));
	}

	// `load_read_only`, never `settings::load`: the latter renames the file on bad
	// input, and nothing the CLI does may rename a user's settings.
	if let Ok(path) = settings_path() {
		let loaded = settings::load_read_only(&path);
		if let Some(active) = loaded.settings.active_recent() {
			return Ok(PathBuf::from(active));
		}
	}

	Err(StoreError::Unavailable(format!(
		"no space to work on. Copper looks in four places, in order: the --space \
		 flag, the {SPACE_ENV} environment variable, this CLI's own selection \
		 (`copper space use <path>`), and the space the Copper app has open \
		 (`settings.json`'s active recent). None of them named one."
	)))
}

/// Opens the resolved space headlessly: no settings, no watcher, no events.
pub fn open(space: &Path) -> Result<Store> {
	store::open_headless(space, Arc::new(NullSink))
}

// --- references inside a document ------------------------------------------------

/// A section id from an id or a name (spec 6).
///
/// Not `ops::section_by_name`: that returns the *first* case-insensitive match
/// and has no notion of ambiguity, which is right for the composer — where the
/// user is looking at the list — and wrong for a CLI, where two sections called
/// "Notes" would silently send a note to whichever happened to sort first.
///
/// An id-shaped reference that names nothing falls through to the name path
/// rather than failing on the prefix, because a section may legitimately be
/// *called* `sec_something`.
pub fn section<'a>(space: &'a Space, reference: &str) -> Result<&'a str> {
	if let Some(found) = space
		.sections
		.iter()
		.find(|section| section.id == reference)
	{
		return Ok(&found.id);
	}

	let wanted = entry::normalise_name(reference).to_lowercase();
	let matches: Vec<&copper_core::store::model::Section> = space
		.sections
		.iter()
		.filter(|section| entry::normalise_name(&section.name).to_lowercase() == wanted)
		.collect();

	match matches.as_slice() {
		[] => Err(StoreError::NotFound(format!(
			"no section matches {reference:?}"
		))),
		[one] => Ok(&one.id),
		many => Err(StoreError::Invalid(format!(
			"{reference:?} matches {} sections: {}",
			many.len(),
			many.iter()
				.map(|section| format!("{} ({})", section.name, section.id))
				.collect::<Vec<_>>()
				.join(", ")
		))),
	}
}

/// A note id from an id or an unambiguous prefix of its hex part (spec 6).
///
/// Ids are `nte_` plus eight lowercase hex characters, so a prefix of three or
/// four is almost always unique within a space and is far less to type. An
/// ambiguous prefix is refused with the full list rather than resolved to the
/// first, for the same reason an ambiguous section name is.
pub fn note_id<'a>(space: &'a Space, reference: &str) -> Result<&'a str> {
	let needle = reference
		.strip_prefix("nte_")
		.unwrap_or(reference)
		.to_ascii_lowercase();
	if needle.is_empty() {
		return Err(StoreError::Invalid("a note id cannot be empty".into()));
	}

	let matches: Vec<&str> = space
		.notes
		.iter()
		.map(|note| note.id.as_str())
		.filter(|id| {
			id.strip_prefix("nte_")
				.is_some_and(|hex| hex.starts_with(&needle))
		})
		.collect();

	match matches.as_slice() {
		[] => Err(StoreError::NotFound(format!("no note matches {reference:?}"))),
		[one] => Ok(one),
		many => Err(StoreError::Invalid(format!(
			"{reference:?} matches {} notes: {}",
			many.len(),
			many.join(", ")
		))),
	}
}

/// Every reference resolved, in the order given.
///
/// Resolved one at a time and before anything is called, so a list with one bad
/// reference in it changes nothing — the same validate-completely-first rule
/// `ops.rs` holds to internally.
pub fn note_ids(space: &Space, references: &[String]) -> Result<Vec<String>> {
	references
		.iter()
		.map(|reference| note_id(space, reference).map(str::to_string))
		.collect()
}
