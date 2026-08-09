//! The clipboard payloads, rendered where the document is.
//!
//! One command, over [`copper_core::markdown`] — the renderer the CLI already
//! copies through. Before task-024 the app rendered its own Markdown in
//! `src/lib/noteMarkdown.ts` and the CLI rendered Rust, which is two
//! implementations of one clipboard format and therefore two formats. This is
//! the app's half of retiring that: the frontend now says *which* notes it wants
//! and in which of the three shapes, and never marshals a body again.
//!
//! **Beside `clipboard.rs` rather than inside `store/commands.rs`**, following
//! the precedent task-006 set with that file: `store/commands.rs`'s own doc
//! pins its contents to spec 8.1's twenty plus one, and this is not a store
//! mutation — it reads the open document and returns text. What it does *not*
//! copy from `clipboard.rs` is that file's plain-`String` error: a failed
//! clipboard write has nothing for a caller to branch on, and this command has
//! two real refusals (`unavailable`, `not-found`), so it uses the store's
//! ordinary `{ kind, message }` shape like every other command with something to
//! say.
//!
//! **The clipboard is not written here.** `win32::clipboard::write_text_private`
//! stays the one door — it is what keeps every copy out of `Win+V` history — so
//! the frontend still calls `clipboard_write_text` with what this returns. A
//! second writer would fork that path for no gain, and whether a result is worth
//! replacing the clipboard with is a frontend question this command has no
//! business answering.

use serde::{Deserialize, Serialize};
use tauri::State;

use copper_core::markdown::{copy_markdown, list_markdown, section_markdown, MarkdownSection};
use copper_core::store::error::StoreError;
use copper_core::store::model::{Note, Space};
use copper_core::store::{lock, SharedStore, Store};

type Reply<T> = std::result::Result<T, StoreError>;

/// Which notes a copy affordance is asking for.
///
/// `tag = "kind"` rather than a new tagging convention: `editor_open_note`'s
/// `OpenOutcome` already crosses this boundary as `{ kind: "opened" } | …`, so
/// the frontend reads one shape for a discriminated union rather than two.
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NoteSelection {
	/// The notes an action targets, whatever order they were named in — see
	/// [`resolve`] for why the order they arrive in is discarded.
	Ids { ids: Vec<String> },
	/// One section and everything in it, whatever a search field holds.
	Section { id: String },
	/// Every section of the document, empty ones included.
	Document,
}

/// Which of the three renderings.
///
/// Spelled to match `copper-cli`'s `copy --format bodies|list|markdown` exactly,
/// because both flags name the same three `copper_core::markdown` functions.
/// One vocabulary for one set of renderers is the whole point of the port; two
/// spellings would be the drift it exists to prevent. The CLI's fourth value,
/// `json`, is absent here on purpose — no copy affordance ever wanted a JSON
/// clipboard payload, and adding it speculatively would widen the surface for a
/// caller that does not exist.
///
/// Kebab-case is the discriminant spelling `StoreError::kind` and `SubmitOutcome`
/// already use; every value here is one word, so it reads as plain lowercase.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkdownFormat {
	Bodies,
	List,
	Markdown,
}

/// The rendering, and how many notes went into it.
///
/// `count` rather than a bare string, and it is not a convenience. The frontend
/// suppresses a copy of nothing and reports "Copied N notes", and computing that
/// N from its own document snapshot means answering a question about the
/// selection this command already resolved — against a document that may have
/// moved since. Returning it makes the number and the text one answer about one
/// document instead of two answers about possibly different ones.
///
/// Both field names are single words, so the camelCase conversion Tauri applies
/// to results is a no-op — the same rule `store/commands.rs` holds its
/// parameters to.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct RenderedNotes {
	pub text: String,
	pub count: usize,
}

/// The command's whole body, given a store.
///
/// The seam the integration tests drive, so that "no space is open" is asserted
/// against the code the command actually runs rather than restated as prose. A
/// `State<'_, SharedStore>` cannot be built outside a running app; a `Store` can.
///
/// The guard the caller holds is live across the render, which every other
/// command in the crate avoids. It is safe here for a reason those have and this
/// one does not need: there is no `.await` in the body, nothing is emitted, and
/// the work is bounded by the document's own size — the same bound
/// `active_space`'s clone already carries under the guard in `get_active_space`.
pub fn render_active(
	store: &Store,
	selection: &NoteSelection,
	format: MarkdownFormat,
) -> Reply<RenderedNotes> {
	render(&store.active_space()?, selection, format)
}

/// Resolve and render, given a document.
///
/// Split out so the whole of the interesting behaviour — which notes each
/// selection picks, in which order, and what an unknown id does — is reachable
/// from a test with no store and no filesystem at all.
pub fn render(
	space: &Space,
	selection: &NoteSelection,
	format: MarkdownFormat,
) -> Reply<RenderedNotes> {
	let sections = resolve(space, selection)?;
	let count = sections.iter().map(|section| section.notes.len()).sum();

	let text = match format {
		MarkdownFormat::Bodies => copy_markdown(&bodies(&sections)),
		MarkdownFormat::List => list_markdown(&bodies(&sections)),
		MarkdownFormat::Markdown => section_markdown(&sections),
	};

	Ok(RenderedNotes { text, count })
}

/// The selection as sections and notes, always in canonical document order.
///
/// **`Ids` does not honour the order its ids arrived in.** The notes are
/// re-derived by walking the document and keeping the ones that were asked for,
/// so `["nte_b", "nte_a"]` and `["nte_a", "nte_b"]` render byte-identically.
/// That is not a new rule: the frontend's `targetIds()` already walked the
/// document rather than the selection's insertion order, and `copper copy`
/// reorders its arguments for the same reason. Moving the resolution here is
/// what makes the property structural instead of something three callers each
/// have to remember.
///
/// One id that names nothing fails the whole call rather than being skipped —
/// `ops.rs`'s validate-completely-before-acting rule, held to here even though
/// nothing is being written, because a copy that silently dropped a note would
/// put fewer notes on the clipboard than the toast claims.
///
/// The three selections differ on empty sections, and each way round is
/// deliberate: `Document` and `Section` keep them, so a heading says "this
/// section exists and is empty"; `Ids` drops them, so copying two notes out of
/// one section produces one heading rather than the document's whole outline.
fn resolve<'a>(space: &'a Space, selection: &NoteSelection) -> Reply<Vec<MarkdownSection<'a>>> {
	match selection {
		NoteSelection::Document => Ok(group(space, |_| true)),

		NoteSelection::Section { id } => {
			let section = space
				.sections
				.iter()
				.find(|section| &section.id == id)
				.ok_or_else(|| StoreError::NotFound(format!("no such section: {id}")))?;
			Ok(vec![MarkdownSection {
				name: section.name.as_str(),
				notes: notes_of(space, &section.id, |_| true),
			}])
		}

		NoteSelection::Ids { ids } => {
			for id in ids {
				if space.note(id).is_none() {
					return Err(StoreError::NotFound(format!("no such note: {id}")));
				}
			}
			let mut sections = group(space, |note| ids.iter().any(|id| *id == note.id));
			sections.retain(|section| !section.notes.is_empty());
			Ok(sections)
		}
	}
}

/// Every section, carrying whichever of its notes the predicate keeps.
fn group<'a>(space: &'a Space, wanted: impl Fn(&Note) -> bool) -> Vec<MarkdownSection<'a>> {
	space
		.sections
		.iter()
		.map(|section| MarkdownSection {
			name: section.name.as_str(),
			notes: notes_of(space, &section.id, &wanted),
		})
		.collect()
}

/// One section's notes, in document order, as the renderer wants them.
fn notes_of<'a>(
	space: &'a Space,
	section: &str,
	wanted: impl Fn(&Note) -> bool,
) -> Vec<(bool, &'a str)> {
	space
		.notes
		.iter()
		.filter(|note| note.section == section && wanted(note))
		.map(|note| (note.done, note.body.as_str()))
		.collect()
}

/// The grouping flattened, for the two body-only renderings.
///
/// `Bodies` and `List` have no notion of a section, so the grouping only decides
/// the order they come out in — which means a document-scoped or section-scoped
/// call to either is well defined even though no copy affordance makes one
/// today.
fn bodies<'a>(sections: &[MarkdownSection<'a>]) -> Vec<&'a str> {
	sections
		.iter()
		.flat_map(|section| section.notes.iter().map(|&(_, body)| body))
		.collect()
}

#[tauri::command]
pub async fn render_notes_markdown(
	selection: NoteSelection,
	format: MarkdownFormat,
	state: State<'_, SharedStore>,
) -> Reply<RenderedNotes> {
	render_active(&lock(&state), &selection, format)
}
