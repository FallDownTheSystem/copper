//! Turning bytes into a blob and the metadata a note will carry.
//!
//! It is on this side of the crate boundary for one reason: [`thumb::dimensions`]
//! decodes the image header, which is the `image` crate, and `copper-core` must
//! stay linkable by a command-line tool that never draws anything.
//!
//! Everything it applies that is a *rule* rather than a step — the size cap and
//! its wording, the name a digest turns into, the collision-tolerant write — is
//! [`copper_core::attachments`] and is called through it. What is genuinely here
//! is the sequence: sniff, hash, write, measure.

use std::path::Path;

use sha2::{Digest, Sha256};

use copper_core::attachments::{
	hex16, resolve, too_large, write_blob, ATTACHMENT_MAX_BYTES,
};
use copper_core::store::atomic;
use copper_core::store::error::{io_err, Result, StoreError};
use copper_core::store::ids;
use copper_core::store::model::Attachment;

use super::thumb;

/// Sniffs, size-checks, hashes, writes atomically, and returns the metadata the
/// document will carry.
///
/// All three ingestion paths converge here — paste, drop and picker — which is
/// what makes the size cap, the sniffing rule and the content addressing
/// impossible to apply inconsistently across three affordances.
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

#[cfg(test)]
mod tests {
	use super::*;

	use copper_core::attachments::assets_dir;

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

	/// The smallest valid PNG: 1×1, 8-bit greyscale.
	fn one_pixel_png() -> Vec<u8> {
		let mut buffer = std::io::Cursor::new(Vec::new());
		image::DynamicImage::new_luma8(1, 1)
			.write_to(&mut buffer, image::ImageFormat::Png)
			.unwrap();
		buffer.into_inner()
	}
}
