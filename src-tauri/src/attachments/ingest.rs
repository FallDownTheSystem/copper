//! Turning bytes into a blob and the metadata a note will carry.
//!
//! It is on this side of the crate boundary for one reason: [`thumb::dimensions`]
//! decodes the image header, which is the `image` crate, and `copper-core` must
//! stay linkable by a command-line tool that never draws anything.
//!
//! Everything it applies that is a *rule* rather than a step — the size cap and
//! its wording, the repair of the user's name, the collision suffix, the atomic
//! write — is [`copper_core::attachments`] and is called through it. What is
//! genuinely here is the sequence: sniff, hash, write, measure.

use std::path::Path;

use sha2::{Digest, Sha256};

use copper_core::attachments::{hex16, store_blob, too_large, ATTACHMENT_MAX_BYTES};
use copper_core::store::error::{Result, StoreError};
use copper_core::store::ids;
use copper_core::store::model::Attachment;

use super::thumb;

/// Sniffs, size-checks, writes atomically under the user's name, and returns
/// the metadata the document will carry.
///
/// All three ingestion paths converge here — paste, drop and picker — which is
/// what makes the size cap, the sniffing rule and the naming rule impossible to
/// apply inconsistently across three affordances.
///
/// **The stored name is the user's, extension included.** The type is still
/// sniffed from the bytes for `mime`, because that is what decides whether the
/// file is thumbnailed or launched — but the name is not rewritten to match it.
/// A `.png` that is really an executable keeps its name and is neither decoded
/// nor launched, because every reader asks the bytes, not the name. The hash is
/// only the fallback for a name that repairs to nothing.
///
/// **The bytes are written before the document is.** A failure after this
/// returns leaves an orphan blob, which `attachments::sweep` collects;
/// the reverse order would leave a document referencing a file that does not
/// exist, which is the strictly worse failure because no later pass can repair it.
pub fn ingest(space_path: &Path, bytes: &[u8], original_name: &str) -> Result<Attachment> {
	let len = bytes.len() as u64;
	if len > ATTACHMENT_MAX_BYTES {
		return Err(too_large(original_name, len, ATTACHMENT_MAX_BYTES));
	}
	if bytes.is_empty() {
		return Err(StoreError::Invalid(format!("{original_name} is empty")));
	}

	// Sniffed from the bytes, never taken from the extension: a `.png` that is
	// really an executable must not be rendered as an image.
	let sniffed = infer::get(bytes);
	let mime = sniffed.map_or("application/octet-stream", |kind| kind.mime_type());

	let mut fallback = hex16(&Sha256::digest(bytes));
	if let Some(extension) = sniffed.map(|kind| kind.extension()).filter(|ext| !ext.is_empty()) {
		fallback.push('.');
		fallback.push_str(extension);
	}
	let file = store_blob(space_path, original_name, &fallback, bytes)?;

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

#[cfg(test)]
mod tests {
	use super::*;

	use copper_core::attachments::assets_dir;

	#[test]
	fn the_stored_name_is_the_users_and_the_mime_is_sniffed() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		// A one-pixel PNG. The extension on the *original* name disagrees on
		// purpose (AC22): the sniffed type decides the mime, the name stays.
		let png = one_pixel_png();

		let meta = ingest(&space, &png, "screenshot.jpg").unwrap();

		assert_eq!(meta.mime, "image/png");
		assert_eq!(meta.file, "screenshot.jpg");
		assert_eq!(meta.name, "screenshot.jpg");
		assert_eq!(meta.bytes, png.len() as u64);
		assert!(meta.id.starts_with("att_"));
		assert!(assets_dir(&space).join(&meta.file).is_file());
	}

	/// Two attachments called the same thing are two files, told apart by
	/// Explorer's suffix, and each entry names its own.
	#[test]
	fn a_second_file_with_the_same_name_takes_the_next_free_suffix() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let png = one_pixel_png();

		let first = ingest(&space, &png, "a.png").unwrap();
		let second = ingest(&space, &png, "a.png").unwrap();

		assert_eq!(first.file, "a.png");
		assert_eq!(second.file, "a (2).png");
		assert_ne!(first.id, second.id, "each attachment entry is its own");
		let written: Vec<_> = std::fs::read_dir(assets_dir(&space)).unwrap().collect();
		assert_eq!(written.len(), 2);
	}

	#[test]
	fn an_unrecognised_type_keeps_its_name_and_gets_an_octet_stream_mime() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");

		let meta = ingest(&space, b"# notes\n", "capture.md").unwrap();

		assert_eq!(meta.mime, "application/octet-stream");
		assert_eq!(meta.file, "capture.md", "the extension the user gave is the one stored");
		assert_eq!(meta.width, None);
	}

	/// A name the directory cannot hold falls back to the content hash, with the
	/// sniffed extension so the fallback still opens in the right application.
	#[test]
	fn an_unusable_name_falls_back_to_the_hash_and_sniffed_extension() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");

		let meta = ingest(&space, &one_pixel_png(), "CON").unwrap();

		assert_eq!(meta.file.len(), 16 + 4, "{}", meta.file);
		assert!(meta.file.ends_with(".png"), "{}", meta.file);
		assert_eq!(meta.name, "CON", "the original name is kept as metadata");
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

	/// The smallest valid PNG: 1×1, 8-bit greyscale.
	fn one_pixel_png() -> Vec<u8> {
		let mut buffer = std::io::Cursor::new(Vec::new());
		image::DynamicImage::new_luma8(1, 1)
			.write_to(&mut buffer, image::ImageFormat::Png)
			.unwrap();
		buffer.into_inner()
	}
}
