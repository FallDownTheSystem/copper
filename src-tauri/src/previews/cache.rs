//! The preview cache on disk: `<app_config_dir>\previews\<key>.json` for the
//! metadata and `<key>.png` beside it for the picture.
//!
//! # Why this one may be deleted, when the attachment sweep may not
//!
//! `attachments::sweep` renames orphaned blobs into a quarantine and never
//! destroys anything, because a blob is the only copy of something the user
//! chose to keep and the undo stack can still need it. Neither is true here. A
//! preview is *derived* — from a page that is still on the internet — nothing in
//! the undo stack references one, and losing every entry costs a re-fetch and
//! nothing else. So this one *deletes*, and it is named `prune` rather than
//! `sweep` so the two verbs cannot be confused at a call site: a sweep in this
//! codebase never destroys anything, and this does.
//!
//! # Why one file per URL rather than an index
//!
//! Previews arrive one at a time, several seconds apart, as a note scrolls into
//! view. A single index would be rewritten in full on each arrival — and a
//! rewrite that raced another arrival would lose it. Per-URL files make each
//! write independent and each one atomic on its own.
//!
//! # Freshness is the file's own mtime
//!
//! No `fetched` field: a rename sets the modification time to the write, so the
//! filesystem already records what a timestamp inside the JSON would. This is
//! the same thing `attachments::sweep_with` reads to decide an orphan's age.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use copper_core::attachments::is_bare_filename;
use copper_core::store::atomic;

use crate::attachments::thumb;
use crate::diagnostics;

use super::{LinkPreview, CACHE_DIR, CACHE_MAX_BYTES, PREVIEW_TTL};

/// How many bytes of downloaded image are decoded, before downscaling.
///
/// Well under the attachment ceiling, because this one is not something the user
/// chose: an `og:image` is whatever a third party put in a `<meta>` tag, and the
/// bound on how much of it this process is willing to read should reflect that.
/// The decode itself is bounded again by `thumb`'s pixel and allocation
/// ceilings, which a file size cannot express.
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// The cache directory inside the app's config directory.
pub fn dir(config_dir: &Path) -> PathBuf {
	config_dir.join(CACHE_DIR)
}

/// The one door into the cache directory, mirroring `attachments::resolve`.
///
/// `file` reaches [`super::commands::preview_image`] from the WebView, so it is
/// as caller-controlled as an attachment's `file` is — and it is rebuilt into a
/// path by exactly one function for the same reason.
pub fn resolve(dir: &Path, file: &str) -> Option<PathBuf> {
	is_bare_filename(file).then(|| dir.join(file))
}

/// The cached preview for `key`, if there is one and it has not expired.
///
/// An unreadable or unparseable entry is a miss rather than a failure. The file
/// is derived data and the honest response to finding it damaged is to fetch it
/// again, not to report anything to anybody.
pub fn read(dir: &Path, key: &str) -> Option<LinkPreview> {
	let path = dir.join(format!("{key}.json"));
	let age = std::fs::metadata(&path)
		.and_then(|meta| meta.modified())
		.ok()
		.and_then(|modified| SystemTime::now().duration_since(modified).ok())?;
	if age > PREVIEW_TTL {
		return None;
	}
	serde_json::from_slice(&std::fs::read(&path).ok()?).ok()
}

/// Writes the metadata. Best effort: a preview that could not be cached is still
/// a preview, and the only cost of the failure is that the next read fetches it
/// again.
pub fn write(dir: &Path, key: &str, preview: &LinkPreview) {
	let Ok(text) = serde_json::to_string_pretty(preview) else {
		return;
	};
	if let Err(err) = ensure(dir).and_then(|()| {
		atomic::write_atomic(&dir.join(format!("{key}.json")), &format!("{text}\n"))
	}) {
		diagnostics::log_error(&format!(
			"[copper] previews: {key} could not be cached and will be fetched again: {err}"
		));
	}
}

/// Downscales the downloaded picture and writes it beside the metadata,
/// returning the filename the frontend asks for it by.
///
/// **The bytes are re-encoded, never stored as they arrived.** Running them
/// through `thumb::thumbnail` is what borrows the attachment path's
/// decompression-bomb ceilings — a 40 KiB PNG declaring 60,000 × 60,000 pixels
/// is refused there — and it is also what guarantees no full-size remote image
/// ever reaches the WebView, by the same argument `attachment_thumb` makes.
///
/// The type is **sniffed from the bytes**, never taken from the response's
/// `Content-Type`: a header is a claim by the same party that chose the image.
pub fn write_image(dir: &Path, key: &str, bytes: &[u8]) -> Option<String> {
	let mime = copper_core::attachments::sniff_mime(bytes);
	if !thumb::is_thumbnailable(mime) {
		return None;
	}
	let scaled = thumb::thumbnail(bytes, mime).ok()?;

	let file = format!("{key}.png");
	ensure(dir).ok()?;
	let target = dir.join(&file);
	let prepared = atomic::prepare_bytes(dir, &scaled).ok()?;
	// Replaces rather than refusing a collision, unlike an attachment blob: the
	// name is a hash of the *URL*, not of the bytes, so a second write is the same
	// page having changed its picture and the new one is the right answer.
	prepared.commit(&target).ok()?;
	Some(file)
}

fn ensure(dir: &Path) -> copper_core::store::error::Result<()> {
	std::fs::create_dir_all(dir)
		.map_err(|err| copper_core::store::error::io_err(dir, "create", &err))
}

/// Deletes expired entries, then the oldest of what is left until the directory
/// is under [`CACHE_MAX_BYTES`].
///
/// **At startup only**, like `attachments::sweep_active_space` — not because an
/// undo could need these (nothing can), but because a mid-session pass would
/// delete an entry a card on screen is about to ask for and turn a rendered
/// preview back into a fetch.
///
/// Best effort throughout: a missing directory is the ordinary case for an
/// install that has never turned the feature on, and every failure is logged and
/// skipped.
pub fn prune(dir: &Path) {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return;
	};

	// Grouped by key rather than swept file by file: the picture and the metadata
	// describe one page, and deleting the `.png` on its own would leave an entry
	// pointing at an image that is no longer there.
	let mut cached: std::collections::HashMap<String, Entry> = std::collections::HashMap::new();
	for entry in entries.flatten() {
		let name = entry.file_name();
		let Some(name) = name.to_str() else { continue };
		let Ok(metadata) = entry.metadata() else {
			continue;
		};
		if !metadata.is_file() {
			continue;
		}
		let Some((key, extension)) = name.rsplit_once('.') else {
			continue;
		};
		if !matches!(extension, "json" | "png") {
			continue;
		}

		let slot = cached.entry(key.to_string()).or_default();
		slot.bytes += metadata.len();
		slot.paths.push(entry.path());
		// The metadata file's own time is the entry's age; the picture is written
		// moments later and would only report the same thing less precisely.
		if extension == "json" {
			slot.written = metadata.modified().ok();
		}
	}

	let now = SystemTime::now();
	let mut live: Vec<(SystemTime, Entry)> = Vec::new();
	let mut total = 0u64;

	for (_, entry) in cached {
		let expired = entry.written.is_none_or(|written| {
			now.duration_since(written)
				.is_ok_and(|age| age > PREVIEW_TTL)
		});
		if expired {
			remove(entry);
			continue;
		}
		total += entry.bytes;
		// `written` is Some here — `is_none_or` above took the other branch.
		if let Some(written) = entry.written {
			live.push((written, entry));
		}
	}

	if total <= CACHE_MAX_BYTES {
		return;
	}
	// Oldest first, which is the only ordering that makes the cap a cache policy
	// rather than an arbitrary cull: what is dropped is what has been useful least
	// recently.
	live.sort_by_key(|(written, _)| *written);
	for (_, entry) in live {
		if total <= CACHE_MAX_BYTES {
			break;
		}
		total -= entry.bytes;
		remove(entry);
	}
}

/// One cached page's files and their combined size.
#[derive(Default)]
struct Entry {
	paths: Vec<PathBuf>,
	bytes: u64,
	/// When the metadata file was written, or `None` when there is no metadata
	/// file at all — an orphaned `.png` from an interrupted write, which is
	/// expired by definition.
	written: Option<SystemTime>,
}

fn remove(entry: Entry) {
	for path in entry.paths {
		if let Err(err) = std::fs::remove_file(&path) {
			diagnostics::log_error(&format!(
				"[copper] previews: could not delete the expired entry {}: {err}",
				path.display()
			));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn preview(title: &str) -> LinkPreview {
		LinkPreview {
			url: "https://example.com/".into(),
			title: Some(title.into()),
			..Default::default()
		}
	}

	/// Backdates an entry so the TTL has demonstrably elapsed, rather than making
	/// the test wait a week.
	fn age(path: &Path, by: std::time::Duration) {
		let file = std::fs::File::options().write(true).open(path).unwrap();
		file.set_modified(SystemTime::now() - by).unwrap();
	}

	#[test]
	fn a_written_preview_reads_back_unchanged() {
		let dir = tempfile::tempdir().unwrap();
		let stored = preview("A title");

		write(dir.path(), "0123456789abcdef", &stored);

		assert_eq!(read(dir.path(), "0123456789abcdef"), Some(stored));
		assert_eq!(read(dir.path(), "no-such-key"), None);
	}

	#[test]
	fn an_entry_past_its_ttl_reads_as_a_miss() {
		let dir = tempfile::tempdir().unwrap();
		write(dir.path(), "abcd", &preview("A title"));
		let path = dir.path().join("abcd.json");

		age(&path, PREVIEW_TTL - std::time::Duration::from_secs(60));
		assert!(read(dir.path(), "abcd").is_some(), "an entry inside the TTL expired");

		age(&path, PREVIEW_TTL + std::time::Duration::from_secs(60));
		assert_eq!(read(dir.path(), "abcd"), None);
	}

	/// Damage is a miss, not a failure: the file is derived data, and the right
	/// response to finding it unreadable is to fetch the page again.
	#[test]
	fn a_corrupt_entry_is_a_miss_rather_than_an_error() {
		let dir = tempfile::tempdir().unwrap();
		std::fs::create_dir_all(dir.path()).unwrap();
		std::fs::write(dir.path().join("abcd.json"), "{ not json").unwrap();

		assert_eq!(read(dir.path(), "abcd"), None);
	}

	/// The picture goes with the metadata. Deleting one and leaving the other is
	/// the failure this grouping exists to prevent — an entry pointing at an
	/// image that is no longer there.
	#[test]
	fn the_prune_takes_an_expired_entry_and_its_picture_together() {
		let dir = tempfile::tempdir().unwrap();
		write(dir.path(), "old", &preview("Old"));
		std::fs::write(dir.path().join("old.png"), b"a picture").unwrap();
		write(dir.path(), "new", &preview("New"));
		std::fs::write(dir.path().join("new.png"), b"a picture").unwrap();
		age(
			&dir.path().join("old.json"),
			PREVIEW_TTL + std::time::Duration::from_secs(60),
		);

		prune(dir.path());

		assert!(!dir.path().join("old.json").exists());
		assert!(!dir.path().join("old.png").exists(), "the picture outlived its entry");
		assert!(dir.path().join("new.json").exists());
		assert!(dir.path().join("new.png").exists());
	}

	/// A `.png` with no `.json` beside it is what an interrupted write leaves, and
	/// nothing will ever ask for it again — the metadata that named it is what the
	/// frontend reads the filename from.
	#[test]
	fn a_picture_with_no_entry_is_collected() {
		let dir = tempfile::tempdir().unwrap();
		std::fs::create_dir_all(dir.path()).unwrap();
		std::fs::write(dir.path().join("stray.png"), b"a picture").unwrap();

		prune(dir.path());

		assert!(!dir.path().join("stray.png").exists());
	}

	#[test]
	fn pruning_a_directory_that_was_never_created_is_a_no_op() {
		let dir = tempfile::tempdir().unwrap();
		let previews = dir.path().join(CACHE_DIR);
		prune(&previews);
		assert!(!previews.exists());
	}

	/// The one door. A `file` arriving from the WebView is rebuilt into a path
	/// here or refused, exactly as an attachment's is.
	#[test]
	fn resolve_refuses_every_name_that_could_leave_the_directory() {
		let dir = Path::new(r"C:\config\previews");
		for file in [
			r"..\..\Windows\System32\config\SAM",
			r"C:\x.png",
			"sub/dir.png",
			".hidden",
			"",
			"nul\0byte.png",
		] {
			assert!(resolve(dir, file).is_none(), "{file:?} resolved to a path");
		}
		assert_eq!(
			resolve(dir, "0123456789abcdef.png"),
			Some(dir.join("0123456789abcdef.png"))
		);
	}
}
