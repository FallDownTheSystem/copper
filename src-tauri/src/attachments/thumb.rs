//! Decoding, downscaling, and the one encoding step the clipboard needs.
//!
//! Two rules shape this module.
//!
//! **Thumbnails travel as bytes over IPC, never through the asset protocol.**
//! That keeps `app.security.assetProtocol` unconfigured, adds no `core:asset-
//! protocol` permission, needs no CSP change, and — the reason it was chosen —
//! avoids a capability scope list that would have to widen every time a space
//! moved somewhere new on the filesystem. The design's "Rust owns all
//! persistence" decision already means the app reads user-chosen paths through
//! its own commands; attachments stay inside that shape.
//!
//! **A full-size image is never loaded into the WebView.** [`THUMB_MAX_EDGE`]
//! is small enough that the IPC cost is irrelevant at this panel's note counts,
//! and opening the real thing goes to the OS instead.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, ImageReader};

use crate::store::error::{Result, StoreError};

/// Logical pixels on the longest edge, so ≤ 640 physical at 2× scaling. A few
/// tens of kilobytes of PNG.
pub const THUMB_MAX_EDGE: u32 = 320;

/// Whether this is a type the thumbnail path can decode at all.
///
/// Asked of the **sniffed** mime, never of an extension, so a `.png` that is
/// really an executable is not offered a decoder (AC22).
pub fn is_thumbnailable(mime: &str) -> bool {
	matches!(
		mime,
		"image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp"
	)
}

/// The image's pixel dimensions, or `(None, None)` for anything that is not a
/// decodable image.
///
/// Read from the header rather than by decoding: this runs on every ingest, and
/// a 10 MiB JPEG's dimensions should not cost a full decode. Advisory in the
/// document either way — nothing sizes an allocation from these.
pub fn dimensions(bytes: &[u8], mime: &str) -> (Option<u32>, Option<u32>) {
	if !is_thumbnailable(mime) {
		return (None, None);
	}
	match reader(bytes).and_then(|reader| reader.into_dimensions().ok()) {
		Some((width, height)) => (Some(width), Some(height)),
		None => (None, None),
	}
}

/// Decodes, downscales to fit [`THUMB_MAX_EDGE`], and re-encodes as PNG.
///
/// An image already inside the box is still re-encoded rather than passed
/// through. Handing the original bytes back would make the "the WebView never
/// receives a full-size image" rule depend on the *source* being small, which
/// is a property of whatever the user pasted rather than of this code.
pub fn thumbnail(bytes: &[u8], mime: &str) -> Result<Vec<u8>> {
	if !is_thumbnailable(mime) {
		return Err(StoreError::Invalid(format!(
			"{mime} attachments have no preview"
		)));
	}
	let decoded = decode(bytes)?;
	// Downscale only. `DynamicImage::thumbnail` resizes to *fit* the box, which
	// for anything smaller means enlarging it — a 16×16 favicon would come back
	// as a blurry 320×320 PNG many times the size of the original, for a preview
	// that shows less.
	let scaled = if decoded.width() > THUMB_MAX_EDGE || decoded.height() > THUMB_MAX_EDGE {
		decoded.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE)
	} else {
		decoded
	};
	encode_png(&scaled)
}

/// A clipboard DIB, turned into PNG bytes something can be ingested from.
///
/// A device-independent bitmap is a BMP file body with its 14-byte
/// `BITMAPFILEHEADER` removed — the clipboard drops it because the format is
/// defined as the header-and-pixels part. Putting one back is what makes the
/// payload a decodable file, and it is why `image`'s `bmp` feature is not
/// optional here.
///
/// **A DIB is never written to disk as one.** It is not a portable file format:
/// nothing outside Windows opens a headerless bitmap, and the whole point of
/// storing an attachment is that the user can double-click it later.
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>> {
	let bmp = bmp_from_dib(dib)?;
	let decoded = image::load_from_memory_with_format(&bmp, ImageFormat::Bmp)
		.map_err(|err| StoreError::Invalid(format!("the pasted image could not be read: {err}")))?;
	encode_png(&opaque_if_fully_transparent(decoded))
}

// --- internals ---------------------------------------------------------------

fn reader(bytes: &[u8]) -> Option<ImageReader<Cursor<&[u8]>>> {
	ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()
}

fn decode(bytes: &[u8]) -> Result<DynamicImage> {
	reader(bytes)
		.ok_or_else(|| StoreError::Invalid("the image could not be read".into()))?
		.decode()
		.map_err(|err| StoreError::Invalid(format!("the image could not be decoded: {err}")))
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>> {
	let mut buffer = Cursor::new(Vec::new());
	image
		.write_to(&mut buffer, ImageFormat::Png)
		.map_err(|err| StoreError::Io(format!("the image could not be encoded: {err}")))?;
	Ok(buffer.into_inner())
}

/// `BI_BITFIELDS` and `BI_ALPHABITFIELDS`, the two compressions that put channel
/// masks between a 40-byte header and the pixels.
const BI_BITFIELDS: u32 = 3;
const BI_ALPHABITFIELDS: u32 = 6;
/// `BITMAPINFOHEADER`. Later headers (V4 at 108, V5 at 124) carry their masks
/// inside themselves, so only this one needs the masks counted separately.
const BITMAPINFOHEADER_SIZE: u32 = 40;
const FILE_HEADER_SIZE: u32 = 14;

/// Prepends a `BITMAPFILEHEADER` whose `bfOffBits` actually points at the
/// pixels.
///
/// Getting that offset wrong is not a decode failure — it is a decode that
/// succeeds and renders the palette as image data — so each of the three things
/// that can sit between the header and the pixels is counted rather than
/// assumed absent: the channel masks after a 40-byte header, and the colour
/// table for any depth of 8 bits or fewer.
fn bmp_from_dib(dib: &[u8]) -> Result<Vec<u8>> {
	let malformed = || StoreError::Invalid("the pasted image is not a readable bitmap".into());

	let header_size = read_u32(dib, 0).ok_or_else(malformed)?;
	if (header_size as usize) < 12 || header_size as usize > dib.len() {
		return Err(malformed());
	}

	// A 12-byte BITMAPCOREHEADER lays its fields out differently and predates
	// every screenshot tool by decades; nothing puts one on the clipboard.
	let bits_per_pixel = read_u16(dib, 14).ok_or_else(malformed)?;
	let compression = read_u32(dib, 16).ok_or_else(malformed)?;
	let colours_used = read_u32(dib, 32).ok_or_else(malformed)?;

	let masks = if header_size == BITMAPINFOHEADER_SIZE {
		match compression {
			BI_BITFIELDS => 12,
			BI_ALPHABITFIELDS => 16,
			_ => 0,
		}
	} else {
		0
	};

	let palette = if bits_per_pixel <= 8 {
		let entries = if colours_used == 0 {
			1u32 << bits_per_pixel
		} else {
			colours_used
		};
		entries.saturating_mul(4)
	} else {
		0
	};

	let offset = FILE_HEADER_SIZE
		.checked_add(header_size)
		.and_then(|sum| sum.checked_add(masks))
		.and_then(|sum| sum.checked_add(palette))
		.ok_or_else(malformed)?;
	let size = FILE_HEADER_SIZE
		.checked_add(u32::try_from(dib.len()).map_err(|_| malformed())?)
		.ok_or_else(malformed)?;

	let mut bmp = Vec::with_capacity(dib.len() + FILE_HEADER_SIZE as usize);
	bmp.extend_from_slice(b"BM");
	bmp.extend_from_slice(&size.to_le_bytes());
	bmp.extend_from_slice(&0u16.to_le_bytes());
	bmp.extend_from_slice(&0u16.to_le_bytes());
	bmp.extend_from_slice(&offset.to_le_bytes());
	bmp.extend_from_slice(dib);
	Ok(bmp)
}

/// Makes a fully transparent decode opaque.
///
/// 32-bit `BI_RGB` bitmaps carry a fourth byte per pixel that the format says
/// nothing about, and most producers leave it zero. Read as alpha that is an
/// entirely invisible image — a pasted screenshot that renders as nothing at
/// all. The condition is deliberately "every pixel is fully transparent" rather
/// than "the source was 32-bit": an image that really is blank everywhere
/// carries no information to lose, and one with any visible pixel is left
/// exactly as decoded.
fn opaque_if_fully_transparent(image: DynamicImage) -> DynamicImage {
	if image.color().has_alpha() {
		let mut rgba = image.to_rgba8();
		if rgba.pixels().all(|pixel| pixel.0[3] == 0) {
			for pixel in rgba.pixels_mut() {
				pixel.0[3] = 255;
			}
			return DynamicImage::ImageRgba8(rgba);
		}
	}
	image
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
	Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
	Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn png(width: u32, height: u32) -> Vec<u8> {
		encode_png(&DynamicImage::new_rgb8(width, height)).unwrap()
	}

	#[test]
	fn only_decodable_image_types_are_thumbnailable() {
		for mime in ["image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp"] {
			assert!(is_thumbnailable(mime), "{mime}");
		}
		for mime in ["application/pdf", "application/octet-stream", "image/svg+xml", "text/plain"] {
			assert!(!is_thumbnailable(mime), "{mime}");
		}
	}

	#[test]
	fn dimensions_come_from_the_header_and_are_absent_for_non_images() {
		assert_eq!(dimensions(&png(1280, 720), "image/png"), (Some(1280), Some(720)));
		assert_eq!(dimensions(b"%PDF-1.4\n", "application/pdf"), (None, None));
		// A mime that claims to be an image over bytes that are not.
		assert_eq!(dimensions(b"not an image at all", "image/png"), (None, None));
	}

	#[test]
	fn a_thumbnail_fits_the_box_and_keeps_its_aspect_ratio() {
		let bytes = thumbnail(&png(1280, 640), "image/png").unwrap();
		let (width, height) = dimensions(&bytes, "image/png");
		assert_eq!((width, height), (Some(THUMB_MAX_EDGE), Some(THUMB_MAX_EDGE / 2)));
	}

	/// The rule is about what reaches the WebView, so it cannot depend on the
	/// source happening to be small — and an image already inside the box keeps
	/// its own size rather than being enlarged to fill it.
	#[test]
	fn a_small_image_is_re_encoded_at_its_own_size_rather_than_upscaled() {
		let bytes = thumbnail(&png(8, 8), "image/png").unwrap();
		assert_eq!(dimensions(&bytes, "image/png"), (Some(8), Some(8)));
		assert!(image::load_from_memory_with_format(&bytes, ImageFormat::Png).is_ok());
	}

	#[test]
	fn a_type_with_no_decoder_has_no_preview_rather_than_a_broken_one() {
		let err = thumbnail(b"%PDF-1.4\n", "application/pdf").unwrap_err();
		assert_eq!(err.kind(), "invalid");
	}

	// --- DIB ---

	/// A 24-bit `BI_RGB` DIB: 40-byte header, no masks, no palette. Rows are
	/// padded to four bytes and stored bottom-up, which is what a positive
	/// height means.
	fn dib_24bpp(width: i32, height: i32) -> Vec<u8> {
		let stride = ((width as usize * 3) + 3) & !3;
		let mut dib = Vec::new();
		dib.extend_from_slice(&40u32.to_le_bytes());
		dib.extend_from_slice(&width.to_le_bytes());
		dib.extend_from_slice(&height.to_le_bytes());
		dib.extend_from_slice(&1u16.to_le_bytes());
		dib.extend_from_slice(&24u16.to_le_bytes());
		dib.extend_from_slice(&0u32.to_le_bytes());
		dib.extend_from_slice(&((stride * height as usize) as u32).to_le_bytes());
		dib.extend_from_slice(&2835i32.to_le_bytes());
		dib.extend_from_slice(&2835i32.to_le_bytes());
		dib.extend_from_slice(&0u32.to_le_bytes());
		dib.extend_from_slice(&0u32.to_le_bytes());
		dib.resize(40 + stride * height as usize, 0x40);
		dib
	}

	#[test]
	fn a_dib_becomes_a_png_of_the_same_size() {
		let bytes = dib_to_png(&dib_24bpp(4, 3)).unwrap();
		assert_eq!(dimensions(&bytes, "image/png"), (Some(4), Some(3)));
		assert_eq!(infer::get(&bytes).map(|kind| kind.mime_type()), Some("image/png"));
	}

	/// `bfOffBits` has to skip the colour table, or the decode succeeds and
	/// renders the palette as pixels.
	#[test]
	fn the_pixel_offset_accounts_for_a_palette() {
		let mut dib = dib_24bpp(2, 2);
		// Rewrite it as 8-bit with a 256-entry palette in front of the pixels.
		dib[14..16].copy_from_slice(&8u16.to_le_bytes());
		let palette = vec![0u8; 256 * 4];
		dib.splice(40..40, palette);

		let bmp = bmp_from_dib(&dib).unwrap();
		let offset = u32::from_le_bytes(bmp[10..14].try_into().unwrap());
		assert_eq!(offset, 14 + 40 + 256 * 4);
	}

	#[test]
	fn the_pixel_offset_accounts_for_bitfield_masks() {
		let mut dib = dib_24bpp(2, 2);
		dib[14..16].copy_from_slice(&32u16.to_le_bytes());
		dib[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
		dib.splice(40..40, vec![0u8; 12]);

		let bmp = bmp_from_dib(&dib).unwrap();
		assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 14 + 40 + 12);
	}

	/// A `BITMAPV5HEADER` carries its masks inside the header, so counting them
	/// again would push the offset twelve bytes into the pixels.
	#[test]
	fn a_v5_header_does_not_have_its_masks_counted_twice() {
		let mut dib = dib_24bpp(2, 2);
		dib[0..4].copy_from_slice(&124u32.to_le_bytes());
		dib[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
		dib.splice(40..40, vec![0u8; 84]);

		let bmp = bmp_from_dib(&dib).unwrap();
		assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 14 + 124);
	}

	#[test]
	fn a_truncated_or_nonsense_dib_is_refused_rather_than_panicking() {
		for dib in [&b""[..], &b"\x28"[..], &[0xffu8; 8][..], &[0u8; 64][..]] {
			assert!(dib_to_png(dib).is_err(), "{dib:?} produced an image");
		}
	}

	/// The screenshot-renders-as-nothing case: a 32-bit `BI_RGB` bitmap whose
	/// undefined fourth byte is zero everywhere.
	#[test]
	fn a_fully_transparent_decode_is_made_opaque() {
		let transparent = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
			2,
			2,
			image::Rgba([10, 20, 30, 0]),
		));
		let fixed = opaque_if_fully_transparent(transparent).to_rgba8();
		assert!(fixed.pixels().all(|pixel| pixel.0[3] == 255));

		// One visible pixel means the alpha channel is real and is left alone.
		let mut partial = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 0]));
		partial.put_pixel(0, 0, image::Rgba([10, 20, 30, 128]));
		let kept = opaque_if_fully_transparent(DynamicImage::ImageRgba8(partial)).to_rgba8();
		assert_eq!(kept.get_pixel(1, 1).0[3], 0, "a real alpha channel was overwritten");
	}
}
