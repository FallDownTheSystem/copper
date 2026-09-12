//! Attachment copies resolve a document snapshot before they touch the clipboard.
//! Every outward reference — a native file list, a pasted path, the path under a
//! note's text — is the stored file itself, which already carries the user's
//! name. There is no export step: one attachment is one file on disk, and a
//! paste target that edits it edits the attachment.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use copper_core::attachments::{read_blob, resolve_existing, sniff_mime};
use copper_core::store::error::{io_err, Result, StoreError};
use copper_core::store::model::{Attachment, Space};
use copper_core::store::{strip_verbatim_str, SharedStore};

use crate::markdown::{selected_notes, NoteSelection, RenderedNotes};
use crate::store::source::DocumentSource;
use crate::win32::clipboard;

use super::thumb;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Hash)]
pub struct AttachmentTarget {
	pub note: String,
	pub attachment: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CopyFormat {
	Auto,
	Image,
	Files,
	Paths,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CopiedFormat {
	Image,
	Files,
	Paths,
}

#[derive(Debug, Serialize)]
pub struct CopiedAttachments {
	pub count: usize,
	pub format: CopiedFormat,
}

enum PreparedCopy {
	Image(image::RgbaImage),
	Files(Vec<PathBuf>),
	Paths(String, usize),
}

impl PreparedCopy {
	fn write(self, expected: u32) -> Result<CopiedAttachments> {
		let (count, format, written) = match self {
			Self::Image(image) => (
				1,
				CopiedFormat::Image,
				clipboard::write_image_private(
					image.width(),
					image.height(),
					image.as_raw(),
					expected,
				),
			),
			Self::Files(paths) => (
				paths.len(),
				CopiedFormat::Files,
				clipboard::write_files_private(&paths, expected),
			),
			Self::Paths(text, count) => (
				count,
				CopiedFormat::Paths,
				clipboard::write_text_private_at(&text, expected),
			),
		};
		written.map_err(copy_failure)?;
		Ok(CopiedAttachments { count, format })
	}
}

fn copy_failure(error: clipboard::ClipboardError) -> StoreError {
	StoreError::Io(format!("could not copy to the clipboard: {error}"))
}

fn targets<'a>(space: &'a Space, requested: &[AttachmentTarget]) -> Result<Vec<&'a Attachment>> {
	if requested.is_empty() {
		return Err(StoreError::Invalid("no attachments selected".into()));
	}
	let wanted: HashSet<_> = requested.iter().collect();
	for target in &wanted {
		let note = space
			.note(&target.note)
			.ok_or_else(|| StoreError::NotFound("a selected note no longer exists".into()))?;
		if !note
			.attachments
			.iter()
			.any(|attachment| attachment.id == target.attachment)
		{
			return Err(StoreError::NotFound(
				"a selected attachment no longer exists".into(),
			));
		}
	}
	// Match note-copy order, not the order Ctrl+click happened to add selections.
	let notes = selected_notes(space, &NoteSelection::Document)?;
	let wanted = &wanted;
	Ok(notes
		.into_iter()
		.flat_map(|note| {
			note.attachments.iter().filter(move |attachment| {
				wanted.contains(&AttachmentTarget {
					note: note.id.clone(),
					attachment: attachment.id.clone(),
				})
			})
		})
		.collect())
}

fn quoted_path(path: &Path) -> Result<String> {
	let absolute = std::path::absolute(path).map_err(|err| io_err(path, "resolve", &err))?;
	let text = absolute.to_str().ok_or_else(|| {
		StoreError::Invalid("the attachment path cannot be represented as text".into())
	})?;
	if text.contains(['\r', '\n', '"']) {
		return Err(StoreError::Invalid(
			"the attachment path cannot be copied as a quoted path".into(),
		));
	}
	Ok(format!("\"{}\"", strip_verbatim_str(text)))
}

fn attachment_path(space: &Path, attachment: &Attachment) -> Result<PathBuf> {
	resolve_existing(space, &attachment.file).map_err(|error| {
		StoreError::Invalid(format!(
			"{} could not be copied: {}",
			attachment.name,
			error.message()
		))
	})
}

/// The stored files behind `attachments`, all of them or none.
///
/// Resolved before anything is written so that one missing blob refuses the
/// whole gesture: a file list with a hole in it would paste the wrong count
/// without saying so.
fn attachment_paths(space: &Path, attachments: &[&Attachment]) -> Result<Vec<PathBuf>> {
	attachments
		.iter()
		.map(|attachment| attachment_path(space, attachment))
		.collect()
}

pub(super) fn prepare_files(
	space_path: &Path,
	space: &Space,
	requested: &[AttachmentTarget],
) -> Result<Vec<PathBuf>> {
	attachment_paths(space_path, &targets(space, requested)?)
}

fn prepare_attachments(
	space_path: &Path,
	space: &Space,
	requested: &[AttachmentTarget],
	format: CopyFormat,
) -> Result<PreparedCopy> {
	let attachments = targets(space, requested)?;
	if format == CopyFormat::Image && attachments.len() != 1 {
		return Err(StoreError::Invalid(
			"select one image to copy as pixels".into(),
		));
	}
	if attachments.len() == 1 && matches!(format, CopyFormat::Auto | CopyFormat::Image) {
		attachment_path(space_path, attachments[0])?;
		let bytes = read_blob(space_path, &attachments[0].file)?;
		if format == CopyFormat::Image || thumb::is_thumbnailable(sniff_mime(&bytes)) {
			return Ok(PreparedCopy::Image(thumb::clipboard_pixels(&bytes)?));
		}
	}

	let paths = attachment_paths(space_path, &attachments)?;
	if format == CopyFormat::Paths {
		let text = paths
			.iter()
			.map(|path| quoted_path(path))
			.collect::<Result<Vec<_>>>()?
			.join("\n");
		Ok(PreparedCopy::Paths(text, attachments.len()))
	} else {
		Ok(PreparedCopy::Files(paths))
	}
}

fn prepare_notes(
	space_path: &Path,
	space: &Space,
	selection: &NoteSelection,
) -> Result<RenderedNotes> {
	let notes = selected_notes(space, selection)?;
	let attachments: Vec<_> = notes
		.iter()
		.flat_map(|note| note.attachments.iter())
		.collect();
	let paths = attachment_paths(space_path, &attachments)?;
	let mut paths = paths.iter();
	let mut bodies = Vec::with_capacity(notes.len());
	let mut ids = Vec::with_capacity(notes.len());
	for note in notes {
		let mut body = note.body.clone();
		if !note.attachments.is_empty() {
			if !body.is_empty() {
				body.push_str("\n\n");
			}
			body.push_str("Attachments (local files):");
			for attachment in &note.attachments {
				let path = paths
					.next()
					.ok_or_else(|| StoreError::Invalid("an attachment path is missing".into()))?;
				let path = quoted_path(path)?;
				let name: String = attachment
					.name
					.chars()
					.map(|ch| if ch.is_control() { ' ' } else { ch })
					.collect();
				body.push_str(&format!("\n- {name}\n  {path}"));
			}
		}
		bodies.push(body);
		ids.push(note.id.clone());
	}
	Ok(RenderedNotes {
		text: bodies.join("\n\n"),
		count: ids.len(),
		ids,
	})
}

#[tauri::command]
pub async fn clipboard_copy_attachments(
	targets: Vec<AttachmentTarget>,
	format: CopyFormat,
	source: DocumentSource,
	state: State<'_, SharedStore>,
) -> Result<CopiedAttachments> {
	let expected = clipboard::sequence_number();
	let (path, space) = source.snapshot(&state)?;
	tauri::async_runtime::spawn_blocking(move || {
		prepare_attachments(&path, &space, &targets, format)?.write(expected)
	})
	.await
	.map_err(|err| StoreError::Io(format!("the attachments could not be copied: {err}")))?
}

#[tauri::command]
pub async fn clipboard_copy_notes(
	selection: NoteSelection,
	source: DocumentSource,
	state: State<'_, SharedStore>,
) -> Result<RenderedNotes> {
	let expected = clipboard::sequence_number();
	let (path, space) = source.snapshot(&state)?;
	tauri::async_runtime::spawn_blocking(move || {
		let rendered = prepare_notes(&path, &space, &selection)?;
		if rendered.count > 0 {
			clipboard::write_text_private_at(&rendered.text, expected).map_err(copy_failure)?;
		}
		Ok(rendered)
	})
	.await
	.map_err(|err| StoreError::Io(format!("the notes could not be copied: {err}")))?
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::attachments::ingest;
	use copper_core::attachments::assets_dir;
	use copper_core::store::model::{Note, Section};

	fn fixture() -> (tempfile::TempDir, PathBuf, Space) {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("test space.copper");
		let attachment = ingest(&path, b"first attachment", "original name.txt").unwrap();
		let space = Space {
			id: "space".into(),
			name: "Test".into(),
			active_section: "section".into(),
			sections: vec![Section {
				id: "section".into(),
				name: "Notes".into(),
				order: 0,
			}],
			notes: vec![Note {
				id: "note".into(),
				section: "section".into(),
				order: 0,
				done: false,
				body: "Keep **this** text.".into(),
				attachments: vec![attachment],
				created: String::new(),
				updated: String::new(),
			}],
		};
		(dir, path, space)
	}

	fn selection(space: &Space) -> Vec<AttachmentTarget> {
		space
			.notes
			.iter()
			.flat_map(|note| {
				note.attachments
					.iter()
					.map(move |attachment| AttachmentTarget {
						note: note.id.clone(),
						attachment: attachment.id.clone(),
					})
			})
			.collect()
	}

	#[test]
	fn note_copy_preserves_body_and_lists_the_stored_files_by_name_and_path() {
		let (_dir, path, space) = fixture();
		let copied = prepare_notes(&path, &space, &NoteSelection::Document).unwrap();
		let listed = PathBuf::from(copied.text.lines().last().unwrap().trim().trim_matches('"'));
		assert_eq!(listed, assets_dir(&path).join("original name.txt"));
		assert_eq!(std::fs::read(&listed).unwrap(), b"first attachment");
		assert_eq!(
			copied.text,
			format!(
				"Keep **this** text.\n\nAttachments (local files):\n- original name.txt\n  {}",
				quoted_path(&listed).unwrap()
			)
		);
		assert_eq!(copied.ids, ["note"]);
		assert_eq!(copied.count, 1);
		let portable = crate::markdown::render(
			&space,
			&NoteSelection::Document,
			crate::markdown::MarkdownFormat::Bodies,
		)
		.unwrap();
		assert_eq!(portable.text, space.notes[0].body);
	}

	#[test]
	fn notes_without_attachments_list_no_files() {
		let (_dir, path, mut space) = fixture();
		space.notes[0].attachments.clear();
		let copied = prepare_notes(&path, &space, &NoteSelection::Document).unwrap();
		assert_eq!(copied.text, space.notes[0].body);
	}

	#[test]
	fn a_queued_copy_cannot_resolve_against_another_space() {
		use copper_core::store::events::NullSink;
		use copper_core::store::lock;
		use std::sync::{Arc, Mutex};
		let dir = tempfile::tempdir().unwrap();
		let store =
			copper_core::store::bootstrap_store(&dir.path().join("app"), Arc::new(NullSink))
				.unwrap();
		let path = store.require_active_path().unwrap();
		let source = DocumentSource {
			path: path.to_str().unwrap().into(),
			id: store.active_space().unwrap().id,
		};
		let shared = Arc::new(Mutex::new(store));
		assert!(source.snapshot(&shared).is_ok());
		let other = DocumentSource {
			path: dir.path().join("other.copper").to_str().unwrap().into(),
			..source.clone()
		};
		assert!(other.snapshot(&shared).is_err());
		lock(&shared).close_space();
		assert!(source.snapshot(&shared).is_err());
	}

	/// The file list is the stored files themselves: the user's names, and the
	/// same bytes a paste target would get from any other folder.
	#[test]
	fn file_copy_hands_out_the_stored_files_under_their_own_names() {
		let (_dir, path, mut space) = fixture();
		space.notes[0]
			.attachments
			.push(ingest(&path, b"second attachment", "original name.txt").unwrap());
		let requested = selection(&space);
		let PreparedCopy::Files(files) =
			prepare_attachments(&path, &space, &requested, CopyFormat::Auto).unwrap()
		else {
			panic!()
		};
		assert_eq!(files.len(), 2);
		assert_eq!(files[0], assets_dir(&path).join("original name.txt"));
		assert_eq!(files[1], assets_dir(&path).join("original name (2).txt"));
		assert_eq!(std::fs::read(&files[0]).unwrap(), b"first attachment");
		assert_eq!(std::fs::read(&files[1]).unwrap(), b"second attachment");
	}

	#[test]
	fn drag_preparation_resolves_all_targets_in_document_order() {
		let (_dir, path, mut space) = fixture();
		space.notes[0]
			.attachments
			.push(ingest(&path, b"second attachment", "original name.txt").unwrap());
		let mut requested = selection(&space);
		requested.reverse();
		requested.push(requested[0].clone());
		let files = prepare_files(&path, &space, &requested).unwrap();
		assert_eq!(files.len(), 2);
		assert!(files.iter().all(|file| file.starts_with(assets_dir(&path))));
		assert_eq!(files[0].file_name().unwrap(), "original name.txt");
		assert_eq!(files[1].file_name().unwrap(), "original name (2).txt");
	}

	/// The pasted path points at the stored file, and pasting it again points
	/// at the same file — there is exactly one.
	#[test]
	fn copied_paths_name_the_stored_file_and_stay_stable_across_copies() {
		let (_dir, path, space) = fixture();
		let PreparedCopy::Paths(text, count) =
			prepare_attachments(&path, &space, &selection(&space), CopyFormat::Paths).unwrap()
		else {
			panic!()
		};
		assert_eq!(count, 1);
		let pasted = PathBuf::from(text.trim_matches('"'));
		assert_eq!(pasted, assets_dir(&path).join("original name.txt"));
		let PreparedCopy::Paths(later, _) =
			prepare_attachments(&path, &space, &selection(&space), CopyFormat::Paths).unwrap()
		else {
			panic!()
		};
		assert_eq!(later, text);
	}

	#[test]
	fn missing_and_untrusted_attachments_refuse_the_whole_copy() {
		let (_dir, path, mut space) = fixture();
		let mut missing = space.notes[0].attachments[0].clone();
		missing.id = "missing".into();
		missing.file = "absent.txt".into();
		space.notes[0].attachments.push(missing);
		for format in [CopyFormat::Auto, CopyFormat::Files, CopyFormat::Paths] {
			assert!(prepare_attachments(&path, &space, &selection(&space), format).is_err());
		}
		assert!(prepare_notes(&path, &space, &NoteSelection::Document).is_err());
		assert!(prepare_files(&path, &space, &selection(&space)).is_err());
		space.notes[0].attachments[1].file = "../outside.txt".into();
		assert!(prepare_notes(&path, &space, &NoteSelection::Document).is_err());
	}

	#[test]
	fn stale_and_duplicate_targets_cannot_misreport_the_copy() {
		let (_dir, path, space) = fixture();
		let mut requested = selection(&space);
		requested.push(requested[0].clone());
		assert_eq!(targets(&space, &requested).unwrap().len(), 1);
		requested[0].attachment = "gone".into();
		assert!(prepare_attachments(&path, &space, &requested, CopyFormat::Files).is_err());
		assert!(targets(&space, &[]).is_err());
	}

	#[test]
	fn image_copy_uses_original_pixels_and_sniffs_content_not_metadata() {
		let (_dir, path, mut space) = fixture();
		let pixels = image::RgbaImage::from_pixel(640, 480, image::Rgba([12, 34, 56, 128]));
		let mut png = std::io::Cursor::new(Vec::new());
		pixels.write_to(&mut png, image::ImageFormat::Png).unwrap();
		let mut attachment = ingest(&path, png.get_ref(), "screen.png").unwrap();
		attachment.mime = "text/plain".into();
		space.notes[0].attachments = vec![attachment];
		let PreparedCopy::Image(copied) =
			prepare_attachments(&path, &space, &selection(&space), CopyFormat::Auto).unwrap()
		else {
			panic!()
		};
		assert_eq!(copied.dimensions(), (640, 480));
		assert_eq!(copied.as_raw(), pixels.as_raw());
		space.notes[0].attachments = vec![ingest(&path, b"not an image", "pretend.png").unwrap()];
		space.notes[0].attachments[0].mime = "image/png".into();
		assert!(
			prepare_attachments(&path, &space, &selection(&space), CopyFormat::Image).is_err()
		);
	}
}
