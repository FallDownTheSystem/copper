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
	/// Metadata only — the bytes live in the space's sidecar assets directory.
	///
	/// Declared here, between `body` and `created`, because declaration order is
	/// key order and the position is asserted by the golden fixture. `skip_
	/// serializing_if` is what keeps every document without attachments byte
	/// identical to what earlier phases wrote: the key is absent, not `[]`.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub attachments: Vec<Attachment>,
	pub created: String,
	pub updated: String,
}

/// One attached file, as the document records it.
///
/// Every field here is **untrusted on the way back in**. The document is
/// hand-editable and git-writable, so `file` is validated against
/// `attachments::is_bare_filename` on load and again at every command that
/// resolves it to a path; `width`/`height` are advisory and are never used to
/// size an allocation.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
	pub id: String,
	/// The content-addressed **bare filename** inside the assets directory.
	/// Never a path, and never trusted to be one.
	pub file: String,
	/// What the user's copy was called. Display only — it is never a storage
	/// name, which is what makes traversal, collision and Windows reserved
	/// device names structurally impossible rather than sanitised away.
	pub name: String,
	pub mime: String,
	pub bytes: u64,
	/// Present for images only, and advisory even then.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub width: Option<u32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub height: Option<u32>,
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
