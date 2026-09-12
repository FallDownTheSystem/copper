//! Export attachment blobs into a user-selected directory.
//!
//! The CLI's `attachment export` is the one caller. The desktop app never
//! exports: its copy, drag and open gestures hand out the stored file itself,
//! because that file already carries the user's name.

use std::path::{Path, PathBuf};

use crate::store::error::Result;

/// Export one attachment without replacing an existing destination file.
///
/// The name is repaired the same way the store repaired it at ingest, so an
/// attachment that arrived from another machine with a colon in its name still
/// comes out. It is not tightened to a bare filename: a destination directory
/// of the user's choosing is allowed a dotfile.
pub fn export_one(space: &Path, dir: &Path, file: &str, name: &str) -> Result<PathBuf> {
	let bytes = super::read_blob(space, file)?;
	super::write_without_clobbering(
		dir,
		&super::sanitise(name, file),
		&bytes,
		super::MAX_COLLISION_ATTEMPTS,
	)
}
