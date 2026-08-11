//! What is left of the store's filesystem tests on the app's side: the paths
//! that go through a command wrapper.
//!
//! The bulk of them — the write pipeline, conflicts, undo, the watcher, startup
//! — are `copper-core/tests/store_fs.rs`, because the code they exercise moved
//! there. Three entry points did not, and these are their tests:
//! `store::commands::submit` (the composer's `# Name` directive rule lives in
//! the wrapper, not in `ops`), `store::commands::add` beside it, and
//! `attachments::ingest` (it decodes an image header, so it stays with the
//! `image` crate). The attachment block is the largest group here, and it is
//! here for the third reason rather than the first two.
//!
//! The `Harness` below is a deliberate duplicate of the one in
//! `copper-core/tests/store_fs.rs`. One crate's integration tests cannot reach
//! another crate's, and a third crate existing only to share fifteen lines of
//! `bootstrap_store` plumbing would be more machinery than these tests justify.
//!
//! Anything involving the watcher **polls**; it never sleeps a fixed time and
//! assumes. notify's timing on Windows depends on the backend and the machine.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use copper_core::store::error::StoreError;
use copper_core::store::events::{ChangeReason, RecordingSink, StoreEvent};
use copper_core::store::model::{Attachment, Note, Section, Space};
use copper_core::store::settings::InsertionPoint;
use copper_core::store::{self, atomic, format, ops, SharedStore, StoreStatus};

// `attachments::` is the core's path layer; `ingest` alone is the app's, because
// it decodes an image header. Two names rather than one, so which side of the
// boundary each call lands on is visible at the call site.
use copper_core::attachments;
use copper_lib::attachments::ingest;
use copper_lib::store::commands::{self, submit, SubmitOutcome, SubmitResult};

// --- harness -----------------------------------------------------------------
struct Harness {
	_dir: tempfile::TempDir,
	shared: SharedStore,
	sink: Arc<RecordingSink>,
}

impl Harness {
	fn new() -> Self {
		let dir = tempfile::tempdir().unwrap();
		let config = dir.path().join("Copper");
		let sink = Arc::new(RecordingSink::new());
		let store = store::bootstrap_store(&config, sink.clone()).unwrap();
		let shared: SharedStore = Arc::new(Mutex::new(store));
		assert!(
			store::attach_watcher(&shared).is_empty(),
			"the watch should register cleanly in a temp directory, with nothing to reconcile"
		);
		assert!(sink.take().is_empty(), "startup emitted an event (spec 8A.2)");
		Self {
			_dir: dir,
			shared,
			sink,
		}
	}

	fn path(&self) -> PathBuf {
		store::lock(&self.shared)
			.active_path()
			.expect("a space is always open")
			.to_path_buf()
	}

	fn doc(&self) -> Space {
		store::lock(&self.shared).active_space().unwrap()
	}

	fn status(&self) -> StoreStatus {
		store::lock(&self.shared).status()
	}

	fn text(&self) -> String {
		std::fs::read_to_string(self.path()).unwrap()
	}

	fn add(&self, body: &str) -> Result<String, StoreError> {
		let body = body.to_string();
		store::lock(&self.shared)
			.mutate(|doc| ops::add_note(doc, &body, None, &[], InsertionPoint::Bottom))
			.map(|(id, _)| id)
	}

	/// The composer's submit, through the same function the command calls.
	fn submit(&self, body: &str) -> Result<SubmitResult, StoreError> {
		submit(&self.shared, body, &[])
	}

	fn section_named(&self, name: &str) -> Option<String> {
		self.doc()
			.sections
			.iter()
			.find(|section| section.name == name)
			.map(|section| section.id.clone())
	}
}

/// Long enough for the debounce to fire and for anything queued behind it to
/// arrive. Used only to prove that nothing was emitted.
fn settle() {
	thread::sleep(Duration::from_millis(700));
}

/// An atomic external write, so a reader never sees a partial file.
fn external_write(path: &Path, text: &str) {
	atomic::write_atomic(path, text).unwrap();
}

fn reasons(events: &[StoreEvent]) -> Vec<ChangeReason> {
	events
		.iter()
		.filter_map(|event| match event {
			StoreEvent::SpaceChanged(payload) => Some(payload.reason),
			_ => None,
		})
		.collect()
}

// --- submit_entry (task-010) --------------------------------------------------
#[test]
fn a_directive_creates_and_activates_a_section_in_one_undoable_step() {
	let harness = Harness::new();
	harness.add("keep me").unwrap();
	let before_section = harness.doc().active_section.clone();
	let before_notes = harness.doc().notes.len();

	let result = harness.submit("# Research").unwrap();

	assert_eq!(result.outcome, SubmitOutcome::SectionCreated);
	assert!(result.note_id.is_none(), "a directive created a note");
	assert_eq!(harness.doc().notes.len(), before_notes, "a directive created a note");
	assert_eq!(harness.doc().active_section, result.section_id);
	assert_eq!(harness.section_named("Research").as_deref(), Some(result.section_id.as_str()));

	// **One** step, not two: the section and the activation are a single snapshot,
	// so one press takes both back. Two sequential commands would need two.
	assert!(store::lock(&harness.shared).undo().unwrap().is_some());
	assert!(harness.section_named("Research").is_none(), "the section survived the undo");
	assert_eq!(
		harness.doc().active_section,
		before_section,
		"the previously active section was not restored"
	);
	// And it stops there: the note added before the directive is still present.
	assert_eq!(harness.doc().notes.len(), before_notes);
}

#[test]
fn a_duplicate_directive_activates_without_pushing_a_snapshot() {
	let harness = Harness::new();
	harness.submit("# Research").unwrap();
	let research = harness.section_named("Research").unwrap();
	// Something undoable, so an unwanted snapshot would be visible as an undo that
	// restores the wrong thing rather than as an empty stack.
	let note = harness.add("alpha").unwrap();
	store::lock(&harness.shared)
		.mutate_no_snapshot(|doc| ops::set_active_section(doc, &research))
		.unwrap();
	let other = harness
		.doc()
		.sections
		.iter()
		.find(|section| section.id != research)
		.unwrap()
		.id
		.clone();
	store::lock(&harness.shared)
		.mutate_no_snapshot(|doc| ops::set_active_section(doc, &other))
		.unwrap();

	// Case and whitespace fold, so this is the same destination.
	let result = harness.submit("#   research  ").unwrap();

	assert_eq!(result.outcome, SubmitOutcome::SectionActivated);
	assert_eq!(result.section_id, research);
	assert_eq!(harness.doc().sections.len(), 2, "a second Research was created");
	assert_eq!(harness.doc().active_section, research);

	// No snapshot of its own, so one Ctrl+Z undoes whatever preceded it — the note.
	assert!(store::lock(&harness.shared).undo().unwrap().is_some());
	assert!(harness.doc().note(&note).is_none(), "the undo did not reach the note");
}

#[test]
fn a_submission_that_is_not_a_directive_is_an_ordinary_note() {
	let harness = Harness::new();

	// Stored byte-identically to what was submitted. `\# Research` is the one
	// exception, and the only body this path rewrites: the backslash escaped a
	// directive, so it is consumed.
	let bodies = [
		("# Research\n\nwith more text", "# Research\n\nwith more text"),
		("## Research", "## Research"),
		("#Research", "#Research"),
		("#", "#"),
		("\\# Research", "# Research"),
		// The backslash is kept when it escaped nothing — neither of these was ever
		// going to be a directive.
		("\\#Research", "\\#Research"),
		("\\## Research", "\\## Research"),
	];

	for (body, expected) in bodies {
		let result = harness.submit(body).unwrap();
		assert_eq!(result.outcome, SubmitOutcome::Note, "{body:?} was treated as a directive");
		let id = result.note_id.expect("a note outcome carries its id");
		assert_eq!(
			result.section_id, harness.doc().active_section,
			"{body:?} landed outside the active section"
		);
		assert_eq!(harness.doc().note(&id).unwrap().body, expected);
	}

	// `#   ` is a note, and the store trims its trailing whitespace on the way in —
	// `ops::clean_body` has done that to every body since task-003, and AC3's
	// "no trimming beyond the composer's existing untrimmed-submit rule" is what
	// admits it. Pinned because nothing else would catch a change to it.
	let result = harness.submit("#   ").unwrap();
	assert_eq!(result.outcome, SubmitOutcome::Note);
	let id = result.note_id.unwrap();
	assert_eq!(harness.doc().note(&id).unwrap().body, "#");

	assert_eq!(harness.doc().sections.len(), 1, "a note submission created a section");
}

/// The snapshot decision and the operation must be taken against the **same**
/// document, and a write conflict is what pulls them apart: the operation is
/// re-applied to the external document, so a decision made from the local view
/// can describe a mutation that did not happen.
///
/// Both divergence directions, because they fail in opposite ways — one leaves a
/// structural change with no undo entry, the other pushes a snapshot for a
/// mutation that only moved `activeSection`.
#[test]
fn a_conflict_rebases_the_snapshot_decision_when_the_section_vanished_externally() {
	let harness = Harness::new();
	harness.submit("# Research").unwrap();
	assert!(harness.section_named("Research").is_some());

	// The section exists locally. Externally it is gone, so the re-applied
	// operation will *create* it — and deciding from the local view would have
	// chosen `mutate_no_snapshot`, leaving AC5 with nothing to undo.
	let mut external = format::from_json(&harness.text()).unwrap();
	let research = harness.section_named("Research").unwrap();
	ops::delete_section(&mut external, &research).unwrap();
	external_write(&harness.path(), &format::to_git_json(&external).unwrap());

	let result = harness.submit("# Research").unwrap();

	assert_eq!(result.outcome, SubmitOutcome::SectionCreated);
	assert_eq!(harness.section_named("Research").as_deref(), Some(result.section_id.as_str()));
	assert!(
		harness.status().can_undo,
		"a section was created with no undo entry behind it"
	);
	// The rebase cleared the stack and pushed exactly one entry — the one that
	// reverts this operation and stops.
	assert!(store::lock(&harness.shared).undo().unwrap().is_some());
	assert!(harness.section_named("Research").is_none());
	assert!(!harness.status().can_undo);
}

#[test]
fn a_conflict_rebases_the_snapshot_decision_when_the_section_appeared_externally() {
	let harness = Harness::new();

	// Absent locally, present externally, so the re-applied operation only
	// activates — and deciding from the local view would have pushed a snapshot for
	// a navigational change, breaking AC2.
	let mut external = format::from_json(&harness.text()).unwrap();
	let research = ops::add_section(&mut external, "Research").unwrap();
	external_write(&harness.path(), &format::to_git_json(&external).unwrap());

	let result = harness.submit("# Research").unwrap();

	assert_eq!(result.outcome, SubmitOutcome::SectionActivated);
	assert_eq!(result.section_id, research);
	assert_eq!(harness.doc().active_section, research);
	assert_eq!(
		harness.doc().sections.len(),
		2,
		"the rebased operation created a second Research"
	);
	// A rebase clears the stack, and an activation pushes nothing to replace it —
	// so there is nothing to undo, which is exactly right: every older entry
	// predates the external change and undoing one would destroy it.
	assert!(!harness.status().can_undo, "an activation pushed a snapshot");
	assert!(store::lock(&harness.shared).undo().unwrap().is_none());
}

#[test]
fn submit_entry_emits_nothing_on_any_of_its_three_paths() {
	let harness = Harness::new();

	// Spec 8.4: a frontend-invoked mutation's return value already describes the
	// change, so an event would only duplicate it.
	harness.submit("an ordinary note").unwrap();
	harness.submit("# Research").unwrap();
	harness.submit("# Research").unwrap();
	harness.submit("   ").unwrap_err();

	assert!(
		harness.sink.events().is_empty(),
		"submit_entry emitted: {:?}",
		harness.sink.names()
	);
	settle();
	assert!(
		harness.sink.events().is_empty(),
		"submit_entry's own write produced a watcher event: {:?}",
		harness.sink.names()
	);
}

/// Task-018. The capture notification names the destination and offers the rest,
/// and both answers come out of the write's own guard — so this asserts the
/// *content* of that answer rather than that a second read could produce it.
#[test]
fn append_capture_reports_where_the_note_landed_and_where_else_it_could_go() {
	let harness = Harness::new();
	harness.submit("# Ideas").unwrap();
	harness.submit("# Tasks").unwrap();

	let landed = store::append_capture(&harness.shared, "captured").unwrap();

	assert_eq!(
		landed.section.name, "Tasks",
		"the destination is the active section at the moment of the write"
	);
	assert_eq!(harness.doc().note(&landed.note).unwrap().section, landed.section.id);
	// Document order, not creation order and not activation order: no most-recently-
	// used state exists anywhere in this codebase, and inventing one for a
	// notification would be a second source of truth about section ordering.
	assert_eq!(
		landed
			.alternatives
			.iter()
			.map(|section| section.name.as_str())
			.collect::<Vec<_>>(),
		["Notes", "Ideas"]
	);
	assert!(
		landed.alternatives.iter().all(|section| section.id != landed.section.id),
		"the destination was offered as an alternative to itself"
	);
}

/// The other half of task-018: a notification's re-route button reaches the same
/// writer the panel does, without going out through IPC to get there.
#[test]
fn move_notes_from_rust_emits_one_reroute_and_is_undoable() {
	let harness = Harness::new();
	let landed = store::append_capture(&harness.shared, "captured").unwrap();
	let elsewhere = harness.submit("# Ideas").unwrap().section_id;
	harness.sink.take();

	store::move_notes(&harness.shared, std::slice::from_ref(&landed.note), &elsewhere).unwrap();

	let events = harness.sink.take();
	assert_eq!(events.len(), 1);
	// Not `Capture`: the panel answers that reason with a sound and a scroll
	// request, and nothing was captured here.
	assert_eq!(reasons(&events), [ChangeReason::Reroute]);
	assert_eq!(harness.doc().note(&landed.note).unwrap().section, elsewhere);

	// One snapshot, so a re-route the user did not mean is one `Ctrl+Z` — exactly
	// what the same move made from the panel costs.
	assert!(harness.status().can_undo);
	store::lock(&harness.shared).undo().unwrap();
	assert_eq!(
		harness.doc().note(&landed.note).unwrap().section,
		landed.section.id
	);
}

/// Three functions create a note and every one of them has to read the setting:
/// `add_note` (the zero-focus paste), `submit` (the composer) and
/// `append_capture` (the global hotkey). Driven through the functions their
/// callers actually reach rather than through `ops::add_note`, which takes the
/// placement as an argument and so cannot show whether anything read it.
#[test]
fn every_write_path_reads_the_insertion_point_setting() {
	let harness = Harness::new();

	fn ids(doc: &Space) -> Vec<String> {
		doc.notes.iter().map(|note| note.id.clone()).collect()
	}

	// The shipped default appends, which is what every build before this feature
	// did — and is the witness that the assertions below are reading a setting
	// rather than a constant.
	let pasted = commands::add(&harness.shared, "pasted", None).unwrap().note_id;
	let composed = harness.submit("composed").unwrap().note_id.unwrap();
	let captured = store::append_capture(&harness.shared, "captured").unwrap().note;
	assert_eq!(
		ids(&harness.doc()),
		vec![pasted.clone(), composed.clone(), captured.clone()],
		"the default is bottom"
	);

	store::lock(&harness.shared)
		.update_settings(serde_json::from_str(r#"{"insertionPoint":"top"}"#).unwrap())
		.unwrap();

	let top_pasted = commands::add(&harness.shared, "pasted at the top", None).unwrap().note_id;
	assert_eq!(position_of(&harness.doc(), &top_pasted), 0, "add_note ignored the setting");

	let top_composed = harness.submit("composed at the top").unwrap().note_id.unwrap();
	assert_eq!(position_of(&harness.doc(), &top_composed), 0, "submit ignored the setting");

	let top_captured = store::append_capture(&harness.shared, "captured at the top").unwrap().note;
	assert_eq!(
		position_of(&harness.doc(), &top_captured),
		0,
		"append_capture ignored the setting"
	);

	// Newest first among the three, and the notes that were already there keep
	// their order behind them.
	assert_eq!(
		ids(&harness.doc()),
		vec![top_captured, top_composed, top_pasted, pasted, composed, captured]
	);
	// `order` is renumbered contiguously from zero by `normalise`, so `-1` never
	// reaches disk.
	for (index, note) in harness.doc().notes.iter().enumerate() {
		assert_eq!(note.order, index as i64);
	}
}

fn position_of(doc: &Space, id: &str) -> usize {
	doc.notes
		.iter()
		.position(|note| note.id == id)
		.expect("the note is in the document")
}

/// The batch behind "paste as separate notes": one snapshot for the whole
/// list, and the pasted order preserved under **both** insertion points — under
/// `top` the op walks the bodies in reverse, and this is what would catch a
/// forward walk showing the list upside down.
#[test]
fn add_notes_is_one_undo_step_and_keeps_the_pasted_order() {
	let harness = Harness::new();
	harness.add("already here").unwrap();

	fn bodies_of(doc: &Space) -> Vec<String> {
		doc.notes.iter().map(|note| note.body.clone()).collect()
	}

	let bodies: Vec<String> = ["one", "two", "three"].map(String::from).to_vec();
	let result = commands::add_many(&harness.shared, &bodies, None).unwrap();

	// The shipped default appends: the batch follows the existing note, in order.
	assert_eq!(bodies_of(&harness.doc()), vec!["already here", "one", "two", "three"]);
	// The ids answer in `bodies` order, which is what the caller reveals by.
	let answered: Vec<String> = result
		.note_ids
		.iter()
		.map(|id| harness.doc().note(id).expect("the id names a note").body.clone())
		.collect();
	assert_eq!(answered, bodies);

	// One snapshot: a single undo removes the whole batch and nothing else.
	store::lock(&harness.shared).undo().unwrap().unwrap();
	assert_eq!(bodies_of(&harness.doc()), vec!["already here"]);

	store::lock(&harness.shared)
		.update_settings(serde_json::from_str(r#"{"insertionPoint":"top"}"#).unwrap())
		.unwrap();

	let result = commands::add_many(&harness.shared, &bodies, None).unwrap();

	// Top insertion: the batch leads the list and still reads top-to-bottom in
	// the pasted order, with the ids still answering in `bodies` order.
	assert_eq!(bodies_of(&harness.doc()), vec!["one", "two", "three", "already here"]);
	let answered: Vec<String> = result
		.note_ids
		.iter()
		.map(|id| harness.doc().note(id).expect("the id names a note").body.clone())
		.collect();
	assert_eq!(answered, bodies);
}

/// A bad body anywhere in the list refuses the whole batch: the op runs against
/// `mutate`'s scratch copy, so nothing reaches the document or the undo stack —
/// never half a pasted list.
#[test]
fn add_notes_refuses_the_whole_batch_on_one_bad_body() {
	let harness = Harness::new();

	let bodies: Vec<String> = ["good", "   "].map(String::from).to_vec();
	let error = commands::add_many(&harness.shared, &bodies, None).unwrap_err();

	assert!(matches!(error, StoreError::Invalid(_)));
	assert!(harness.doc().notes.is_empty(), "a refused batch still added a note");
	assert!(!harness.status().can_undo, "a refused batch pushed a snapshot");
}

/// The multi-select block move: one `mutate`, so the whole block travels back
/// on a single undo — the same discipline `add_notes` states, at the other end
/// of a note's life.
#[test]
fn reorder_notes_is_one_undo_step_for_the_whole_block() {
	let harness = Harness::new();
	let first = harness.add("first").unwrap();
	let second = harness.add("second").unwrap();
	let third = harness.add("third").unwrap();

	fn order(doc: &Space) -> Vec<String> {
		doc.notes.iter().map(|note| note.id.clone()).collect()
	}

	let section = harness.doc().active_section.clone();
	let block = vec![first.clone(), second.clone()];
	store::lock(&harness.shared)
		.mutate(|doc| ops::reorder_notes(doc, &block, &section, 1))
		.unwrap();
	assert_eq!(order(&harness.doc()), vec![third.clone(), first.clone(), second.clone()]);

	// One snapshot: a single undo puts the whole block back where it was.
	store::lock(&harness.shared).undo().unwrap().unwrap();
	assert_eq!(order(&harness.doc()), vec![first, second, third]);
}

/// A known two-section, two-note document. It is the same fixture
/// `copper-core/tests/store_fs.rs` serialises byte-for-byte; here it is only a
/// second space with recognisable contents to switch to.
fn golden_doc() -> Space {
	Space {
		id: "spc_7f3aa001".into(),
		name: "development".into(),
		active_section: "sec_a1000001".into(),
		sections: vec![
			Section {
				id: "sec_a1000001".into(),
				name: "Research".into(),
				order: 0,
			},
			Section {
				id: "sec_b2000002".into(),
				name: "Configuration Formats".into(),
				order: 1,
			},
		],
		notes: vec![
			Note {
				id: "nte_01000001".into(),
				section: "sec_a1000001".into(),
				order: 0,
				done: false,
				body: "**Negation in inherited configs.** The moment a config can extend a base, \
				       every list-valued option needs a way to say \"not this one\"."
					.into(),
				attachments: Vec::new(),
				created: "2026-07-30T14:02:11Z".into(),
				updated: "2026-07-30T14:02:11Z".into(),
			},
			Note {
				id: "nte_02000002".into(),
				section: "sec_b2000002".into(),
				order: 0,
				done: true,
				body: "Line one.\n\nLine two, indented:\n\tvalue = 1".into(),
				attachments: Vec::new(),
				created: "2026-07-30T14:05:00Z".into(),
				updated: "2026-07-30T14:08:12Z".into(),
			},
		],
	}
}

// --- attachments (task-011) --------------------------------------------------
/// Ingests bytes into the harness's own space, returning the metadata the
/// document will carry. The real path, not a hand-built `Attachment`: these
/// tests are about what `ingest` and the document do together.
fn attach(harness: &Harness, bytes: &[u8], name: &str) -> Attachment {
	ingest(&harness.path(), bytes, name).unwrap()
}

/// A 2×2 PNG whose bytes depend on `seed`, so two calls with different seeds
/// hash differently and one with the same seed hashes identically.
fn png(seed: u8) -> Vec<u8> {
	let mut buffer = std::io::Cursor::new(Vec::new());
	image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
		2,
		2,
		image::Rgb([seed, seed / 2, 7]),
	))
	.write_to(&mut buffer, image::ImageFormat::Png)
	.unwrap();
	buffer.into_inner()
}

fn blobs(space: &Path) -> Vec<String> {
	let mut names: Vec<String> = std::fs::read_dir(attachments::assets_dir(space))
		.map(|entries| {
			entries
				.flatten()
				.map(|entry| entry.file_name().to_string_lossy().into_owned())
				.collect()
		})
		.unwrap_or_default();
	names.sort();
	names
}

/// Puts `files` on an existing note, through the ordinary write pipeline.
fn set_attachments(harness: &Harness, id: &str, files: Vec<Attachment>) {
	let id = id.to_string();
	store::lock(&harness.shared)
		.mutate(|doc| {
			doc.note_mut(&id)
				.ok_or_else(|| StoreError::NotFound(id.clone()))?
				.attachments = files.clone();
			Ok(())
		})
		.unwrap();
}

/// The field's placement is not cosmetic: key order is struct declaration
/// order, so moving it changes every `.copper` file the next time it is
/// written, and the diff-minimality guarantee is measured against that order.
#[test]
fn attachments_serialise_between_body_and_created() {
	let harness = Harness::new();
	let file = attach(&harness, &png(1), "shot.png");
	let id = harness.add("with a file").unwrap();
	set_attachments(&harness, &id, vec![file]);

	let text = harness.text();
	let note = text.split(&id).nth(1).expect("the note is in the document");
	let positions: Vec<usize> = ["body", "attachments", "created", "updated"]
		.into_iter()
		.map(|key| {
			note.find(&format!("\"{key}\""))
				.unwrap_or_else(|| panic!("{key} is missing from the note"))
		})
		.collect();
	assert!(
		positions.windows(2).all(|pair| pair[0] < pair[1]),
		"attachments is not between body and created: {positions:?}"
	);
}

/// AC6. Attaching a file to one note may only touch that note's lines.
#[test]
fn adding_an_attachment_touches_only_that_note() {
	let harness = Harness::new();
	harness.add("first").unwrap();
	let target = harness.add("second").unwrap();
	harness.add("third").unwrap();
	let before = harness.text();

	let file = attach(&harness, &png(2), "shot.png");
	set_attachments(&harness, &target, vec![file.clone()]);
	let after = harness.text();

	let before_lines: Vec<&str> = before.lines().collect();
	// Every line the after-text does not share with the before-text has to belong
	// to the attachment that was added, and to nothing else in the document.
	let added: Vec<&str> = after
		.lines()
		.filter(|line| !before_lines.contains(line))
		.collect();
	assert!(!added.is_empty(), "nothing changed at all");
	for line in &added {
		let trimmed = line.trim();
		let belongs = trimmed.starts_with("\"attachments\"")
			|| trimmed.starts_with("\"id\"")
			|| trimmed.starts_with("\"file\"")
			|| trimmed.starts_with("\"name\"")
			|| trimmed.starts_with("\"mime\"")
			|| trimmed.starts_with("\"bytes\"")
			|| trimmed.starts_with("\"width\"")
			|| trimmed.starts_with("\"height\"")
			|| trimmed == "{"
			|| trimmed == "}"
			|| trimmed == "],";
		assert!(belongs, "a line outside the attachment changed: {line:?}");
	}
	// The other two notes survive verbatim, including the commas around them.
	for body in ["\"body\": \"first\",", "\"body\": \"third\","] {
		assert_eq!(
			before.matches(body).count(),
			after.matches(body).count(),
			"another note's body changed"
		);
	}
	assert!(after.contains(&file.file));
}

/// AC12, and the reason `sweep` exists at all: deleting a note must not delete
/// bytes, or undo restores a note whose attachments are gone.
#[test]
fn deleting_a_note_leaves_its_blobs_and_undo_restores_them() {
	let harness = Harness::new();
	let file = attach(&harness, &png(3), "shot.png");
	let id = harness.add("has a file").unwrap();
	set_attachments(&harness, &id, vec![file.clone()]);
	assert_eq!(blobs(&harness.path()), [file.file.clone()]);

	store::lock(&harness.shared)
		.mutate(|doc| ops::delete_notes(doc, &[id.clone()]))
		.unwrap();
	assert!(harness.doc().note(&id).is_none());
	assert_eq!(
		blobs(&harness.path()),
		[file.file.clone()],
		"deleting a note deleted its bytes, so the undo below cannot work"
	);

	let restored = store::lock(&harness.shared).undo().unwrap().unwrap();
	let note = restored.note(&id).expect("undo restored the note");
	assert_eq!(note.attachments, vec![file.clone()]);
	assert!(
		attachments::assets_dir(&harness.path()).join(&file.file).is_file(),
		"the restored note points at bytes that are not there"
	);
}

/// AC14. Survivor first, then the others in canonical order, de-duplicated by
/// the content hash — and all of it as one undoable step.
#[test]
fn merging_concatenates_attachments_survivor_first_and_deduplicates() {
	let harness = Harness::new();
	let shared_file = attach(&harness, &png(4), "shared.png");
	let only_first = attach(&harness, &png(5), "first.png");
	let only_second = attach(&harness, &png(6), "second.png");

	let first = harness.add("first note").unwrap();
	let second = harness.add("second note").unwrap();
	set_attachments(&harness, &first, vec![only_first.clone(), shared_file.clone()]);
	set_attachments(&harness, &second, vec![shared_file.clone(), only_second.clone()]);

	// Argument order deliberately reversed: the survivor is decided by document
	// order, not by which id was listed first.
	store::lock(&harness.shared)
		.mutate(|doc| ops::merge_notes(doc, &[second.clone(), first.clone()]))
		.unwrap();

	let merged = harness.doc();
	let survivor = merged.note(&first).expect("the first note survives a merge");
	assert_eq!(
		survivor
			.attachments
			.iter()
			.map(|attachment| attachment.file.as_str())
			.collect::<Vec<_>>(),
		[
			only_first.file.as_str(),
			shared_file.file.as_str(),
			only_second.file.as_str()
		],
		"the merged list is not survivor-first, or the shared file was duplicated"
	);
	assert!(merged.note(&second).is_none());

	// One undoable step: a single Ctrl+Z puts both notes back with their own
	// lists, rather than taking two presses.
	store::lock(&harness.shared).undo().unwrap().unwrap();
	let back = harness.doc();
	assert_eq!(back.note(&first).unwrap().attachments.len(), 2);
	assert_eq!(back.note(&second).unwrap().attachments.len(), 2);
}

/// AC21. Content addressing makes a write of identical bytes idempotent, so
/// concurrent ingests of the same file all succeed and leave one blob — the
/// `commit_new` collision is the success signal, not an error.
#[test]
fn concurrent_ingests_of_identical_bytes_all_succeed_and_write_one_file() {
	let harness = Harness::new();
	let path = harness.path();
	let bytes = png(7);

	// Four rather than a larger burst. The property is that concurrent writers
	// interleave and all succeed, which four demonstrate as well as forty — and
	// this file also holds tests that time a writer against the store's backoff
	// (`an_external_write_during_the_backoff_is_not_overwritten`), which a pile of
	// threads competing for the same cores can push outside its window.
	let stored: Vec<String> = std::thread::scope(|scope| {
		let handles: Vec<_> = (0..4)
			.map(|index| {
				let path = path.clone();
				let bytes = bytes.clone();
				scope.spawn(move || ingest(&path, &bytes, &format!("shot{index}.png")))
			})
			.collect();
		handles
			.into_iter()
			.map(|handle| handle.join().unwrap().expect("a racing ingest failed").file)
			.collect()
	});

	assert_eq!(stored.len(), 4);
	assert!(
		stored.windows(2).all(|pair| pair[0] == pair[1]),
		"identical bytes produced different names: {stored:?}"
	);
	assert_eq!(blobs(&path), [stored[0].clone()]);
}

/// The watch is registered on the space file's *directory*, and this task adds
/// a subdirectory the app writes into. The filename filter is what stops a blob
/// write from being read as an external document change — and a spurious reload
/// would clear the undo stack, which is exactly what AC12 depends on.
#[test]
fn writing_a_blob_does_not_look_like_an_external_document_change() {
	let harness = Harness::new();
	let id = harness.add("a note that stays put").unwrap();
	harness.sink.take();

	attach(&harness, &png(8), "shot.png");
	// Creating the directory itself is an event in the watched directory, so a
	// second write proves the steady state as well as the first.
	attach(&harness, &png(9), "another.png");
	settle();

	assert!(
		harness.sink.take().is_empty(),
		"a blob write was reported as an external change"
	);
	assert!(harness.status().can_undo, "a spurious reload cleared the undo stack");
	assert!(harness.doc().note(&id).is_some());
}

/// A `# Name` line and a pending attachment are a contradiction, and it is
/// refused rather than resolved silently in either direction — dropping the
/// files destroys work the tray still shows, and forcing a note hides the
/// section the user asked for.
#[test]
fn a_section_directive_carrying_attachments_is_refused() {
	let harness = Harness::new();
	let file = attach(&harness, &png(10), "shot.png");

	let err = submit(&harness.shared, "# Research", std::slice::from_ref(&file)).unwrap_err();

	assert_eq!(err.kind(), "invalid");
	assert!(harness.section_named("Research").is_none(), "a section was created anyway");
	assert!(
		attachments::assets_dir(&harness.path()).join(&file.file).is_file(),
		"a refused submission destroyed the bytes"
	);

	// The same attachment on an ordinary body goes through, so the refusal is
	// about the contradiction and not about attachments.
	let result = submit(&harness.shared, "with a file", std::slice::from_ref(&file)).unwrap();
	assert_eq!(result.outcome, SubmitOutcome::Note);
	let note = result.space.note(result.note_id.as_deref().unwrap()).unwrap();
	assert_eq!(note.attachments, vec![file]);
}

/// The document is hand-editable and this list made a round trip through IPC,
/// so `add_note` re-validates it rather than trusting that `ingest` minted it.
#[test]
fn submitting_an_attachment_with_an_escaping_file_name_is_refused() {
	let harness = Harness::new();
	let mut file = attach(&harness, &png(11), "shot.png");
	file.file = r"..\..\Windows\System32\config\SAM".into();

	let err = submit(&harness.shared, "a note", std::slice::from_ref(&file)).unwrap_err();

	assert_eq!(err.kind(), "invalid");
	assert!(harness.doc().notes.is_empty(), "the note was created anyway");
}

/// The per-note cap is enforced where the document is written, not only in the
/// UI that counts the tray.
#[test]
fn a_note_cannot_be_created_with_more_attachments_than_the_cap() {
	let harness = Harness::new();
	let files: Vec<Attachment> = (0..=attachments::ATTACHMENT_MAX_PER_NOTE)
		.map(|index| attach(&harness, &png(20 + index as u8), &format!("shot{index}.png")))
		.collect();

	let err = submit(&harness.shared, "too many", &files).unwrap_err();

	assert_eq!(err.kind(), "invalid");
	assert!(harness.doc().notes.is_empty());
	assert!(
		submit(&harness.shared, "just enough", &files[1..]).is_ok(),
		"the cap itself was refused"
	);
}

/// The MUST-FIX repro: ingest against space A, switch to B, submit.
///
/// The blob is in `A.copper.assets\`, so writing B's document with a reference
/// to it would leave a note pointing at a file that is not, and never will be,
/// beside it. The frontend clears the tray on a switch; this is the half that
/// does not depend on the frontend having done so.
#[test]
fn attachments_ingested_against_another_space_are_refused_after_a_switch() {
	let harness = Harness::new();
	let first = harness.path();
	let file = attach(&harness, &png(30), "shot.png");

	// A second space in the same directory, opened the way the switcher does.
	let second = first.with_file_name("other.copper");
	std::fs::write(&second, format::to_git_json(&golden_doc()).unwrap()).unwrap();
	store::open_space(&harness.shared, &second).unwrap();
	assert_eq!(harness.path(), second);

	let err = submit(&harness.shared, "a note", std::slice::from_ref(&file)).unwrap_err();

	assert_eq!(err.kind(), "invalid");
	assert!(err.message().contains("shot.png"), "{}", err.message());
	assert!(
		harness
			.doc()
			.notes
			.iter()
			.all(|note| note.attachments.is_empty()),
		"a dangling attachment reference reached the document"
	);
	// And the blob is untouched under the space it belongs to.
	assert!(attachments::assets_dir(&first).join(&file.file).is_file());

	// Back in its own space the same attachment goes through, so the refusal is
	// about *where* the blob is and not about the attachment.
	store::open_space(&harness.shared, &first).unwrap();
	assert!(submit(&harness.shared, "a note", std::slice::from_ref(&file)).is_ok());
}

/// A content-addressed name is not a capability. Sixteen hex characters is 64
/// bits, and — far more mundanely — the occupant could be a directory or a file
/// some other program left behind. Adopting it on the strength of the name
/// alone would let a note reference bytes nobody checked.
#[test]
fn a_colliding_name_holding_different_bytes_fails_the_ingest() {
	let harness = Harness::new();
	let space = harness.path();
	let bytes = png(31);
	let expected = ingest(&space, &bytes, "shot.png").unwrap();

	// Same name, different content — what a deliberate prefix collision produces.
	let occupied = attachments::assets_dir(&space).join(&expected.file);
	std::fs::write(&occupied, b"not the bytes that hash to this name").unwrap();

	let err = ingest(&space, &bytes, "shot.png").unwrap_err();
	assert_eq!(err.kind(), "io");
	assert!(err.message().contains("already exists"), "{}", err.message());

	// Identical bytes still succeed, so idempotent re-ingest is unaffected.
	std::fs::write(&occupied, &bytes).unwrap();
	assert_eq!(
		ingest(&space, &bytes, "shot.png").unwrap().file,
		expected.file
	);
}

/// A directory sitting where a blob should be is not a blob.
#[test]
fn a_directory_occupying_a_blob_name_fails_rather_than_being_adopted() {
	let harness = Harness::new();
	let space = harness.path();
	let bytes = png(32);
	let name = ingest(&space, &bytes, "shot.png").unwrap().file;

	let path = attachments::assets_dir(&space).join(&name);
	std::fs::remove_file(&path).unwrap();
	std::fs::create_dir(&path).unwrap();

	assert!(ingest(&space, &bytes, "shot.png").is_err());
	// And reading it refuses too, rather than following whatever it is.
	assert!(attachments::read_blob(&space, &name).is_err());
}

/// A `.copper` space can arrive from a git remote, and git can create symlinks.
/// A link inside the assets directory has a perfectly valid bare filename, so
/// the name check alone would let a read or a shell launch follow it out.
#[test]
#[cfg_attr(
	not(windows),
	ignore = "symlink creation needs Developer Mode or elevation"
)]
fn a_symlink_in_the_assets_directory_is_refused_rather_than_followed() {
	let harness = Harness::new();
	let space = harness.path();
	let name = ingest(&space, &png(33), "shot.png").unwrap().file;

	let outside = space.with_file_name("secret.txt");
	std::fs::write(&outside, b"not for the panel").unwrap();
	let link = attachments::assets_dir(&space).join("linked.png");
	// Creating a symlink needs Developer Mode or elevation; skip rather than fail
	// the suite on a machine that has neither, since the property is a property
	// of the code and not of this machine's privileges.
	if std::os::windows::fs::symlink_file(&outside, &link).is_err() {
		return;
	}

	let err = attachments::read_blob(&space, "linked.png").unwrap_err();
	assert_eq!(err.kind(), "invalid");
	// The real blob beside it still reads, so the refusal is about the link.
	assert!(attachments::read_blob(&space, &name).is_ok());
}

/// Ids are identity: two entries sharing one give the note two rows the
/// frontend keys identically, and make "remove this one" ambiguous.
#[test]
fn two_attachments_sharing_an_id_are_refused() {
	let harness = Harness::new();
	let one = attach(&harness, &png(34), "a.png");
	let mut two = attach(&harness, &png(35), "b.png");
	two.id = one.id.clone();

	let err = submit(&harness.shared, "a note", &[one.clone(), two]).unwrap_err();
	assert_eq!(err.kind(), "invalid");
	assert!(harness.doc().notes.is_empty());

	// Two entries pointing at the same *file* are fine and expected — that is
	// AC3, the same screenshot attached twice.
	let twice = Attachment {
		id: "att_second".into(),
		..one.clone()
	};
	assert!(submit(&harness.shared, "a note", &[one, twice]).is_ok());
}

/// `hex16` only ever emits lowercase, so two entries differing in case name the
/// same file on Windows — which a hand-edited document can easily contain.
#[test]
fn merging_deduplicates_attachment_files_case_insensitively() {
	let harness = Harness::new();
	let file = attach(&harness, &png(36), "shot.png");
	let shouty = Attachment {
		id: "att_shouty".into(),
		file: file.file.to_uppercase(),
		..file.clone()
	};

	let first = harness.add("first note").unwrap();
	let second = harness.add("second note").unwrap();
	set_attachments(&harness, &first, vec![file.clone()]);
	set_attachments(&harness, &second, vec![shouty]);

	store::lock(&harness.shared)
		.mutate(|doc| ops::merge_notes(doc, &[first.clone(), second.clone()]))
		.unwrap();

	assert_eq!(
		harness.doc().note(&first).unwrap().attachments.len(),
		1,
		"the same file in two cases survived as two entries"
	);
}
