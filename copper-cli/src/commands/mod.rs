//! One module per top-level command, plus the few helpers more than one needs.

pub mod attachment;
pub mod copy;
pub mod note;
pub mod search;
pub mod section;
pub mod space;

use std::io::Read;

use copper_core::store::error::{Result, StoreError};
use copper_core::store::model::{Note, Space};

use crate::output::{AttachmentRow, NoteRow};

/// A body from positional arguments or from standard input.
///
/// Arguments are joined with single spaces, so `copper note add buy milk` does
/// the obvious thing without quoting. Standard input is taken **verbatim** apart
/// from one trailing newline: a here-doc or a piped file is the way multi-line
/// Markdown reaches a note, and reflowing it would corrupt every fence in it.
///
/// The single trailing newline goes because every shell adds one and no user
/// meant it. `ops::clean_body` trims the end anyway — this is here so the
/// intent is stated rather than relied on.
pub fn body(words: &[String], from_stdin: bool) -> Result<String> {
	if !from_stdin {
		return Ok(words.join(" "));
	}
	let mut text = String::new();
	std::io::stdin()
		.read_to_string(&mut text)
		.map_err(|err| StoreError::Io(format!("could not read standard input: {err}")))?;
	Ok(text.strip_suffix('\n').unwrap_or(&text).to_string())
}

/// A note's section's name, or its id when the section has gone.
///
/// The fallback cannot fire on a normalised document — `format::normalise`
/// reassigns any note whose `section` names nothing — but this runs over
/// documents the CLI merely read, and reporting a bare id is better than a panic
/// or an empty column.
fn section_name<'a>(space: &'a Space, id: &'a str) -> &'a str {
	space
		.sections
		.iter()
		.find(|section| section.id == id)
		.map_or(id, |section| section.name.as_str())
}

pub fn note_row(space: &Space, note: &Note) -> NoteRow {
	NoteRow {
		id: note.id.clone(),
		section: note.section.clone(),
		section_name: section_name(space, &note.section).to_string(),
		order: note.order,
		done: note.done,
		body: note.body.clone(),
		attachments: note
			.attachments
			.iter()
			.map(|attachment| AttachmentRow {
				id: attachment.id.clone(),
				file: attachment.file.clone(),
				name: attachment.name.clone(),
				mime: attachment.mime.clone(),
				bytes: attachment.bytes,
				width: attachment.width,
				height: attachment.height,
			})
			.collect(),
		created: note.created.clone(),
		updated: note.updated.clone(),
	}
}
