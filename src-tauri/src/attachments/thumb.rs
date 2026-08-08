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
//! **No full-size image reaches the WebView through *this* module.**
//! [`THUMB_MAX_EDGE`] is small enough that the IPC cost is irrelevant at this
//! panel's note counts, and [`thumbnail`] re-encodes even an image already inside
//! the box so the rule cannot depend on the source happening to be small.
//!
//! Task-014's in-panel viewer does hand the original bytes over, through
//! `commands::attachment_full` — a separate command precisely so this one keeps
//! the property above rather than growing a flag that retires it. That path
//! decodes nothing in Rust, so the ceilings below never enter it; its bound is
//! `ATTACHMENT_MAX_BYTES`, applied by the read itself.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, ImageReader, Limits};

use crate::store::error::{Result, StoreError};

/// Logical pixels on the longest edge, so ≤ 640 physical at 2× scaling. A few
/// tens of kilobytes of PNG.
pub const THUMB_MAX_EDGE: u32 = 320;

/// The ceiling on a decoded image's pixel count, and the reason it exists.
///
/// A file's *size* bounds nothing useful here: a 40 KiB PNG can declare 60,000
/// × 60,000 pixels and expand to fourteen gigabytes when decoded, which is the
/// decompression bomb every image decoder has to be told about. `image`'s
/// default [`Limits`] set no dimension bound at all and allow a 512 MiB
/// allocation, so the default is not a limit this app can accept.
///
/// 50 megapixels is comfortably above any screenshot — an 8K display is 33 —
/// and 200 MB of RGBA below it, which is survivable once. The frontend
/// separately bounds how many of these can be in flight at a time, because the
/// panel can hold two hundred notes carrying ten attachments each and the
/// amplification, not the single decode, is what would take the process down.
const MAX_DECODED_PIXELS: u64 = 50_000_000;

/// What one decode may allocate. Two hundred megabytes: enough for
/// [`MAX_DECODED_PIXELS`] of RGBA, and two and a half times below `image`'s own
/// default.
const MAX_DECODE_ALLOC: u64 = 200 * 1024 * 1024;

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
/// a 32 MiB JPEG's dimensions should not cost a full decode. Advisory in the
/// document either way — nothing sizes an allocation from these.
pub fn dimensions(bytes: &[u8], mime: &str) -> (Option<u32>, Option<u32>) {
	if !is_thumbnailable(mime) {
		return (None, None);
	}
	match header_reader(bytes).and_then(|reader| reader.into_dimensions().ok()) {
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
	// Checked from the *header*, before any pixel is decoded. This is the cheap
	// half of the bomb defence and the one that gives a readable message; the
	// limits handed to the decoder below are the half that holds when a header
	// lies about what follows it.
	if let (Some(width), Some(height)) = dimensions(bytes, mime) {
		let pixels = u64::from(width) * u64::from(height);
		if pixels > MAX_DECODED_PIXELS {
			return Err(StoreError::Invalid(format!(
				"that image is {width} × {height} and too large to preview"
			)));
		}
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
	encode_png(&decode(&bmp)?)
}

// --- internals ---------------------------------------------------------------

fn reader(bytes: &[u8]) -> Option<ImageReader<Cursor<&[u8]>>> {
	ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()
}

/// A reader for reading the header and nothing else.
///
/// It deliberately carries **no dimension bounds**. Reading a header allocates
/// nothing, and bounding the probe would make an oversized image fail *here* —
/// so [`thumbnail`] would never learn what the dimensions were and could only
/// report a generic decode failure instead of naming the size. `max_alloc`
/// still applies, because a container format can make even a header read ask
/// for memory.
fn header_reader(bytes: &[u8]) -> Option<ImageReader<Cursor<&[u8]>>> {
	let mut reader = reader(bytes)?;
	let mut limits = Limits::no_limits();
	limits.max_alloc = Some(MAX_DECODE_ALLOC);
	reader.limits(limits);
	Some(reader)
}

/// The decoding half, with the bounds the header probe leaves off.
///
/// `Limits::default()` sets `max_image_width` and `max_image_height` to `None`
/// and `max_alloc` to 512 MiB, so a decoder handed a hostile header will
/// happily try for half a gigabyte. These are the backstop for a header that
/// *lies* — the readable refusal comes from the pixel-count check in
/// [`thumbnail`], which runs first and gets to say what the size actually was.
fn decode(bytes: &[u8]) -> Result<DynamicImage> {
	let mut reader =
		reader(bytes).ok_or_else(|| StoreError::Invalid("the image could not be read".into()))?;
	let mut limits = Limits::no_limits();
	// A square at the pixel cap, so the per-axis bounds and the pixel bound
	// describe the same ceiling rather than two that can disagree.
	let edge = (MAX_DECODED_PIXELS as f64).sqrt() as u32;
	limits.max_image_width = Some(edge);
	limits.max_image_height = Some(edge);
	limits.max_alloc = Some(MAX_DECODE_ALLOC);
	reader.limits(limits);
	reader
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
/// A DIB whose "pixels" are an embedded JPEG or PNG stream. Legal in the
/// format, unreadable by a BMP decoder, and refused by name so the message says
/// something a person can act on.
const BI_JPEG: u32 = 4;
const BI_PNG: u32 = 5;
/// `BITMAPINFOHEADER`. Later headers (V4 at 108, V5 at 124) carry their masks
/// inside themselves, so only this one needs the masks counted separately.
const BITMAPINFOHEADER_SIZE: u32 = 40;
const FILE_HEADER_SIZE: u32 = 14;

/// Prepends a `BITMAPFILEHEADER` whose `bfOffBits` actually points at the
/// pixels.
///
/// Getting that offset wrong is not a decode failure — it is a decode that
/// succeeds and renders the palette as image data — so each of the two things
/// that can sit between the header and the pixels is counted rather than
/// assumed absent: the channel masks after a 40-byte header, and the colour
/// table, which `biClrUsed` may declare at any depth and not only at eight bits
/// or fewer.
fn bmp_from_dib(dib: &[u8]) -> Result<Vec<u8>> {
	let malformed = || StoreError::Invalid("the pasted image is not a readable bitmap".into());

	let header_size = read_u32(dib, 0).ok_or_else(malformed)?;
	// A 12-byte `BITMAPCOREHEADER` lays its fields out differently — no
	// compression, no `biClrUsed`, 16-bit dimensions — so every offset read
	// below would be reading the wrong bytes. It is refused rather than
	// mis-parsed; nothing has put one on the clipboard in thirty years.
	if (header_size as usize) < BITMAPINFOHEADER_SIZE as usize || header_size as usize > dib.len() {
		return Err(malformed());
	}

	let width = read_u32(dib, 4).ok_or_else(malformed)? as i32;
	let height = read_u32(dib, 8).ok_or_else(malformed)? as i32;
	let bits_per_pixel = read_u16(dib, 14).ok_or_else(malformed)?;
	let compression = read_u32(dib, 16).ok_or_else(malformed)?;
	let colours_used = read_u32(dib, 32).ok_or_else(malformed)?;

	// A DIB may embed a whole JPEG or PNG stream instead of pixels. The BMP
	// decoder cannot read either, and the failure it produces otherwise names
	// nothing a person can act on.
	if matches!(compression, BI_JPEG | BI_PNG) {
		return Err(StoreError::Invalid(
			"that clipboard image is in a form Copper cannot read. Save it to a file and attach it"
				.into(),
		));
	}

	let masks = if header_size == BITMAPINFOHEADER_SIZE {
		match compression {
			BI_BITFIELDS => 12,
			BI_ALPHABITFIELDS => 16,
			_ => 0,
		}
	} else {
		0
	};

	// `biClrUsed` is honoured **at every depth**, not only at 8 bits or fewer:
	// the field is documented as the number of colour-table entries actually
	// used, and a 16- or 32-bit bitmap is allowed to carry an optimisation
	// palette. Ignoring it there put the offset short by the whole table and
	// decoded the palette as pixels. The `2^bpp` fallback is the indexed-only
	// default, because that is the only depth for which an absent count means a
	// full table rather than no table.
	let entries = if colours_used != 0 {
		colours_used
	} else if bits_per_pixel <= 8 {
		1u32 << bits_per_pixel
	} else {
		0
	};
	let palette = entries.checked_mul(4).ok_or_else(malformed)?;

	let offset = FILE_HEADER_SIZE
		.checked_add(header_size)
		.and_then(|sum| sum.checked_add(masks))
		.and_then(|sum| sum.checked_add(palette))
		.ok_or_else(malformed)?;

	// The offset has to land inside the payload, and what follows it has to be
	// big enough to be the pixels the header describes. Without this a header
	// claiming a huge palette produces a valid-looking file whose `bfOffBits`
	// points past the end, and the decoder's own error says only that the file
	// is truncated.
	let pixel_start = (offset - FILE_HEADER_SIZE) as usize;
	if pixel_start > dib.len() {
		return Err(malformed());
	}
	let stride = (u64::from(width.unsigned_abs()) * u64::from(bits_per_pixel))
		.div_ceil(8)
		.next_multiple_of(4);
	let needed = stride
		.checked_mul(u64::from(height.unsigned_abs()))
		.ok_or_else(malformed)?;
	if ((dib.len() - pixel_start) as u64) < needed {
		return Err(malformed());
	}

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

	/// A 32-bit `BI_RGB` bitmap keeps whatever `image` decodes it to, and what
	/// that is turns out to be `Rgb8` — no alpha channel at all.
	///
	/// This is the test that retired a repair. An earlier version forced a
	/// fully-transparent decode opaque, on the theory that the undefined fourth
	/// byte of a 32-bit `BI_RGB` pixel would be read as alpha and render a pasted
	/// screenshot invisible. It cannot: `image` drops that byte, so the repair
	/// could never fire for the case it documented — and the only case it *did*
	/// fire for was a V4/V5 bitmap with a real, deliberately transparent alpha
	/// channel, which it then destroyed.
	#[test]
	fn a_32_bit_bi_rgb_bitmap_decodes_without_an_alpha_channel() {
		for masked in [false, true] {
			let mut dib = dib_24bpp(2, 2);
			dib[14..16].copy_from_slice(&32u16.to_le_bytes());
			if masked {
				dib[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
				// Real red/green/blue masks and no alpha mask. All-zero masks are
				// refused outright by the decoder, so they would prove nothing.
				let mut masks = Vec::new();
				for mask in [0x00ff_0000u32, 0x0000_ff00, 0x0000_00ff] {
					masks.extend_from_slice(&mask.to_le_bytes());
				}
				dib.splice(40..40, masks);
			}
			// Replace the 24-bit rows with 32-bit ones: 2×2 at four bytes each.
			dib.truncate(dib.len() - 16);
			dib.extend_from_slice(&[0u8; 16]);

			let bmp = bmp_from_dib(&dib).unwrap();
			let decoded = image::load_from_memory_with_format(&bmp, ImageFormat::Bmp).unwrap();
			assert!(
				!decoded.color().has_alpha(),
				"a 32-bit bitmap (masked: {masked}) decoded with alpha; the retired repair may be \
				 needed again"
			);
		}
	}

	/// `biClrUsed` is honoured at every depth. A 32-bit bitmap carrying an
	/// optimisation palette had its table counted as pixels before this.
	#[test]
	fn the_pixel_offset_accounts_for_a_palette_at_any_depth() {
		let mut dib = dib_24bpp(2, 2);
		dib[14..16].copy_from_slice(&32u16.to_le_bytes());
		dib[32..36].copy_from_slice(&16u32.to_le_bytes());
		dib.splice(40..40, vec![0u8; 16 * 4]);
		dib.extend_from_slice(&[0u8; 32]);

		let bmp = bmp_from_dib(&dib).unwrap();
		assert_eq!(
			u32::from_le_bytes(bmp[10..14].try_into().unwrap()),
			14 + 40 + 16 * 4
		);
	}

	#[test]
	fn a_core_header_is_refused_rather_than_read_at_the_wrong_offsets() {
		let mut dib = dib_24bpp(2, 2);
		dib[0..4].copy_from_slice(&12u32.to_le_bytes());
		assert!(bmp_from_dib(&dib).is_err());
	}

	#[test]
	fn an_embedded_jpeg_or_png_stream_is_refused_with_a_readable_reason() {
		for compression in [BI_JPEG, BI_PNG] {
			let mut dib = dib_24bpp(2, 2);
			dib[16..20].copy_from_slice(&compression.to_le_bytes());
			let err = bmp_from_dib(&dib).unwrap_err();
			assert_eq!(err.kind(), "invalid");
			assert!(err.message().contains("Save it to a file"), "{}", err.message());
		}
	}

	/// A header claiming a palette larger than the payload used to produce a
	/// file whose `bfOffBits` pointed past the end.
	#[test]
	fn an_offset_or_pixel_run_that_does_not_fit_the_payload_is_refused() {
		let mut dib = dib_24bpp(2, 2);
		dib[32..36].copy_from_slice(&100_000u32.to_le_bytes());
		assert!(bmp_from_dib(&dib).is_err(), "an offset past the end was accepted");

		// Dimensions that describe far more pixels than the payload holds.
		let mut short = dib_24bpp(2, 2);
		short[4..8].copy_from_slice(&4000u32.to_le_bytes());
		short[8..12].copy_from_slice(&4000u32.to_le_bytes());
		assert!(bmp_from_dib(&short).is_err(), "a truncated pixel run was accepted");
	}

	/// A decompression bomb is a small file that declares an enormous image, so
	/// the *size* of the input bounds nothing. Refused from the header, before
	/// anything is allocated, and the refusal names the size.
	#[test]
	fn an_image_over_the_pixel_cap_is_refused_before_it_is_decoded() {
		// A 54-byte BMP claiming 60,000 × 60,000 — fourteen gigabytes of RGBA if
		// it were ever decoded. BMP rather than PNG because it carries no CRC, so
		// the header is a header rather than a checksum puzzle.
		let mut dib = dib_24bpp(1, 1);
		dib[4..8].copy_from_slice(&60_000u32.to_le_bytes());
		dib[8..12].copy_from_slice(&60_000u32.to_le_bytes());
		let mut bmp = Vec::new();
		bmp.extend_from_slice(b"BM");
		bmp.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
		bmp.extend_from_slice(&0u32.to_le_bytes());
		bmp.extend_from_slice(&54u32.to_le_bytes());
		bmp.extend_from_slice(&dib);

		assert_eq!(dimensions(&bmp, "image/bmp"), (Some(60_000), Some(60_000)));

		let err = thumbnail(&bmp, "image/bmp").unwrap_err();
		assert_eq!(err.kind(), "invalid");
		assert!(
			err.message().contains("60000"),
			"the refusal should name the size: {}",
			err.message()
		);
	}
}
