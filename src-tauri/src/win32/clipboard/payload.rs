//! Owned clipboard payloads, prepared before the system clipboard is opened.

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use super::{ClipboardError, Result, ATTACHMENT_READ_LIMIT};

const DROPFILES_SIZE: u32 = 20;
const BITMAPV5HEADER_SIZE: u32 = 124;

pub(in crate::win32) fn files(paths: &[PathBuf]) -> Result<Vec<u8>> {
	if paths.is_empty() {
		return Err(ClipboardError::Refused("no attachments selected".into()));
	}

	let mut bytes = Vec::new();
	for value in [DROPFILES_SIZE, 0, 0, 0, 1] {
		bytes.extend_from_slice(&value.to_le_bytes());
	}
	for path in paths {
		let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
		if !path.is_absolute() || wide.contains(&0) {
			return Err(ClipboardError::Refused(
				"an attachment has an invalid file path".into(),
			));
		}
		if wide.len() > (ATTACHMENT_READ_LIMIT.saturating_sub(bytes.len()) / 2).saturating_sub(2) {
			return Err(ClipboardError::Refused(
				"too many attachment paths to copy".into(),
			));
		}
		for unit in wide.into_iter().chain(std::iter::once(0)) {
			bytes.extend_from_slice(&unit.to_le_bytes());
		}
	}
	bytes.extend_from_slice(&0u16.to_le_bytes());
	Ok(bytes)
}

pub(super) fn image(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
	if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
		return Err(ClipboardError::Refused(
			"the image has invalid pixel dimensions".into(),
		));
	}
	let size = u64::from(width) * u64::from(height) * 4;
	if size != rgba.len() as u64 {
		return Err(ClipboardError::Refused(
			"the image has invalid pixel dimensions".into(),
		));
	}
	if size + u64::from(BITMAPV5HEADER_SIZE) > ATTACHMENT_READ_LIMIT as u64 {
		return Err(ClipboardError::Refused(
			"that image is too large to copy as pixels; copy it as a file instead".into(),
		));
	}

	let mut bytes = Vec::with_capacity(BITMAPV5HEADER_SIZE as usize + rgba.len());
	bytes.extend_from_slice(&BITMAPV5HEADER_SIZE.to_le_bytes());
	bytes.extend_from_slice(&(width as i32).to_le_bytes());
	// Top-down pixels preserve the decoder's row order without a second image buffer.
	bytes.extend_from_slice(&(-(height as i32)).to_le_bytes());
	bytes.extend_from_slice(&1u16.to_le_bytes());
	bytes.extend_from_slice(&32u16.to_le_bytes());
	bytes.extend_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS carries the alpha mask.
	bytes.extend_from_slice(&(size as u32).to_le_bytes());
	bytes.extend_from_slice(&[0; 16]);
	for mask in [0x00ff_0000u32, 0x0000_ff00, 0x0000_00ff, 0xff00_0000] {
		bytes.extend_from_slice(&mask.to_le_bytes());
	}
	bytes.extend_from_slice(&0x7352_4742u32.to_le_bytes()); // LCS_sRGB
	bytes.extend_from_slice(&[0; 48]);
	bytes.extend_from_slice(&4u32.to_le_bytes()); // LCS_GM_IMAGES
	bytes.extend_from_slice(&[0; 12]);
	for pixel in rgba.chunks_exact(4) {
		bytes.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
	}
	Ok(bytes)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn file_list_round_trips_spaces_unicode_and_multiple_files() {
		let paths = vec![
			PathBuf::from(r"C:\Attachment copies\screen shot.png"),
			PathBuf::from(r"C:\Attachment copies\muistiinpanot ä 🦀.txt"),
		];
		let bytes = files(&paths).unwrap();
		assert_eq!(super::super::parse_hdrop(&bytes), paths);
		assert!(bytes.ends_with(&[0, 0, 0, 0]));
	}

	#[test]
	fn invalid_file_lists_are_refused_before_any_write() {
		assert!(files(&[]).is_err());
		assert!(files(&[PathBuf::from("relative.txt")]).is_err());
		assert!(files(&[PathBuf::from("C:\\bad\0name.txt")]).is_err());
	}

	#[test]
	fn bitmap_preserves_dimensions_row_order_colors_and_alpha() {
		let rgba = [255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 4, 5, 6, 255];
		let dib = image(2, 2, &rgba).unwrap();
		assert_eq!(dib.len(), 124 + rgba.len());
		assert_eq!(&dib[8..12], &(-2i32).to_le_bytes());
		let png = crate::attachments::thumb::dib_to_png(&dib).unwrap();
		let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
		assert_eq!(decoded.dimensions(), (2, 2));
		assert_eq!(decoded.as_raw(), &rgba);
	}

	#[test]
	fn invalid_pixels_are_refused_before_any_write() {
		assert!(image(0, 1, &[]).is_err());
		assert!(image(u32::MAX, u32::MAX, &[]).is_err());
		assert!(image(2, 2, &[0; 4]).is_err());
	}
}
