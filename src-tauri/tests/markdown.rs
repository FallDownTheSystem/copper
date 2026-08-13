//! Which notes each copy scope resolves to, and in which order.
//!
//! The *formatting* is not tested here. It is `copper_core::markdown`'s, and
//! `copper-core/tests/markdown.rs` holds the whole of `noteMarkdown.test.ts`'s
//! corpus byte for byte — so re-asserting a rendering here would be a second
//! copy of a contract that already has one. What is this module's own is the
//! step in front of the renderer: a `NoteSelection` and a document going in, a
//! grouped set of notes coming out, and the two refusals when a selection names
//! something the document does not hold.
//!
//! Every test drives `markdown::render` directly rather than the
//! `#[tauri::command]` wrapper. The wrapper is `active_space()` and this call,
//! and a `State<'_, SharedStore>` cannot be built outside a running app — see
//! `tests/commands.rs` for why the mock runtime is not an option here either.

use std::sync::Arc;

use copper_core::store::error::StoreError;
use copper_core::store::events::NullSink;
use copper_core::store::model::{Note, Section, Space};
use copper_core::store::{self, Store};

use copper_lib::markdown::{render, render_active, MarkdownFormat, NoteSelection, RenderedNotes};

// --- the document every test resolves against --------------------------------

/// Two sections holding notes and one holding none, which is the shape the
/// empty-section rules below are about. `Research`'s second note opens a fence,
/// so the renderer's block form shows up in the expected text and a grouping
/// that silently dropped a continuation line would be visible.
fn space() -> Space {
	Space {
		id: "spc_00000001".into(),
		name: "test".into(),
		active_section: "sec_00000001".into(),
		sections: vec![
			section("sec_00000001", "Research", 0),
			section("sec_00000002", "Inbox", 1),
			section("sec_00000003", "Archive", 2),
		],
		notes: vec![
			note("nte_00000001", "sec_00000001", 0, false, "first note"),
			note(
				"nte_00000002",
				"sec_00000001",
				1,
				true,
				"```js\nconst a = 1\n```",
			),
			note(
				"nte_00000003",
				"sec_00000002",
				0,
				true,
				"a note in the second section",
			),
		],
	}
}

fn section(id: &str, name: &str, order: i64) -> Section {
	Section {
		id: id.into(),
		name: name.into(),
		order,
	}
}

fn note(id: &str, section: &str, order: i64, done: bool, body: &str) -> Note {
	Note {
		id: id.into(),
		section: section.into(),
		order,
		done,
		body: body.into(),
		attachments: Vec::new(),
		created: "2026-08-05T00:00:00Z".into(),
		updated: "2026-08-05T00:00:00Z".into(),
	}
}

const RESEARCH: &str = "# Research\n- [ ] first note\n- [x]\n\n  ```js\n  const a = 1\n  ```";
const INBOX: &str = "# Inbox\n- [x] a note in the second section";
const ARCHIVE: &str = "# Archive";

fn markdown(selection: NoteSelection) -> RenderedNotes {
	render(&space(), &selection, MarkdownFormat::Markdown).expect("the selection resolves")
}

fn ids(ids: &[&str]) -> NoteSelection {
	NoteSelection::Ids {
		ids: ids.iter().map(|id| (*id).to_string()).collect(),
	}
}

// --- the three selections ----------------------------------------------------

/// A section that holds nothing is still part of the document, so its heading is
/// part of a copy of the document. `count` is notes, never sections, which is
/// what makes the frontend's toast say "Copied 3 notes" rather than counting the
/// headings it also put on the clipboard.
#[test]
fn the_document_scope_keeps_an_empty_section_as_a_bare_heading() {
	let rendered = markdown(NoteSelection::Document);

	assert_eq!(rendered.text, format!("{RESEARCH}\n\n{INBOX}\n\n{ARCHIVE}"));
	assert_eq!(rendered.count, 3);
}

#[test]
fn the_section_scope_is_one_section_and_all_of_it() {
	let rendered = markdown(NoteSelection::Section {
		id: "sec_00000001".into(),
	});

	assert_eq!(rendered.text, RESEARCH);
	assert_eq!(rendered.count, 2);
}

/// A heading with nothing under it is not an error — the frontend declines to
/// replace the clipboard with it, and it does so by reading `count`, which is
/// why the zero matters more than the text does.
#[test]
fn an_empty_section_renders_its_heading_and_counts_nothing() {
	let rendered = markdown(NoteSelection::Section {
		id: "sec_00000003".into(),
	});

	assert_eq!(rendered.text, ARCHIVE);
	assert_eq!(rendered.count, 0);
}

/// The ordering-semantics regression, and the reason resolution moved here at
/// all: the rendering is a function of *which* notes were chosen, never of the
/// order they were named in. `PanelShell.test.ts` asserts the frontend's half —
/// that `targetIds()` hands over canonical order even while a search has
/// reordered the rows — and this is the same property one level down, where a
/// caller that got it wrong can no longer break the output.
#[test]
fn scrambled_ids_render_byte_identically_to_canonical_ones() {
	let canonical = markdown(ids(&["nte_00000001", "nte_00000002", "nte_00000003"]));
	let scrambled = markdown(ids(&["nte_00000003", "nte_00000001", "nte_00000002"]));

	assert_eq!(scrambled.text, canonical.text);
	assert_eq!(scrambled.count, canonical.count);
	// Not vacuously equal: both carry both sections, in document order, and the
	// fence-bearing note's block form.
	assert_eq!(canonical.text, format!("{RESEARCH}\n\n{INBOX}"));
	assert_eq!(canonical.count, 3);
}

/// AC8/AC9's frontend rule, now structural: copying two notes out of one section
/// produces one heading rather than the document's whole outline.
#[test]
fn the_ids_scope_drops_sections_that_contribute_nothing() {
	let rendered = markdown(ids(&["nte_00000001"]));

	assert_eq!(rendered.text, "# Research\n- [ ] first note");
	assert_eq!(rendered.count, 1);
}

/// The select-all case: the same notes as the whole document, differing only by
/// the empty section's heading — which is the documented rule rather than a
/// disagreement between the two scopes.
#[test]
fn selecting_every_note_matches_the_document_but_for_the_empty_heading() {
	let document = markdown(NoteSelection::Document);
	let everything = markdown(ids(&["nte_00000001", "nte_00000002", "nte_00000003"]));

	assert_eq!(document.count, everything.count);
	assert_eq!(document.text, format!("{}\n\n{ARCHIVE}", everything.text));
}

/// Nothing selected is not a failure. The frontend calls this before it knows
/// whether the answer is worth writing, and an error would make it handle a
/// refusal for what is simply an empty result.
#[test]
fn an_empty_id_list_renders_nothing_rather_than_failing() {
	let rendered = markdown(ids(&[]));

	assert_eq!(rendered.text, "");
	assert_eq!(rendered.count, 0);
}

// --- the two body-only formats -----------------------------------------------

/// `Bodies` and `List` have no notion of a section, so the grouping decides only
/// the order — which means a document-scoped call to either is well defined even
/// though no copy affordance makes one today.
#[test]
fn the_body_only_formats_flatten_the_grouping_in_document_order() {
	let document = space();

	let bodies = render(&document, &NoteSelection::Document, MarkdownFormat::Bodies).unwrap();
	assert_eq!(
		bodies.text,
		"first note\n\n```js\nconst a = 1\n```\n\na note in the second section"
	);
	assert_eq!(bodies.count, 3);

	let list = render(&document, &ids(&["nte_00000003", "nte_00000001"]), MarkdownFormat::List)
		.unwrap();
	// Document order, not argument order — and no checkbox syntax, whatever the
	// notes' own done state, which is `list_markdown`'s recorded contract.
	assert_eq!(list.text, "- first note\n- a note in the second section");
	assert_eq!(list.count, 2);
}

// --- the refusals ------------------------------------------------------------

/// One unresolvable id fails the whole call rather than being skipped. Silently
/// dropping it would put fewer notes on the clipboard than the toast claims, and
/// `ops.rs` validates completely before acting for the same reason.
#[test]
fn an_unknown_note_id_fails_the_whole_call() {
	let error = render(
		&space(),
		&ids(&["nte_00000001", "nte_ffffffff"]),
		MarkdownFormat::Markdown,
	)
	.expect_err("an id that names nothing is a refusal");

	assert!(matches!(error, StoreError::NotFound(_)), "{error:?}");
	assert!(error.message().contains("nte_ffffffff"), "{error:?}");
	assert_eq!(error.kind(), "not-found");
}

#[test]
fn an_unknown_section_id_is_not_found() {
	let error = render(
		&space(),
		&NoteSelection::Section {
			id: "sec_ffffffff".into(),
		},
		MarkdownFormat::Markdown,
	)
	.expect_err("a section that names nothing is a refusal");

	assert!(matches!(error, StoreError::NotFound(_)), "{error:?}");
	assert!(error.message().contains("sec_ffffffff"), "{error:?}");
}

/// The command's other refusal, and the one case that needs a real store: "no
/// space is open" comes from the `active_space()` the command opens with, not
/// from the resolver.
///
/// Driven through `render_active`, which is the command's whole body — so this
/// asserts the path that runs rather than restating it. It matters on an
/// ordinary launch: a space on a drive that is not mounted leaves the store with
/// nothing open, and a copy chord then has to say so rather than write an empty
/// clipboard.
#[test]
fn a_store_with_no_space_open_refuses_before_any_rendering() {
	let dir = tempfile::tempdir().unwrap();
	let mut store: Store =
		store::bootstrap_store(&dir.path().join("Copper"), Arc::new(NullSink)).unwrap();

	// Bootstrap always opens something, so the seam is proved to work before it is
	// proved to refuse — otherwise a `render_active` that refused everything would
	// pass this test.
	let opened = render_active(&store, &NoteSelection::Document, MarkdownFormat::Markdown)
		.expect("bootstrap leaves a space open");
	assert_eq!(opened.count, 0, "a fresh space holds no notes");

	store.close_space();

	let error = render_active(&store, &NoteSelection::Document, MarkdownFormat::Markdown)
		.expect_err("nothing is open, so there is no document to render");

	assert!(matches!(error, StoreError::Unavailable(_)), "{error:?}");
	assert_eq!(error.kind(), "unavailable");
}

// --- the shapes that cross the boundary --------------------------------------

/// The wire spelling of the selection, which is the frontend's half of the
/// contract. `kind` rather than serde's default enum encoding, matching
/// `OpenOutcome` — one shape for a discriminated union, so the frontend needs
/// one way to build them rather than two.
#[test]
fn a_note_selection_arrives_tagged_by_kind() {
	let ids: NoteSelection =
		serde_json::from_value(serde_json::json!({ "kind": "ids", "ids": ["nte_00000001"] }))
			.unwrap();
	assert!(matches!(ids, NoteSelection::Ids { ids } if ids == ["nte_00000001"]));

	let section: NoteSelection =
		serde_json::from_value(serde_json::json!({ "kind": "section", "id": "sec_00000001" }))
			.unwrap();
	assert!(matches!(section, NoteSelection::Section { id } if id == "sec_00000001"));

	let document: NoteSelection =
		serde_json::from_value(serde_json::json!({ "kind": "document" })).unwrap();
	assert!(matches!(document, NoteSelection::Document));

	// A kind nobody defined is a deserialisation failure, so a typo in the
	// frontend is an `invalid` reply rather than a silently empty clipboard.
	assert!(serde_json::from_value::<NoteSelection>(serde_json::json!({ "kind": "all" })).is_err());
}

/// The three format values, spelled exactly as `copper copy --format` spells
/// them. Two vocabularies for one set of renderers is the drift the port exists
/// to prevent, and this is the assertion that keeps them one.
#[test]
fn the_format_vocabulary_matches_the_cli_flag() {
	for (wire, expected) in [
		("bodies", MarkdownFormat::Bodies),
		("list", MarkdownFormat::List),
		("markdown", MarkdownFormat::Markdown),
	] {
		let parsed: MarkdownFormat = serde_json::from_value(serde_json::json!(wire)).unwrap();
		assert_eq!(parsed, expected);
	}

	// The CLI's fourth format is deliberately not part of the app's surface.
	assert!(serde_json::from_value::<MarkdownFormat>(serde_json::json!("json")).is_err());
}

#[test]
fn a_rendering_crosses_the_boundary_as_text_count_and_ids() {
	let payload = serde_json::to_value(RenderedNotes {
		text: "# Research".into(),
		count: 0,
		ids: vec![],
	})
	.unwrap();

	assert_eq!(payload["text"], "# Research");
	assert_eq!(payload["count"], 0);
	assert_eq!(payload["ids"], serde_json::json!([]));
	assert_eq!(
		payload.as_object().unwrap().len(),
		3,
		"render_notes_markdown grew a field"
	);
}

/// The ids are the rendered set in the order the text used, whatever order the
/// selection arrived in — they exist so `doneOnCopy` marks exactly what went to
/// the clipboard, and a set resolved a second time on the frontend could not
/// promise that.
#[test]
fn a_rendering_names_the_notes_it_rendered_in_document_order() {
	let rendered = markdown(NoteSelection::Document);
	assert_eq!(
		rendered.ids,
		["nte_00000001", "nte_00000002", "nte_00000003"]
	);

	let rendered = markdown(ids(&["nte_00000003", "nte_00000001"]));
	assert_eq!(rendered.ids, ["nte_00000001", "nte_00000003"]);

	let rendered = markdown(NoteSelection::Section {
		id: "sec_00000003".into(),
	});
	assert_eq!(rendered.ids, Vec::<String>::new());
}
