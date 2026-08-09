//! The blob layer: the only code that turns a document's `file` value into a
//! path, and the only code that reads or writes a blob under one.
//!
//! # The invariant
//!
//! > Every path built from a document is `assets_dir(space_path).join(f)` where
//! > `f` passed [`is_bare_filename`]. No path is ever taken from the document,
//! > joined from user input, or canonicalised into existence.
//!
//! The sweep in the `copper` crate also walks the assets directory, and does not
//! weaken this: it enumerates the directory rather than resolving a name out of
//! the document, so it never needs a door this module would have to open.
//!
//! It is enforced at both boundaries, and the **read** side is where it earns
//! its keep: the write side cannot produce a bad name because names are content
//! hashes, whereas a `.copper` file is hand-editable and git-writable, so a
//! `file` of `..\..\Windows\System32\config\SAM` is a thing a reader will
//! actually be handed. [`resolve`] is the one door, and nothing anywhere turns a
//! document's `file` value into a path except by walking through it.
//!
//! # Why the bytes are not in the JSON
//!
//! Task-003 makes two promises this would break. The document is serialised
//! byte-stably and diffs minimally under git — a base64 screenshot is a
//! multi-megabyte single-line diff — and undo snapshots whole documents fifty
//! deep, which would multiply every attachment by fifty in memory. Metadata in
//! the document and bytes in a sidecar directory keeps both.
//!
//! # Why blobs outlive the notes that reference them
//!
//! Undo restores a *document*, not a filesystem. Deleting a note and pressing
//! `Ctrl+Z` can only bring its attachments back if the bytes are still there, so
//! **nothing on a mutation path deletes a blob**. Unreferenced bytes are
//! collected by the sweep instead, at space close and at startup only — never
//! mid-session, because the undo stack is session-scoped and a sweep during one
//! would silently make an undo unrestorable.
//!
//! Even then the bytes are not destroyed. The sweep *renames* an orphan into a
//! [`COLLECTED_DIR`] directory beside it and stops there — **nothing in Copper
//! ever deletes a blob**. The accepted cost is that the directory only grows;
//! emptying it is a manual lever the user pulls, and there is no auto-purge.
//!
//! # What is here, and what is not
//!
//! Everything above that is a *path* is here. The two halves that are not are
//! `ingest` and the sweep itself, which live in the `copper` crate's own
//! `attachments` module: ingest reads an image's dimensions through the `image`
//! crate, and the sweep logs its failures through the app's `diagnostics`. Both
//! reach back into this module for the rules — [`resolve`], [`write_blob`],
//! [`assets_dir`], [`ORPHAN_GRACE`], [`COLLECTED_DIR`] — so the invariant above
//! is stated once even though it is applied on both sides of the boundary.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::store::atomic;
use crate::store::error::{io_err, Result, StoreError};

// --- limits ------------------------------------------------------------------
// Constants, not settings (Open Question 3, answered 2026-08-05). One named
// place each; never a literal at a call site.

/// Per file. A drop or paste over this keeps its path as a note instead of its
/// bytes, and the rest of a multi-file drop still attaches.
///
/// 32 MiB, raised from 10 on 2026-08-09 by user request. Still a constant and
/// not a setting: blobs are content-addressed copies inside the space's own
/// sidecar, so this number is the ceiling on what a shared or synced space
/// quietly grows by per file.
pub const ATTACHMENT_MAX_BYTES: u64 = 32 * 1024 * 1024;
/// How many attachments one **submission** may carry.
///
/// Deliberately not enforced on `merge_notes`: merging two full notes produces
/// a list of twenty, and applying the cap there would make the merge either
/// fail or silently drop files the user still has, both worse than a long list.
/// The cap governs what may be *attached*, not what a document may hold.
pub const ATTACHMENT_MAX_PER_NOTE: usize = 10;
/// How long an unreferenced blob is left alone before the sweep may take it.
///
/// The window is what makes the write-bytes-then-write-document ordering in the
/// ingest path safe: between the two there is a blob nothing references yet, and
/// an abandoned composer draft leaves one indefinitely. Hours rather than
/// minutes because a draft can sit in the tray across a lunch break.
pub const ORPHAN_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// The suffix appended to the space file's **full name**, so the directory
/// travels with the space and cannot collide with a sibling space in the same
/// folder: `notes.copper` and `notes2.copper` get different sidecars, whereas a
/// stem-based name would give `notes.copper` and `notes.md` the same one.
const ASSETS_SUFFIX: &str = ".assets";

// --- paths -------------------------------------------------------------------

/// `D:\x\notes.copper` → `D:\x\notes.copper.assets`.
pub fn assets_dir(space_path: &Path) -> PathBuf {
	let mut name = space_path.file_name().unwrap_or_default().to_os_string();
	name.push(ASSETS_SUFFIX);
	space_path.with_file_name(name)
}

/// Windows device names, which are reserved **with or without an extension** and
/// in any case. `CON.png` opens the console, not a file.
/// `CONIN$` and `CONOUT$` are on the list because they are console devices like
/// `CON`, and the superscript `COM¹`/`LPT²` forms are on it because Windows
/// really does resolve those digits — a name that looks like ordinary text and
/// opens a serial port is exactly the kind of thing this table exists for.
const RESERVED_DEVICE_NAMES: [&str; 30] = [
	"CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
	"COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
	"LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Whether `name` is a plain filename that can only ever mean a child of the
/// directory it is joined to.
///
/// Rejects separators in both spellings, a drive or ADS colon, NUL, `..` and
/// `.`, a leading or trailing dot or space (Windows silently strips trailing
/// ones, so `x.png. ` and `x.png` name the same file and the round trip stops
/// agreeing with itself), the empty string, and the reserved device names.
///
/// A pure function with a table test, called on read as well as on write.
pub fn is_bare_filename(name: &str) -> bool {
	if name.is_empty() || name.len() > 255 {
		return false;
	}
	if name.contains(['/', '\\', ':', '\0']) {
		return false;
	}
	if name.starts_with('.') || name.starts_with(' ') || name.ends_with('.') || name.ends_with(' ') {
		return false;
	}
	// Control characters are legal in a POSIX filename and illegal on NTFS, and a
	// name carrying one is far more likely to be an injection attempt than a file.
	if name.chars().any(|ch| ch.is_control()) {
		return false;
	}
	!is_reserved_device_name(name)
}

/// Whether `name`'s first segment is one of [`RESERVED_DEVICE_NAMES`].
///
/// Split out and made public because a second front end asks the same question
/// and answers it differently. [`is_bare_filename`] *rejects* a name the store
/// minted; `copper-cli`'s attachment export *repairs* a user's original
/// filename, so it needs the test without the rejection. Thirty names written
/// out twice is how one copy comes to be missing `CONIN$`.
///
/// **The segment before the first dot**, not the last: `COM1.foo.bar` is
/// reserved, and a stem taken from the final dot would test `COM1.foo` and miss
/// it.
pub fn is_reserved_device_name(name: &str) -> bool {
	let stem = name.split('.').next().unwrap_or_default();
	RESERVED_DEVICE_NAMES
		.iter()
		.any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// The one door into the assets directory.
///
/// Takes the space path and a `file` value straight off the document and either
/// returns a path inside `assets_dir(space_path)` or refuses. There is no other
/// way to build one, which is what makes "no file outside the assets directory
/// is opened or read" a property of the code rather than of every call site
/// remembering to check.
pub fn resolve(space_path: &Path, file: &str) -> Result<PathBuf> {
	if !is_bare_filename(file) {
		return Err(invalid_file_name(file));
	}
	Ok(assets_dir(space_path).join(file))
}

/// The one wording for a `file` value that is not a bare filename.
///
/// The *check* happens in two places by design — here on the way to a path, and
/// in `ops::clean_attachments` on the way into the document — because a name is
/// validated wherever it is received. The *refusal* is spelled once, because two
/// wordings of one rule read as two different problems.
pub fn invalid_file_name(file: &str) -> StoreError {
	StoreError::Invalid(format!("{file:?} is not a valid attachment file name"))
}

/// [`resolve`], plus the checks that only make sense for a path something is
/// about to *read* or hand to the shell.
///
/// `resolve` cannot make them itself: it is also the write path, where the file
/// legitimately does not exist yet.
///
/// **`symlink_metadata`, not `metadata`.** A `.copper` space can arrive from a
/// git remote, and git can create symlinks — so a `file` naming a link inside
/// the assets directory would otherwise resolve to a bare filename, pass every
/// check, and then read or launch whatever it points at. Following the link is
/// the whole attack; refusing anything that is not a regular file removes it,
/// and costs nothing real, because every blob this app writes is a regular file
/// it created itself.
pub fn resolve_existing(space_path: &Path, file: &str) -> Result<PathBuf> {
	let path = resolve(space_path, file)?;
	let metadata = std::fs::symlink_metadata(&path).map_err(|err| io_err(&path, "read", &err))?;
	if !metadata.is_file() {
		return Err(StoreError::Invalid(format!(
			"{file} is not a file in this space's attachments"
		)));
	}
	Ok(path)
}

/// Reads at most `limit` bytes, and fails rather than truncating if there are
/// more.
///
/// A `metadata().len()` check followed by an unbounded `read` is a TOCTOU: the
/// file can grow between the two, and on Windows a named pipe or a device
/// reports a length of zero and then hands over as much as it likes. Reading
/// `limit + 1` and refusing when the extra byte arrives makes the bound a
/// property of the read itself.
pub fn read_capped(path: &Path, limit: u64, name: &str) -> Result<Vec<u8>> {
	// `limit + 1`: the extra byte arriving is the refusal signal, and it is what
	// makes the bound a property of the read rather than of a length check.
	let bytes = read_take(path, limit + 1)?;
	if bytes.len() as u64 > limit {
		return Err(too_large(name, bytes.len() as u64, limit));
	}
	Ok(bytes)
}

/// The first `limit` bytes, for sniffing. A short file is not an error.
pub fn read_prefix(path: &Path, limit: u64) -> Result<Vec<u8>> {
	read_take(path, limit)
}

/// At most `take` bytes, however long the file claims to be.
///
/// The bound is applied by the reader itself, which is the point: a length read
/// followed by an unbounded read is a TOCTOU, and on Windows a pipe or a device
/// reports zero and then hands over as much as it likes.
fn read_take(path: &Path, take: u64) -> Result<Vec<u8>> {
	use std::io::Read;

	let file = std::fs::File::open(path).map_err(|err| io_err(path, "read", &err))?;
	// A capacity hint and nothing more — the `take` below is still the bound. A
	// `Take<File>` does not inherit `File`'s `read_to_end` size hint, so without
	// this a ten-megabyte blob is twenty reallocations and twice its own size in
	// memcpy. Clamped to `take` so a lying length cannot reserve past what the
	// read would accept.
	let hint = file.metadata().map_or(0, |meta| meta.len().min(take)) as usize;
	let mut bytes = Vec::with_capacity(hint + 1);
	file.take(take)
		.read_to_end(&mut bytes)
		.map_err(|err| io_err(path, "read", &err))?;
	Ok(bytes)
}

/// Enough for every magic number `infer` knows; the longest it inspects is a
/// few hundred bytes.
pub const SNIFF_PREFIX_BYTES: u64 = 8 * 1024;

/// The mime the **bytes** say they are, never the extension and never the
/// document's `mime` field.
///
/// The default carries as much weight as the sniff: an unrecognised type is
/// `application/octet-stream`, which `thumb::is_thumbnailable` refuses — so a
/// `.png` that is really an executable is neither decoded nor launched (AC22).
/// One function, so the rule and its default cannot drift apart between the
/// places that apply it.
pub fn sniff_mime(bytes: &[u8]) -> &'static str {
	infer::get(bytes).map_or("application/octet-stream", |kind| kind.mime_type())
}

// --- writing -----------------------------------------------------------------

/// Where the bytes came from, for the one message the caller shows on a refusal.
///
/// The limit is a parameter rather than [`ATTACHMENT_MAX_BYTES`] read from here:
/// `occupant_matches` reads with the *existing file's* length as its bound, and
/// naming the ingest cap in that refusal would report a limit this read never
/// applied.
///
/// `pub` for the ingest path in the `copper` crate, which applies the same cap
/// to bytes it has in memory rather than to a read.
pub fn too_large(name: &str, len: u64, limit: u64) -> StoreError {
	StoreError::Invalid(format!(
		"{name} is {} and the limit is {}, so it was not attached",
		human_bytes(len),
		human_bytes(limit)
	))
}

/// Sizes in the units a person reading a refusal expects. `pub` for the
/// clipboard's own too-large refusal, which applies a different ceiling and has
/// to be able to name it in the same words.
pub fn human_bytes(bytes: u64) -> String {
	const MIB: u64 = 1024 * 1024;
	const KIB: u64 = 1024;
	if bytes >= MIB {
		format!("{:.1} MB", bytes as f64 / MIB as f64)
	} else if bytes >= KIB {
		format!("{} KB", bytes.div_ceil(KIB))
	} else {
		format!("{bytes} bytes")
	}
}

/// Writes the blob through task-003's atomic helpers, treating a collision as a
/// success.
///
/// A `commit_new` refusal on a content-addressed name means a file with these
/// exact bytes is already there — attaching the same screenshot twice, or two
/// ingests racing — so the desired end state already holds. Reporting it as an
/// error would make the second paste of an identical image fail for no reason,
/// and would make two concurrent ingests of the same bytes a coin flip.
///
/// `pub` for the ingest path in the `copper` crate: the collision rule and the
/// backoff belong to the blob layer, not to the caller that happens to decode
/// images.
pub fn write_blob(path: &Path, dir: &Path, bytes: &[u8]) -> Result<()> {
	let mut held: Option<atomic::Prepared> = None;
	atomic::with_backoff(|| {
		let prepared = match held.take() {
			Some(prepared) => prepared,
			None => match atomic::prepare_bytes(dir, bytes) {
				Ok(prepared) => prepared,
				Err(err) => return atomic::Attempt::Failed(err),
			},
		};
		match prepared.commit_new(path) {
			Ok(()) => atomic::Attempt::Done(()),
			Err(failure) if failure.error.kind() == std::io::ErrorKind::AlreadyExists => {
				match occupant_matches(path, bytes) {
					Ok(()) => atomic::Attempt::Done(()),
					Err(err) => atomic::Attempt::Failed(err),
				}
			}
			Err(failure) => atomic::classify_commit_failure(path, failure, &mut held),
		}
	})
}

/// Whether the file already at `path` really is the one we were about to write.
///
/// The collision is *usually* the same screenshot attached twice, and treating
/// it as success is what makes ingestion idempotent. But "usually" is not a
/// security property: sixteen hex characters is 64 bits, so a prefix collision
/// is engineerable, and — far more mundanely — the occupant could be a
/// directory, a symlink, or a file some other program happened to leave there.
/// Accepting any of those on the strength of the name alone would let a note
/// reference bytes nobody checked.
///
/// So the occupant is verified: a regular file, the right length, the right
/// bytes. Anything else fails the ingest rather than silently adopting it.
fn occupant_matches(path: &Path, bytes: &[u8]) -> Result<()> {
	let metadata = std::fs::symlink_metadata(path).map_err(|err| io_err(path, "read", &err))?;
	let mismatch = || {
		StoreError::Io(format!(
			"{} already exists and is not the file being attached",
			path.display()
		))
	};
	if !metadata.is_file() || metadata.len() != bytes.len() as u64 {
		return Err(mismatch());
	}
	// Compared in full rather than trusting the length: the length agreeing is
	// what a deliberate collision would arrange first.
	let existing = read_capped(path, bytes.len() as u64, "the existing attachment")?;
	if existing == bytes {
		Ok(())
	} else {
		Err(mismatch())
	}
}

/// The first 16 hex characters of the digest.
///
/// Eight bytes of SHA-256. Enough that a collision between two files one person
/// attaches is not a thing that happens, and short enough that the directory
/// stays readable in Explorer — which is half the reason the sidecar is a real
/// directory rather than a container.
///
/// `pub` for the ingest path and for the preview cache, which names its files
/// the same way over a different input — a URL rather than the bytes. The
/// reasoning above about length and readability is what the two share, and a
/// second copy of it would be a second place for the two directories to stop
/// looking alike.
pub fn hex16(digest: &[u8]) -> String {
	let head: [u8; 8] = digest[..8].try_into().expect("a SHA-256 digest is 32 bytes");
	format!("{:016x}", u64::from_be_bytes(head))
}

// --- reading -----------------------------------------------------------------

/// Reads a blob for the thumbnail path, refusing anything over the ingest cap.
///
/// The cap is re-applied on read rather than trusted from `bytes` in the
/// document: that field is hand-editable, and a decoder is exactly the wrong
/// place to discover that it lied.
pub fn read_blob(space_path: &Path, file: &str) -> Result<Vec<u8>> {
	let path = resolve_existing(space_path, file)?;
	read_capped(&path, ATTACHMENT_MAX_BYTES, file)
}

// --- sweep -------------------------------------------------------------------

/// Where a collected blob goes: a directory inside the assets directory.
///
/// The leading dot does real work. [`is_bare_filename`] rejects any name that
/// starts with one, so no document can name this directory and [`resolve`] can
/// never build a path into it — the quarantine is unreachable from every reader
/// in the module by the same rule that keeps readers inside the assets
/// directory at all.
///
/// The sweep that fills it is in the `copper` crate; the name is here because it
/// is part of the sidecar's layout, which this module defines.
pub const COLLECTED_DIR: &str = ".collected";

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_assets_directory_sits_beside_the_space_and_keeps_its_full_name() {
		assert_eq!(
			assets_dir(Path::new(r"D:\Projects\acme\notes.copper")),
			PathBuf::from(r"D:\Projects\acme\notes.copper.assets")
		);
		// The *full* name, not the stem: a stem-based suffix would give
		// `notes.copper` and a sibling `notes.md` the same sidecar.
		assert_ne!(
			assets_dir(Path::new(r"D:\x\notes.copper")),
			assets_dir(Path::new(r"D:\x\notes.md"))
		);
	}

	#[test]
	fn bare_filenames_are_accepted() {
		for name in [
			"3f9a1c0e7b2d5481.png",
			"3f9a1c0e7b2d5481",
			"a.b.c.png",
			"CONSOLE.png",
			"COM10.png",
			"NULL.txt",
		] {
			assert!(is_bare_filename(name), "{name:?} should be accepted");
		}
	}

	#[test]
	fn everything_that_could_leave_the_directory_is_rejected() {
		for name in [
			"",
			".",
			"..",
			r"..\..\Windows\System32\config\SAM",
			"../../etc/passwd",
			r"C:\x.txt",
			"C:x.txt",
			"x.txt:stream",
			"sub/dir.png",
			r"sub\dir.png",
			".hidden",
			"trailing.",
			"trailing ",
			" leading",
			"nul\0byte.png",
			"bell\u{7}.png",
		] {
			assert!(!is_bare_filename(name), "{name:?} should be rejected");
		}
	}

	#[test]
	fn windows_reserved_device_names_are_rejected_with_and_without_an_extension() {
		for name in ["CON", "con", "NUL.png", "nul.PNG", "aux", "COM1.txt", "LPT9"] {
			assert!(!is_bare_filename(name), "{name:?} is a reserved device name");
		}
	}

	/// AC9, asserted on the resolver directly rather than inferred from a
	/// command's behaviour.
	#[test]
	fn resolve_refuses_every_escaping_name_and_joins_the_rest() {
		let space = Path::new(r"D:\x\notes.copper");
		for file in [
			r"..\..\Windows\System32\config\SAM",
			r"C:\x.txt",
			"nul\0byte.png",
			"..",
		] {
			assert!(resolve(space, file).is_err(), "{file:?} resolved to a path");
		}

		let resolved = resolve(space, "3f9a1c0e7b2d5481.png").unwrap();
		assert_eq!(
			resolved,
			PathBuf::from(r"D:\x\notes.copper.assets\3f9a1c0e7b2d5481.png")
		);
		assert!(resolved.starts_with(assets_dir(space)));
	}

	/// The gate every command that hands a path to the shell stands behind —
	/// `attachment_open` and `attachment_reveal` both resolve through it and
	/// nothing else. `resolve` alone is not enough for them: it is the write path
	/// too, where the file legitimately does not exist yet.
	#[test]
	fn resolve_existing_accepts_only_a_regular_file_already_in_the_directory() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let assets = assets_dir(&space);
		std::fs::create_dir_all(assets.join("subdir")).unwrap();
		std::fs::write(assets.join("blob.png"), b"x").unwrap();

		assert_eq!(
			resolve_existing(&space, "blob.png").unwrap(),
			assets.join("blob.png")
		);
		// A directory wearing a bare filename is not something to reveal or launch.
		assert!(resolve_existing(&space, "subdir").is_err());
		assert!(resolve_existing(&space, "absent.png").is_err());
		// Refused before the filesystem is ever consulted.
		assert!(resolve_existing(&space, r"..\..\Windows\System32\config\SAM").is_err());
	}
}
