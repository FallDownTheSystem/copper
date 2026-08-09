//! `copper attachment export` — a note's files, copied out under their original
//! names.
//!
//! Reads only. It never touches the sidecar beyond opening blobs through
//! `resolve_existing`, and it **never calls `attachments::sweep`** — sweep moves
//! unreferenced blobs to quarantine, and a CLI cannot know whether a running
//! app's undo stack is holding snapshots that still reference them.

use std::path::{Path, PathBuf};

use copper_core::attachments;
use copper_core::store::error::{io_err, Result, StoreError};
use copper_core::store::{atomic, path_string, Store};

use crate::output::{ExportRow, FailedExport, Report};
use crate::resolve;

pub fn run(store: &Store, reference: &str, out: Option<&Path>) -> Result<Report> {
	let space_path = store.require_active_path()?;
	let space = store.active_space()?;
	let id = resolve::note_id(&space, reference)?;
	let note = space
		.note(id)
		.ok_or_else(|| StoreError::NotFound(format!("no note {id}")))?;

	let target = match out {
		Some(dir) => resolve::absolute(dir)?,
		None => std::env::current_dir()
			.map_err(|err| StoreError::Io(format!("could not read the working directory: {err}")))?,
	};
	if !note.attachments.is_empty() {
		std::fs::create_dir_all(&target).map_err(|err| io_err(&target, "create", &err))?;
	}

	let mut exported = Vec::new();
	let mut failed = Vec::new();

	for attachment in &note.attachments {
		// One attachment's failure must not sink the rest: the command exports
		// everything on a note in one call, and a single missing blob is a reason
		// to report that blob, not to abandon the other nine files the user asked
		// for. The exit code below still says the command did not fully succeed.
		match export_one(&space_path, &target, &attachment.file, &attachment.name) {
			Ok(written) => exported.push(ExportRow {
				name: attachment.name.clone(),
				path: path_string(&written),
				bytes: attachment.bytes,
			}),
			Err(err) => failed.push(FailedExport {
				name: attachment.name.clone(),
				message: err.message(),
			}),
		}
	}

	Ok(Report::Export { exported, failed })
}

/// `read_blob` is the only door in, and it is enough: it calls `resolve_existing`
/// itself, which is what enforces the module's invariant that every path it opens
/// is `assets_dir(space).join(f)` for an `f` that passed `is_bare_filename`, and
/// that the result is a regular file rather than a link out of the sidecar.
fn export_one(space: &Path, dir: &Path, file: &str, name: &str) -> Result<PathBuf> {
	let bytes = attachments::read_blob(space, file)?;
	write_without_clobbering(dir, &sanitise(name, file), &bytes)
}

/// Writes `bytes` as `name`, or as `name (2)`, `name (3)`, … if that is taken.
///
/// `commit_new` at every step rather than an `exists()` check followed by a
/// replacing write: the filesystem is what refuses, so a file that appears
/// between the check and the write has no window in which to be destroyed. That
/// matters more here than anywhere else in the CLI, because the destination is a
/// directory of the user's choosing full of files Copper knows nothing about.
///
/// The ` (2)` convention is Windows Explorer's, and goes before the extension so
/// the file still opens in the same application.
fn write_without_clobbering(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
	let (stem, extension) = split_extension(name);

	for attempt in 1..=MAX_COLLISION_ATTEMPTS {
		let candidate = if attempt == 1 {
			name.to_string()
		} else {
			format!("{stem} ({attempt}){extension}")
		};
		let path = dir.join(&candidate);

		match atomic::prepare_bytes(dir, bytes)?.commit_new(&path) {
			Ok(()) => return Ok(path),
			Err(failure) if failure.error.kind() == std::io::ErrorKind::AlreadyExists => continue,
			Err(failure) => return Err(io_err(&path, "write", &failure.error)),
		}
	}

	Err(StoreError::Io(format!(
		"{name} and its first {MAX_COLLISION_ATTEMPTS} alternatives already exist in {}",
		dir.display()
	)))
}

/// Enough that an ordinary directory never reaches it, few enough that a
/// pathological one fails in a moment instead of spinning.
const MAX_COLLISION_ATTEMPTS: usize = 100;

fn split_extension(name: &str) -> (&str, &str) {
	match name.rfind('.') {
		// A leading dot is the whole name of a dotfile, not an extension.
		Some(at) if at > 0 => (&name[..at], &name[at..]),
		_ => (name, ""),
	}
}

/// A user's original filename, made safe for a destination directory.
///
/// This is CLI-local rather than a call to `attachments::is_bare_filename`, and
/// the difference is what each one is for. That function *validates* a name the
/// store itself minted, inside the assets directory, and rejecting is the right
/// answer there. This one *repairs* an arbitrary string a user typed on another
/// machine, possibly years ago, into something this filesystem will accept —
/// rejecting would mean refusing to export a file over a colon in its name.
///
/// Falls back to the content-addressed `file` name when sanitising leaves
/// nothing, so an attachment named `...` still comes out with its bytes intact
/// under a name that is at least unique.
fn sanitise(name: &str, file: &str) -> String {
	let cleaned: String = name
		.chars()
		.map(|ch| {
			if ch < ' ' || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
				'_'
			} else {
				ch
			}
		})
		.collect();

	// Windows drops trailing dots and spaces silently, so a name ending in one
	// would be created under a *different* name than the one reported.
	let trimmed = cleaned.trim().trim_end_matches(['.', ' ']).trim();
	if trimmed.is_empty() || is_reserved(trimmed) {
		return file.to_string();
	}
	trimmed.to_string()
}

/// The DOS device names, which are still not usable as filenames and fail with an
/// error that names none of this.
///
/// The list and the split both match `attachments::is_bare_filename`'s, which is
/// the same question asked about the store's own names. Two details are easy to
/// get wrong and both are worth stating:
///
/// - **The segment before the *first* dot**, not the last. `COM1.foo.bar` is
///   reserved; taking the stem before the final dot would test `COM1.foo` and
///   miss it.
/// - **The console handles and the superscript forms.** `CONIN$`, `CONOUT$` and
///   `COM¹`/`LPT²` are device names too, and a user's original filename is far
///   more likely to hold one by accident than a name the store minted.
fn is_reserved(name: &str) -> bool {
	const RESERVED: [&str; 30] = [
		"CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5",
		"COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4",
		"LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
	];
	let stem = name.split('.').next().unwrap_or_default();
	RESERVED
		.iter()
		.any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn illegal_characters_become_underscores() {
		assert_eq!(sanitise("a/b:c?.png", "abc.png"), "a_b_c_.png");
	}

	#[test]
	fn a_name_that_sanitises_to_nothing_falls_back_to_the_blob_name() {
		assert_eq!(sanitise("   ", "0123456789abcdef.png"), "0123456789abcdef.png");
		assert_eq!(sanitise("...", "0123456789abcdef.png"), "0123456789abcdef.png");
	}

	/// Windows would create `report` and report `report.`, so the two names the
	/// user sees would disagree.
	#[test]
	fn trailing_dots_and_spaces_go() {
		assert_eq!(sanitise("report. ", "x.png"), "report");
	}

	#[test]
	fn a_device_name_falls_back_rather_than_failing_at_the_filesystem() {
		assert_eq!(sanitise("CON.txt", "beef.txt"), "beef.txt");
		assert_eq!(sanitise("con", "beef.txt"), "beef.txt");
		// The segment before the *first* dot, or this one escapes.
		assert_eq!(sanitise("COM1.foo.bar", "beef.txt"), "beef.txt");
		assert_eq!(sanitise("CONIN$", "beef.txt"), "beef.txt");
		assert_eq!(sanitise("LPT¹.png", "beef.txt"), "beef.txt");
		// A name that merely starts with one is an ordinary name.
		assert_eq!(sanitise("console.txt", "beef.txt"), "console.txt");
		assert_eq!(sanitise("COM10.txt", "beef.txt"), "COM10.txt");
	}

	#[test]
	fn the_collision_suffix_goes_before_the_extension() {
		assert_eq!(split_extension("report.pdf"), ("report", ".pdf"));
		assert_eq!(split_extension("report"), ("report", ""));
		assert_eq!(split_extension(".gitignore"), (".gitignore", ""));
	}
}
