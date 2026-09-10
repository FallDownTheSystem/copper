//! Export attachment blobs into a user-selected directory.

use std::path::{Path, PathBuf};

use super::is_reserved_device_name;
use crate::store::atomic;
use crate::store::error::{io_err, Result, StoreError};

/// Export one attachment without replacing an existing destination file.
pub fn export_one(space: &Path, dir: &Path, file: &str, name: &str) -> Result<PathBuf> {
	export_with_limit(space, dir, file, name, MAX_COLLISION_ATTEMPTS)
}

/// A batch can contain more identical names than an individual export's search
/// budget. Reserve a candidate for every member without making collision search unbounded.
pub fn export_batch(
	space: &Path,
	dir: &Path,
	attachments: &[&crate::store::model::Attachment],
) -> Result<Vec<PathBuf>> {
	let attempts = MAX_COLLISION_ATTEMPTS.saturating_add(attachments.len());
	attachments
		.iter()
		.map(|attachment| {
			export_with_limit(space, dir, &attachment.file, &attachment.name, attempts)
		})
		.collect()
}

fn export_with_limit(
	space: &Path,
	dir: &Path,
	file: &str,
	name: &str,
	attempts: usize,
) -> Result<PathBuf> {
	let bytes = super::read_blob(space, file)?;
	write_without_clobbering(dir, &sanitise(name, file), &bytes, attempts)
}

/// Writes `bytes` as `name`, or as `name (2)`, `name (3)`, … if that is taken.
///
/// `commit_new` at every step rather than an `exists()` check followed by a
/// replacing write: the filesystem is what refuses, so a file that appears
/// between the check and the write has no window in which to be destroyed. That
/// matters more here than anywhere else in the export path, because the destination
/// is a directory of the user's choosing full of files Copper knows nothing about.
///
/// The ` (2)` convention is Windows Explorer's, and goes before the extension so
/// the file still opens in the same application.
fn write_without_clobbering(
	dir: &Path,
	name: &str,
	bytes: &[u8],
	attempts: usize,
) -> Result<PathBuf> {
	let (stem, extension) = split_extension(name);
	// `CommitFailure` hands the prepared file back so a further attempt does not
	// serialise and fsync the same bytes again — the same reason
	// `attachments::write_blob` parks one. Only the *name* changes between
	// attempts, so a directory full of collisions costs one blob write, not a
	// hundred.
	let mut held: Option<atomic::Prepared> = None;

	for attempt in 1..=attempts {
		let candidate = if attempt == 1 {
			name.to_string()
		} else {
			format!("{stem} ({attempt}){extension}")
		};
		let path = dir.join(&candidate);

		let prepared = match held.take() {
			Some(prepared) => prepared,
			None => atomic::prepare_bytes(dir, bytes)?,
		};
		match prepared.commit_new(&path) {
			Ok(()) => return Ok(path),
			Err(failure) if failure.error.kind() == std::io::ErrorKind::AlreadyExists => {
				held = Some(failure.prepared);
			}
			Err(failure) => return Err(io_err(&path, "write", &failure.error)),
		}
	}

	Err(StoreError::Io(format!(
		"{name} and its first {attempts} alternatives already exist in {}",
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
/// This repairs an arbitrary string a user typed on another machine, possibly
/// years ago, into something this filesystem will accept — rejecting would mean
/// refusing to export a file over a colon in its name.
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
	// The store's own table, asked here about a name a user typed rather than one
	// the store minted — the same thirty device names, the same segment before
	// the first dot. Only what we do with the answer differs: `is_bare_filename`
	// refuses, and this falls back to the content-addressed name so the bytes
	// still come out.
	if trimmed.is_empty() || is_reserved_device_name(trimmed) {
		return file.to_string();
	}
	trimmed.to_string()
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
		assert_eq!(
			sanitise("   ", "0123456789abcdef.png"),
			"0123456789abcdef.png"
		);
		assert_eq!(
			sanitise("...", "0123456789abcdef.png"),
			"0123456789abcdef.png"
		);
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
