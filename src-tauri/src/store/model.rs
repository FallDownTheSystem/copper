//! The document model, and nothing else.
//!
//! Field declaration order **is** the on-disk key order — serde emits struct
//! fields in declaration order — so reordering a field here changes every
//! `.copper` file the next time it is written. Keep it matching the design's
//! Data model block exactly.
//!
//! Two modelling choices are deliberate and are the same choice twice: a field
//! the store can repair locally must never be able to make the surrounding
//! document unloadable.
//!
//! - `created` / `updated` are `String`, never a parsed timestamp (spec 1.2). A
//!   hand-edited or malformed timestamp is preserved verbatim rather than
//!   rejecting the file.
//! - `order` is `i64`, not `usize`. `normalise` overwrites it on every load and
//!   every write, so its incoming value is advisory; modelling it as unsigned
//!   would turn a hand-typed `-1` into a parse failure of the whole document.
//!
//! `body` is opaque Markdown. Nothing in this module tree parses it.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Space {
	pub id: String,
	pub name: String,
	pub active_section: String,
	pub sections: Vec<Section>,
	pub notes: Vec<Note>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Section {
	pub id: String,
	pub name: String,
	pub order: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Note {
	pub id: String,
	pub section: String,
	pub order: i64,
	pub done: bool,
	pub body: String,
	pub created: String,
	pub updated: String,
}

/// The name given to the section `normalise` inserts into a document that has
/// none, and to the section a newly created space starts with.
pub const DEFAULT_SECTION_NAME: &str = "Notes";

impl Space {
	/// Whether `id` names a section of this document.
	pub fn has_section(&self, id: &str) -> bool {
		self.sections.iter().any(|section| section.id == id)
	}

	pub fn note(&self, id: &str) -> Option<&Note> {
		self.notes.iter().find(|note| note.id == id)
	}

	pub fn note_mut(&mut self, id: &str) -> Option<&mut Note> {
		self.notes.iter_mut().find(|note| note.id == id)
	}

	pub fn section_index(&self, id: &str) -> Option<usize> {
		self.sections.iter().position(|section| section.id == id)
	}
}
