//! The blob layer: the only code that reads or writes a space's assets
//! directory.
//!
//! # The invariant
//!
//! > Every path this module opens is `assets_dir(space_path).join(f)` where `f`
//! > passed [`is_bare_filename`]. No path is ever taken from the document,
//! > joined from user input, or canonicalised into existence.
//!
//! It is enforced at both boundaries, and the **read** side is where it earns
//! its keep: the write side cannot produce a bad name because names are content
//! hashes, whereas a `.copper` file is hand-editable and git-writable, so a
//! `file` of `..\..\Windows\System32\config\SAM` is a thing a reader will
//! actually be handed. [`resolve`] is the one door, and nothing outside this
//! module builds a path into the assets directory.
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
//! collected by [`sweep`] instead, at space close and at startup only — never
//! mid-session, because the undo stack is session-scoped and a sweep during one
//! would silently make an undo unrestorable.
//!
//! Even then the bytes are not destroyed. The sweep moves them to the Recycle
//! Bin, so the last defence against a collection the user did not want is one
//! the OS already provides and already knows how to undo.

pub mod commands;
pub mod thumb;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::diagnostics;
use crate::store::atomic;
use crate::store::error::{io_err, Result, StoreError};
use crate::store::ids;
use crate::store::model::{Attachment, Space};

// --- limits ------------------------------------------------------------------
// Constants, not settings (Open Question 3, answered 2026-08-05). One named
// place each; never a literal at a call site.

/// Per file. A drop or paste over this is refused by name, and the rest of a
/// multi-file drop still proceeds.
pub const ATTACHMENT_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// How many attachments one **submission** may carry.
///
/// Deliberately not enforced on `merge_notes`: merging two full notes produces
/// a list of twenty, and applying the cap there would make the merge either
/// fail or silently drop files the user still has, both worse than a long list.
/// The cap governs what may be *attached*, not what a document may hold.
pub const ATTACHMENT_MAX_PER_NOTE: usize = 10;
/// How long an unreferenced blob is left alone before the sweep may take it.
///
/// The window is what makes the write-bytes-then-write-document ordering in
/// [`ingest`] safe: between the two there is a blob nothing references yet, and
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
	let stem = name.split('.').next().unwrap_or_default();
	!RESERVED_DEVICE_NAMES
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
			"{file} is not a file in this project's attachments"
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

// --- ingest ------------------------------------------------------------------

/// Where the bytes came from, for the one message the caller shows on a refusal.
///
/// The limit is a parameter rather than [`ATTACHMENT_MAX_BYTES`] read from here:
/// `occupant_matches` reads with the *existing file's* length as its bound, and
/// naming the ingest cap in that refusal would report a limit this read never
/// applied.
fn too_large(name: &str, len: u64, limit: u64) -> StoreError {
	StoreError::Invalid(format!(
		"{name} is {} and the limit is {} — it was not attached",
		human_bytes(len),
		human_bytes(limit)
	))
}

/// Sizes in the units a person reading a refusal expects. `pub(crate)` for the
/// clipboard's own too-large refusal, which applies a different ceiling and has
/// to be able to name it in the same words.
pub(crate) fn human_bytes(bytes: u64) -> String {
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

/// Sniffs, size-checks, hashes, writes atomically, and returns the metadata the
/// document will carry.
///
/// All three ingestion paths converge here — paste, drop and picker — which is
/// what makes the size cap, the sniffing rule and the content addressing
/// impossible to apply inconsistently across three affordances.
///
/// **The bytes are written before the document is.** A failure after this
/// returns leaves an orphan blob, which [`sweep`] collects; the reverse order
/// would leave a document referencing a file that does not exist, which is the
/// strictly worse failure because no later pass can repair it.
pub fn ingest(space_path: &Path, bytes: &[u8], original_name: &str) -> Result<Attachment> {
	let len = bytes.len() as u64;
	if len > ATTACHMENT_MAX_BYTES {
		return Err(too_large(original_name, len, ATTACHMENT_MAX_BYTES));
	}
	if bytes.is_empty() {
		return Err(StoreError::Invalid(format!("{original_name} is empty")));
	}

	// Sniffed from the bytes, never taken from the extension: a `.png` that is
	// really an executable must not be stored as, or rendered as, an image.
	let sniffed = infer::get(bytes);
	let mime = sniffed.map_or("application/octet-stream", |kind| kind.mime_type());
	let extension = sniffed.map(|kind| kind.extension()).unwrap_or_default();

	let digest = Sha256::digest(bytes);
	let mut file = hex16(&digest);
	if !extension.is_empty() {
		file.push('.');
		file.push_str(extension);
	}

	let path = resolve(space_path, &file)?;
	let dir = atomic::parent_dir(&path)?;
	std::fs::create_dir_all(dir).map_err(|err| io_err(dir, "create", &err))?;
	write_blob(&path, dir, bytes)?;

	let (width, height) = thumb::dimensions(bytes, mime);
	Ok(Attachment {
		id: ids::new_id(ids::ATTACHMENT),
		file,
		name: original_name.to_string(),
		mime: mime.to_string(),
		bytes: len,
		width,
		height,
	})
}

/// Writes the blob through task-003's atomic helpers, treating a collision as a
/// success.
///
/// A `commit_new` refusal on a content-addressed name means a file with these
/// exact bytes is already there — attaching the same screenshot twice, or two
/// ingests racing — so the desired end state already holds. Reporting it as an
/// error would make the second paste of an identical image fail for no reason,
/// and would make two concurrent ingests of the same bytes a coin flip.
fn write_blob(path: &Path, dir: &Path, bytes: &[u8]) -> Result<()> {
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
fn hex16(digest: &[u8]) -> String {
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

/// Every `file` the document references, whether or not it is a valid name.
///
/// Invalid entries are included deliberately: a blob whose name a corrupt entry
/// happens to spell is still referenced by the user's intent, and the sweep
/// deleting it would turn a repairable typo into lost bytes.
fn referenced(doc: &Space) -> std::collections::HashSet<&str> {
	doc.notes
		.iter()
		.flat_map(|note| note.attachments.iter())
		.map(|attachment| attachment.file.as_str())
		.collect()
}

/// The outcome of getting rid of one blob, spelled out in full because this
/// module's own `Result` is the store's.
type Disposed = std::result::Result<(), String>;

/// Moves a collected blob to the Recycle Bin.
///
/// **Not `fs::remove_file`, and there is deliberately no fallback to it.** Every
/// other failure in and around the sweep is already "log it and leave it for
/// next time", and answering a failed gentle operation with a permanent delete
/// would be the one place in this tree that escalates — reintroducing exactly
/// the unrecoverable outcome this exists to remove. On a volume that can never
/// recycle, the honest cost is that the same blob is retried and re-logged at
/// every space close and every startup and the directory never shrinks. That
/// cost is bounded and visible; a permanent delete's is neither.
///
/// The error is flattened to a `String` so the seam [`sweep_with`] takes stays
/// free of `trash`'s types, which is what lets a test drive it.
fn recycle(path: &Path) -> Disposed {
	trash::delete(path).map_err(|err| err.to_string())
}

/// Moves blobs no note references, and that nothing has touched for
/// [`ORPHAN_GRACE`], to the Recycle Bin.
///
/// Best-effort throughout: every failure is logged and skipped, nothing is
/// propagated, and a missing directory is the ordinary case for a space that
/// has never had an attachment. It must never block a space switch — the user
/// asked to change space, not to tidy a directory.
///
/// **Called with no store lock held.** The caller clones the path and the
/// document out of the store and drops the guard first, following task-006's
/// two-lock discipline: walking a directory while holding the store mutex would
/// stall every capture for the duration of the walk.
pub fn sweep(space_path: &Path, doc: &Space) {
	sweep_with(space_path, doc, recycle);
}

/// [`sweep`], with the disposal step as a parameter.
///
/// The seam is here for the tests and for nothing else — the one production
/// caller passes [`recycle`]. A `cargo test` that reached the real Recycle Bin
/// would deposit a file in the developer's bin on every run, and would go red on
/// a runner whose temp volume has no bin at all, for a reason that has nothing
/// to do with the code. It is also the only way to reach the failure branch
/// below, since making a real trash move fail on demand needs a filesystem a
/// test cannot conjure.
fn sweep_with(space_path: &Path, doc: &Space, dispose: impl Fn(&Path) -> Disposed) {
	let dir = assets_dir(space_path);
	let entries = match std::fs::read_dir(&dir) {
		Ok(entries) => entries,
		// Never had an attachment, or the directory went away with the drive.
		Err(_) => return,
	};

	let live = referenced(doc);
	let now = SystemTime::now();
	for entry in entries.flatten() {
		let name = entry.file_name();
		let Some(name) = name.to_str() else { continue };
		if live.contains(name) {
			continue;
		}
		// A subdirectory, or a temp file some other process is mid-write on. The
		// metadata read failing is itself a reason to leave it alone.
		let Ok(metadata) = entry.metadata() else { continue };
		if !metadata.is_file() {
			continue;
		}
		let old_enough = metadata
			.modified()
			.ok()
			.and_then(|modified| now.duration_since(modified).ok())
			.is_some_and(|age| age >= ORPHAN_GRACE);
		if !old_enough {
			continue;
		}
		let path = entry.path();
		if let Err(err) = dispose(&path) {
			diagnostics::log_error(&format!(
				"[copper] could not move the unreferenced attachment {} to the Recycle Bin, so it \
				 stays where it is until the next sweep: {err}",
				path.display()
			));
		}
	}
}

#[cfg(test)]
mod tests {
	use std::cell::RefCell;

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

	#[test]
	fn the_stored_name_is_the_hash_and_the_sniffed_extension() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		// A one-pixel PNG. The extension on the *original* name disagrees on
		// purpose (AC22): the sniffed type is what decides both.
		let png = one_pixel_png();

		let meta = ingest(&space, &png, "screenshot.jpg").unwrap();

		assert_eq!(meta.mime, "image/png");
		assert!(meta.file.ends_with(".png"), "{}", meta.file);
		assert_eq!(meta.file.len(), 16 + 4);
		assert_eq!(meta.name, "screenshot.jpg", "the original name is kept as metadata");
		assert_eq!(meta.bytes, png.len() as u64);
		assert!(meta.id.starts_with("att_"));
		assert!(assets_dir(&space).join(&meta.file).is_file());
	}

	/// AC3. Content addressing makes the write idempotent, so two pastes of one
	/// screenshot are two entries and one file.
	#[test]
	fn ingesting_identical_bytes_twice_writes_one_file_and_mints_two_entries() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let png = one_pixel_png();

		let first = ingest(&space, &png, "a.png").unwrap();
		let second = ingest(&space, &png, "b.png").unwrap();

		assert_eq!(first.file, second.file, "identical bytes must address the same file");
		assert_ne!(first.id, second.id, "each attachment entry is its own");
		let written: Vec<_> = std::fs::read_dir(assets_dir(&space)).unwrap().collect();
		assert_eq!(written.len(), 1, "a second file was written for identical bytes");
	}

	#[test]
	fn an_unrecognised_type_stores_with_no_extension_and_an_octet_stream_mime() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");

		let meta = ingest(&space, b"just some bytes nobody has a magic number for", "x.png").unwrap();

		assert_eq!(meta.mime, "application/octet-stream");
		assert_eq!(meta.file.len(), 16, "an unsniffable type must not borrow an extension");
		assert_eq!(meta.width, None);
	}

	#[test]
	fn an_oversized_file_is_refused_by_name_and_writes_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");

		let err = ingest(&space, &vec![0u8; ATTACHMENT_MAX_BYTES as usize + 1], "huge.bin").unwrap_err();

		assert_eq!(err.kind(), "invalid");
		assert!(err.message().contains("huge.bin"), "{}", err.message());
		assert!(!assets_dir(&space).exists(), "a refused file created the directory");
	}

	#[test]
	fn image_dimensions_are_recorded_and_non_images_have_none() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");

		let image = ingest(&space, &one_pixel_png(), "a.png").unwrap();
		assert_eq!((image.width, image.height), (Some(1), Some(1)));

		let other = ingest(&space, b"%PDF-1.4\n%not really a pdf\n", "a.pdf").unwrap();
		assert_eq!(other.mime, "application/pdf");
		assert_eq!((other.width, other.height), (None, None));
	}

	// --- sweep ---

	fn space_with(files: &[&str]) -> Space {
		use crate::store::model::{Note, Section};
		Space {
			id: "spc_00000001".into(),
			name: "test".into(),
			active_section: "sec_a".into(),
			sections: vec![Section {
				id: "sec_a".into(),
				name: "Notes".into(),
				order: 0,
			}],
			notes: vec![Note {
				id: "nte_1".into(),
				section: "sec_a".into(),
				order: 0,
				done: false,
				body: "a note".into(),
				attachments: files
					.iter()
					.map(|file| Attachment {
						id: "att_1".into(),
						file: (*file).to_string(),
						name: (*file).to_string(),
						mime: "image/png".into(),
						bytes: 1,
						width: None,
						height: None,
					})
					.collect(),
				created: "2026-08-04T14:12:33Z".into(),
				updated: "2026-08-04T14:12:33Z".into(),
			}],
		}
	}

	/// Backdates a file so the grace window has demonstrably elapsed, rather
	/// than making the test sleep for a day.
	fn age(path: &Path) {
		let old = SystemTime::now() - ORPHAN_GRACE - Duration::from_secs(60);
		let file = std::fs::File::options().write(true).open(path).unwrap();
		file.set_modified(old).unwrap();
	}

	/// Stands in for the Recycle Bin: records what it was handed and takes the
	/// file out of the assets directory, which is the half of a trash move the
	/// sweep's own assertions are about. The other half — that the bytes are
	/// still recoverable afterwards — belongs to the OS, and no test here
	/// exercises it: nothing in this suite may put a file in the real Recycle
	/// Bin. AC2 is verified by hand, once, against a running build.
	fn recorder(seen: &RefCell<Vec<PathBuf>>) -> impl Fn(&Path) -> Disposed + '_ {
		|path| {
			seen.borrow_mut().push(path.to_path_buf());
			std::fs::remove_file(path).map_err(|err| err.to_string())
		}
	}

	/// What the disposer was handed, by name, in the order it was handed them.
	fn names(seen: &RefCell<Vec<PathBuf>>) -> Vec<String> {
		seen.borrow()
			.iter()
			.map(|path| path.file_name().unwrap_or_default().to_string_lossy().into_owned())
			.collect()
	}

	/// AC13, both halves: an old orphan goes and a young one stays. Task-015
	/// adds the third assertion — the set the disposer is handed is exactly the
	/// set that goes, so a change to the delete mechanism cannot reach a blob
	/// the gates above it meant to keep.
	#[test]
	fn the_sweep_collects_old_orphans_and_leaves_referenced_and_young_ones() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let assets = assets_dir(&space);
		std::fs::create_dir_all(&assets).unwrap();
		for name in ["kept.png", "old-orphan.png", "young-orphan.png"] {
			std::fs::write(assets.join(name), b"x").unwrap();
		}
		age(&assets.join("kept.png"));
		age(&assets.join("old-orphan.png"));

		let seen = RefCell::new(Vec::new());
		sweep_with(&space, &space_with(&["kept.png"]), recorder(&seen));

		assert_eq!(names(&seen), ["old-orphan.png"], "the wrong set reached the bin");
		assert!(assets.join("kept.png").exists(), "a referenced blob was collected");
		assert!(
			assets.join("young-orphan.png").exists(),
			"an orphan inside the grace window was collected"
		);
		assert!(
			!assets.join("old-orphan.png").exists(),
			"an old orphan survived the sweep"
		);
	}

	/// Task-015 AC3. A blob the bin refuses stays exactly where it is — there is
	/// no falling back to a permanent delete — and the refusal does not stop the
	/// sweep from reaching the orphans behind it.
	#[test]
	fn a_refused_move_leaves_the_blob_alone_and_the_sweep_carries_on() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let assets = assets_dir(&space);
		std::fs::create_dir_all(&assets).unwrap();
		for name in ["a-orphan.png", "b-orphan.png"] {
			std::fs::write(assets.join(name), b"x").unwrap();
			age(&assets.join(name));
		}

		let seen = RefCell::new(Vec::new());
		sweep_with(&space, &space_with(&[]), |path| {
			seen.borrow_mut().push(path.to_path_buf());
			if path.ends_with("a-orphan.png") {
				// What a volume with no Recycle Bin reports, near enough.
				return Err("the operation is not supported here".to_string());
			}
			std::fs::remove_file(path).map_err(|err| err.to_string())
		});

		// Order-independent: whichever of the two `read_dir` yields first, both
		// were attempted, which is the property — one refusal did not end the walk.
		assert_eq!(names(&seen).len(), 2, "a refusal stopped the sweep short");
		assert!(
			assets.join("a-orphan.png").exists(),
			"a blob the bin refused was deleted anyway"
		);
		assert!(
			!assets.join("b-orphan.png").exists(),
			"the orphan behind the refusal was never collected"
		);
	}

	#[test]
	fn sweeping_a_space_with_no_assets_directory_is_a_no_op() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let seen = RefCell::new(Vec::new());
		sweep_with(&space, &space_with(&[]), recorder(&seen));
		assert!(!assets_dir(&space).exists());
		assert!(names(&seen).is_empty());
	}

	/// A corrupt `file` value protects the blob it names rather than condemning
	/// it: the entry is repairable by hand, the bytes are not recoverable.
	#[test]
	fn a_blob_named_by_an_invalid_entry_is_not_collected() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let assets = assets_dir(&space);
		std::fs::create_dir_all(&assets).unwrap();
		std::fs::write(assets.join("kept.png"), b"x").unwrap();
		age(&assets.join("kept.png"));

		// The document spells it as an escaping path, which `resolve` refuses —
		// but the intent still names these bytes.
		let seen = RefCell::new(Vec::new());
		sweep_with(&space, &space_with(&[r"..\kept.png", "kept.png"]), recorder(&seen));

		assert!(assets.join("kept.png").exists());
		assert!(names(&seen).is_empty());
	}

	/// The smallest valid PNG: 1×1, 8-bit greyscale.
	fn one_pixel_png() -> Vec<u8> {
		let mut buffer = std::io::Cursor::new(Vec::new());
		image::DynamicImage::new_luma8(1, 1)
			.write_to(&mut buffer, image::ImageFormat::Png)
			.unwrap();
		buffer.into_inner()
	}
}
