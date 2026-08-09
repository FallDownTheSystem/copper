//! Filesystem, conflict, watcher and startup behaviour, against real temp dirs.
//!
//! These run without a Tauri runtime — there is no Tauri in this crate at all.
//! The store's core takes a `Weak` to itself and an `EventSink`, so `cargo test`
//! drives the whole of it. Emissions go to a `RecordingSink` and are counted
//! exactly — "emits exactly one `settings-changed`" is the kind of claim that
//! cannot be verified by reading a thin command wrapper (A9.37).
//!
//! The other half of these tests — everything reaching `store::commands::submit`,
//! `store::commands::add` or `attachments::ingest` — is
//! `src-tauri/tests/store_fs.rs`, because all three stayed with the app.
//!
//! Anything involving the watcher **polls**; it never sleeps a fixed time and
//! assumes. notify's timing on Windows depends on the backend and the machine.

use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use copper_core::store::error::StoreError;
use copper_core::store::events::{ChangeReason, EventSink, RecordingSink, StoreEvent};
use copper_core::store::model::{Note, Section, Space};
use copper_core::store::settings::{InsertionPoint, Settings};
use copper_core::store::{self, atomic, format, ops, settings, SharedStore, Store, StoreStatus};

/// Windows sharing modes, from `winnt.h`. Used to induce the transient failures
/// the write path is built to survive.
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
/// The watcher's debounce is 300 ms; two seconds is the bound A9.8 states.
const WATCH_TIMEOUT: Duration = Duration::from_secs(2);

// --- harness -----------------------------------------------------------------
struct Harness {
	_dir: tempfile::TempDir,
	config: PathBuf,
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
			config,
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

	fn settings(&self) -> Settings {
		store::lock(&self.shared).settings().clone()
	}

	fn on_disk_text(&self) -> String {
		store::lock(&self.shared).on_disk_text().unwrap().to_string()
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

	fn section_named(&self, name: &str) -> Option<String> {
		self.doc()
			.sections
			.iter()
			.find(|section| section.name == name)
			.map(|section| section.id.clone())
	}
}

fn wait_until(mut ready: impl FnMut() -> bool) -> bool {
	let deadline = Instant::now() + WATCH_TIMEOUT;
	while Instant::now() < deadline {
		if ready() {
			return true;
		}
		thread::sleep(Duration::from_millis(20));
	}
	ready()
}

/// Waits for at least `count` events, then returns everything recorded.
fn wait_for_events(sink: &RecordingSink, count: usize) -> Vec<StoreEvent> {
	wait_until(|| sink.events().len() >= count);
	sink.events()
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

fn body_set(doc: &Space) -> Vec<String> {
	let mut bodies: Vec<String> = doc.notes.iter().map(|note| note.body.clone()).collect();
	bodies.sort();
	bodies
}

// --- format: golden fixture (A9.1, A9.2) -------------------------------------
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

fn golden_text() -> String {
	// Read as bytes and converted by hand: `read_to_string` would be just as
	// literal, but this makes it obvious that no line-ending translation happens
	// anywhere in the comparison.
	let bytes = std::fs::read(Path::new("tests/fixtures/space-golden.copper")).unwrap();
	String::from_utf8(bytes).unwrap()
}

#[test]
fn a_known_document_serialises_to_the_committed_fixture() {
	assert_eq!(format::to_git_json(&golden_doc()).unwrap(), golden_text());
}

#[test]
fn the_fixture_round_trips_byte_identically() {
	let text = golden_text();
	let parsed = format::from_json(&text).unwrap();
	assert_eq!(format::to_git_json(&parsed).unwrap(), text);

	// And it is already canonical, so normalising changes nothing.
	let mut normalised = parsed.clone();
	format::normalise(&mut normalised);
	assert_eq!(normalised, parsed);
}

// --- first run and startup (A9.12, A9.13, A9.40) ------------------------------
#[test]
fn a_first_run_produces_settings_a_loadable_space_and_a_working_add_note() {
	let harness = Harness::new();

	assert!(harness.config.join("settings.json").is_file());
	assert!(harness.path().is_file());
	assert_eq!(harness.doc().sections.len(), 1);

	let id = harness.add("captured immediately").unwrap();

	let reloaded = format::from_json(&harness.text()).unwrap();
	assert_eq!(reloaded.notes.len(), 1);
	assert_eq!(reloaded.notes[0].id, id);
	assert_eq!(reloaded.notes[0].body, "captured immediately");
}

#[test]
fn startup_notices_accumulate_rather_than_overwrite() {
	let dir = tempfile::tempdir().unwrap();
	let config = dir.path().join("Copper");
	std::fs::create_dir_all(&config).unwrap();
	std::fs::write(config.join("settings.json"), "definitely not json").unwrap();

	let mut store = store::bootstrap_store(&config, Arc::new(RecordingSink::new())).unwrap();
	let from_settings = store
		.startup_notice()
		.expect("a corrupt settings file must record a notice")
		.to_string();

	// Phase 6 is explicitly permitted to add one for a failed cold launch.
	store.push_startup_notice("D:\\gone\\notes.copper could not be opened.");

	let combined = store.status().startup_notice.unwrap();
	assert!(combined.contains(&from_settings), "the first notice was lost");
	assert!(combined.contains("D:\\gone\\notes.copper"), "the second notice was lost");
	assert!(combined.contains('\n'), "notices were not joined: {combined}");
}

// --- recents (A9.17, A9.39) ---------------------------------------------------
#[test]
fn a_recents_path_never_carries_the_verbatim_prefix() {
	let harness = Harness::new();
	let spaces = harness.config.join("spaces");

	// Opened through a path containing `..`, which is exactly what makes
	// canonicalize emit `\\?\`.
	let indirect = spaces.join("..").join("spaces").join("personal.copper");
	store::open_space(&harness.shared, &indirect).unwrap();

	for entry in harness.settings().recents {
		assert!(!entry.starts_with(r"\\?\"), "verbatim prefix leaked: {entry}");
	}
	assert_eq!(harness.settings().recents.len(), 1, "the `..` form was stored twice");
}

#[test]
fn remove_recent_forgets_a_path_without_closing_the_space() {
	let harness = Harness::new();
	let open = harness.path();
	let other = harness.config.join("spaces").join("other.copper");
	store::create_space(&harness.shared, &other, "other").unwrap();
	// `other` is now open and first in recents; `personal` is second.
	assert_eq!(harness.settings().recents.len(), 2);
	harness.sink.take();

	// Forgetting the *open* space is bookkeeping, not a close.
	let event = store::lock(&harness.shared).remove_recent(&other).unwrap();
	assert!(matches!(event, StoreEvent::SettingsChanged(_)));
	assert!(
		harness.sink.events().is_empty(),
		"remove_recent emitted from inside the lock"
	);
	assert_eq!(store::lock(&harness.shared).active_path(), Some(other.as_path()));
	assert_eq!(harness.settings().recents.len(), 1);
	// The open space's path is gone from recents, so the index clamps.
	assert_eq!(harness.settings().active_space, 0);
	assert!(harness.add("still writable").is_ok());

	// Forgetting the other entry leaves the list ordered and re-points at nothing.
	store::lock(&harness.shared).remove_recent(&open).unwrap();
	assert!(harness.settings().recents.is_empty());

	// An absent path is a successful no-op.
	store::lock(&harness.shared)
		.remove_recent(Path::new("D:\\never\\here.copper"))
		.unwrap();
}

#[test]
fn remove_recent_repoints_active_space_at_the_still_open_path() {
	let harness = Harness::new();
	let personal = harness.path();
	let other = harness.config.join("spaces").join("other.copper");
	store::create_space(&harness.shared, &other, "other").unwrap();

	// `other` is open and at index 0; forgetting `personal` shifts it.
	store::lock(&harness.shared).remove_recent(&personal).unwrap();

	let settings = harness.settings();
	assert_eq!(settings.recents.len(), 1);
	assert_eq!(
		settings.recents[settings.active_space],
		store::path_string(&other)
	);
}

// --- opening (A9.16, A9.30, A9.31) --------------------------------------------
#[test]
fn a_failed_open_leaves_the_previous_space_open_watched_and_unchanged() {
	let harness = Harness::new();
	harness.add("original note").unwrap();
	let path = harness.path();
	let bytes = std::fs::read(&path).unwrap();
	harness.sink.take();

	let bad_json = harness.config.join("spaces").join("broken.copper");
	std::fs::write(&bad_json, "<<<<<<< HEAD\n{ }\n").unwrap();
	let a_directory = harness.config.join("spaces");

	for candidate in [
		harness.config.join("spaces").join("no-such-file.copper"),
		bad_json,
		a_directory,
	] {
		let err = store::open_space(&harness.shared, &candidate).unwrap_err();
		assert!(
			matches!(err.kind(), "not-found" | "parse" | "invalid"),
			"unexpected kind {} for {}",
			err.kind(),
			candidate.display()
		);
	}

	assert_eq!(harness.path(), path);
	assert_eq!(std::fs::read(&path).unwrap(), bytes);
	assert!(harness.status().watching, "the previous space lost its watch");
	assert!(harness.sink.events().is_empty(), "a failed open emitted");

	// The watch is not merely reported as live — it still works.
	let mut changed = format::from_json(&harness.text()).unwrap();
	changed.name = "renamed outside".into();
	external_write(&path, &format::to_git_json(&changed).unwrap());
	assert!(wait_until(|| !harness.sink.events().is_empty()));
}

#[test]
fn create_space_refuses_to_clobber_and_emits_one_settings_changed() {
	let harness = Harness::new();
	let target = harness.config.join("spaces").join("project.copper");
	harness.sink.take();

	let created = store::create_space(&harness.shared, &target, "project").unwrap();
	assert_eq!(created.name, "project");
	assert_eq!(created.sections.len(), 1);
	assert_eq!(
		harness.sink.take().len(),
		1,
		"create_space must emit exactly one settings-changed"
	);

	let bytes = std::fs::read(&target).unwrap();
	let err = store::create_space(&harness.shared, &target, "project").unwrap_err();
	assert_eq!(err.kind(), "invalid");
	assert_eq!(std::fs::read(&target).unwrap(), bytes, "the existing file was rewritten");
	assert!(harness.sink.events().is_empty(), "a failed create emitted");
}

#[test]
fn a_document_with_duplicate_ids_is_refused_at_load_and_left_alone() {
	let harness = Harness::new();
	let duplicates = harness.config.join("spaces").join("duplicates.copper");

	for (field, id) in [("note", "nte_01000001"), ("section", "sec_a1000001")] {
		let mut doc = golden_doc();
		if field == "note" {
			doc.notes[1].id = id.into();
		} else {
			doc.sections[1].id = id.into();
		}
		let text = format::to_git_json(&doc).unwrap();
		std::fs::write(&duplicates, &text).unwrap();

		let err = store::open_space(&harness.shared, &duplicates).unwrap_err();
		assert_eq!(err.kind(), "parse");
		assert!(err.message().contains(id), "the offending id is not named: {}", err.message());
		assert_eq!(std::fs::read_to_string(&duplicates).unwrap(), text);
	}
}

// --- undo (A9.10, A9.24) ------------------------------------------------------
#[test]
fn every_structural_operation_leaves_an_undo_that_restores_exactly() {
	let harness = Harness::new();
	let first = harness.add("alpha").unwrap();
	let second = harness.add("beta").unwrap();
	let section = harness.doc().active_section.clone();

	// One structural operation, boxed so a list of them can be iterated.
	type Operation = Box<dyn Fn(&mut Space) -> Result<(), StoreError>>;

	let operations: Vec<(&str, Operation)> = vec![
		("add", {
			Box::new(|doc: &mut Space| ops::add_note(doc, "gamma", None, &[], InsertionPoint::Bottom).map(|_| ()))
		}),
		("done", {
			let ids = vec![first.clone()];
			Box::new(move |doc: &mut Space| ops::set_notes_done(doc, &ids, true))
		}),
		("reorder", {
			let id = first.clone();
			let section = section.clone();
			Box::new(move |doc: &mut Space| ops::reorder_note(doc, &id, &section, 1))
		}),
		("add section", {
			Box::new(|doc: &mut Space| ops::add_section(doc, "Later").map(|_| ()))
		}),
		("rename section", {
			let section = section.clone();
			Box::new(move |doc: &mut Space| ops::rename_section(doc, &section, "Renamed"))
		}),
		("reorder section", {
			let section = section.clone();
			Box::new(move |doc: &mut Space| ops::reorder_section(doc, &section, 1))
		}),
		("move", {
			let ids = vec![second.clone()];
			Box::new(move |doc: &mut Space| {
				// The first section, not the last: by this point the note already
				// sits at the end of the last one, so moving it there would be a
				// no-op and the test would assert nothing.
				let target = doc.sections.first().unwrap().id.clone();
				ops::move_notes(doc, &ids, &target)
			})
		}),
		("merge", {
			Box::new(|doc: &mut Space| {
				let ids: Vec<String> = doc.notes.iter().take(2).map(|n| n.id.clone()).collect();
				ops::merge_notes(doc, &ids)
			})
		}),
		("delete section", {
			Box::new(|doc: &mut Space| {
				let id = doc.sections.last().unwrap().id.clone();
				ops::delete_section(doc, &id)
			})
		}),
		("delete", {
			Box::new(|doc: &mut Space| {
				let ids: Vec<String> = doc.notes.iter().map(|n| n.id.clone()).collect();
				ops::delete_notes(doc, &ids)
			})
		}),
	];

	// The operations accumulate rather than being rolled back between rounds:
	// several of them need a document the earlier ones built (a second section to
	// reorder, notes in two sections to move between), and undoing each round
	// would leave every later operation a no-op with nothing to assert.
	for (name, operation) in operations {
		let before = harness.doc();
		let before_text = harness.text();

		store::lock(&harness.shared).mutate(operation).unwrap();
		assert_ne!(harness.doc(), before, "{name} changed nothing to undo");
		assert!(harness.status().can_undo);
		assert!(!harness.status().can_redo, "{name} did not clear redo");

		let restored = store::lock(&harness.shared).undo().unwrap().unwrap();
		assert_eq!(restored, before, "undoing {name} did not restore the document");
		assert_eq!(harness.text(), before_text, "undoing {name} did not restore the file");

		let redone = store::lock(&harness.shared).redo().unwrap().unwrap();
		assert_ne!(redone, before, "redoing {name} did nothing");
		assert_eq!(harness.doc(), redone);
	}
}

#[test]
fn text_edits_and_navigation_do_not_push_snapshots() {
	let harness = Harness::new();
	let id = harness.add("alpha").unwrap();
	let section = harness.doc().active_section.clone();
	// One structural op so the stack is non-empty and a stray push would show.
	harness.add("beta").unwrap();
	let baseline = harness.doc();

	store::lock(&harness.shared)
		.mutate_no_snapshot(|doc| ops::edit_note(doc, &id, "alpha, revised"))
		.unwrap();
	store::lock(&harness.shared)
		.mutate_no_snapshot(|doc| ops::set_active_section(doc, &section))
		.unwrap();

	// The one snapshot on the stack is still the pre-`beta` document, not the
	// pre-edit one.
	let restored = store::lock(&harness.shared).undo().unwrap().unwrap();
	assert_eq!(restored.notes.len(), 1, "an edit pushed a snapshot");
	assert_ne!(baseline, restored);
}

#[test]
fn sixty_operations_leave_a_fifty_deep_stack() {
	let harness = Harness::new();
	for index in 0..60 {
		harness.add(&format!("note {index}")).unwrap();
	}

	let mut restored = 0;
	while store::lock(&harness.shared).undo().unwrap().is_some() {
		restored += 1;
		assert!(restored <= 60, "the stack is not capped");
	}
	assert_eq!(restored, 50);
	// Fifty undos took it back to ten notes, not to zero: the oldest ten
	// snapshots were dropped.
	assert_eq!(harness.doc().notes.len(), 10);
}

/// Task-016 AC7/AC8. "Delete all done" is one undoable step, and the claim has
/// to be proved against the stack's **depth** rather than against the document.
///
/// Restoring every note is what a loop of single deletes would do too — it would
/// just take one press per note to do it, which is exactly the outcome the batch
/// discipline exists to prevent. So this counts the entries the operation left
/// behind, by draining the stack the way the cap test above does.
#[test]
fn deleting_every_done_note_is_one_undoable_step() {
	let harness = Harness::new();

	let ids: Vec<String> = ["alpha", "beta", "gamma", "delta", "epsilon"]
		.iter()
		.map(|body| harness.add(body).unwrap())
		.collect();

	// Three of the five, deliberately not contiguous: a restore that merely
	// re-appended them would pass a contiguous case and fail this one.
	let done = vec![ids[0].clone(), ids[2].clone(), ids[4].clone()];
	store::lock(&harness.shared)
		.mutate(|doc| ops::set_notes_done(doc, &done, true))
		.unwrap();

	// Five adds and one mark-done, each one snapshot.
	let depth_before = 6;
	let before = harness.doc();

	store::lock(&harness.shared)
		.mutate(|doc| ops::delete_notes(doc, &done))
		.unwrap();
	assert_eq!(harness.doc().notes.len(), 2, "the bulk delete left a done note behind");

	// One press restores the whole pre-delete document: the notes, their
	// `done: true`, and the positions they held between the two survivors.
	let restored = store::lock(&harness.shared).undo().unwrap().unwrap();
	assert_eq!(restored, before, "one undo did not restore the document");
	assert_eq!(
		restored.notes.iter().filter(|note| note.done).count(),
		3,
		"the notes came back without their done state"
	);

	// And the stack is exactly where it was before the delete, so the delete
	// contributed one entry rather than one per note.
	let mut remaining = 0;
	while store::lock(&harness.shared).undo().unwrap().is_some() {
		remaining += 1;
		assert!(remaining <= depth_before, "the bulk delete pushed more than one snapshot");
	}
	assert_eq!(remaining, depth_before);
}

// --- submit_entry (task-010) --------------------------------------------------
#[test]
fn a_capture_never_honours_a_directive() {
	let harness = Harness::new();
	let sections = harness.doc().sections.len();

	// Open Question 1, answered 2026-08-05: the capture path saves `# Name` as an
	// ordinary note, exactly like any other capture. This is the assertion that
	// keeps a future refactor from routing capture through `submit`.
	let id = store::append_capture(&harness.shared, "# Research").unwrap().note;

	assert_eq!(harness.doc().note(&id).unwrap().body, "# Research");
	assert_eq!(harness.doc().sections.len(), sections);
	assert!(harness.section_named("Research").is_none());
}

#[test]
fn an_empty_stack_returns_null_rather_than_erroring() {
	let harness = Harness::new();
	assert!(store::lock(&harness.shared).undo().unwrap().is_none());
	assert!(store::lock(&harness.shared).redo().unwrap().is_none());
}

#[test]
fn a_failed_operation_writes_nothing_and_pushes_no_snapshot() {
	let harness = Harness::new();
	harness.add("alpha").unwrap();
	let before = harness.doc();
	let before_text = harness.text();
	let can_undo = harness.status().can_undo;

	let err = harness.add("   ").unwrap_err();
	assert_eq!(err.kind(), "invalid");

	assert_eq!(harness.doc(), before);
	assert_eq!(harness.text(), before_text);
	assert_eq!(harness.status().can_undo, can_undo);
	assert!(harness.sink.events().is_empty(), "a failed mutation emitted");
}

// --- conflict (A9.6, A9.20, A9.21, A9.22, A9.23, A9.24, A9.7) -----------------
/// Rewrites the file behind the store's back, returning the text now on disk.
fn external_note(harness: &Harness, body: &str) -> String {
	let mut doc = format::from_json(&harness.text()).unwrap();
	let section = doc.active_section.clone();
	ops::add_note(&mut doc, body, Some(&section), &[], InsertionPoint::Bottom).unwrap();
	let text = format::to_git_json(&doc).unwrap();
	external_write(&harness.path(), &text);
	text
}

#[test]
fn a_conflicting_write_keeps_both_changes() {
	let harness = Harness::new();
	harness.add("ours, first").unwrap();
	external_note(&harness, "theirs");

	harness.add("ours, second").unwrap();

	let on_disk = format::from_json(&harness.text()).unwrap();
	assert_eq!(
		body_set(&on_disk),
		["ours, first", "ours, second", "theirs"],
		"the conflict path lost a change"
	);
	assert_eq!(harness.doc(), on_disk);
	assert_eq!(harness.on_disk_text(), harness.text());
}

/// A9.20. Without this, every other conflict test still passes while `Ctrl+Z`
/// silently reverts someone else's change.
#[test]
fn undo_after_a_conflict_restores_the_external_document() {
	let harness = Harness::new();
	harness.add("ours, first").unwrap();
	let external_text = external_note(&harness, "theirs");

	harness.add("ours, second").unwrap();
	let restored = store::lock(&harness.shared).undo().unwrap().unwrap();

	assert_eq!(
		body_set(&restored),
		["ours, first", "theirs"],
		"undo reverted the external change instead of only our own"
	);
	assert_eq!(harness.text(), external_text);
}

/// The second half of A9.20, and the one that was missing.
///
/// One undo after a conflict correctly reverts only our own change. The *second*
/// undo used to reach a snapshot taken before the external write and restore it,
/// destroying somebody else's note — and the watcher could not rescue it,
/// because the merged document is what we just wrote and the reload is
/// suppressed as a self-write, so spec 4.6's clear never ran.
#[test]
fn undoing_past_a_conflict_cannot_reach_a_pre_external_document() {
	let harness = Harness::new();
	harness.add("ours, first").unwrap();
	harness.add("ours, second").unwrap();
	external_note(&harness, "theirs");

	harness.add("ours, third").unwrap();

	// One undo: our own change goes, the external one stays.
	let restored = store::lock(&harness.shared).undo().unwrap().unwrap();
	assert_eq!(
		body_set(&restored),
		["ours, first", "ours, second", "theirs"],
		"the first undo did not revert exactly our own change"
	);

	// There is nothing left to undo into: every older snapshot predates the
	// external change.
	assert!(
		!harness.status().can_undo,
		"a pre-external snapshot survived the rebase and can still be undone into"
	);
	assert!(store::lock(&harness.shared).undo().unwrap().is_none());

	let on_disk = format::from_json(&harness.text()).unwrap();
	assert!(
		on_disk.notes.iter().any(|note| note.body == "theirs"),
		"undoing past the conflict destroyed the external change"
	);
}

/// The same defect one step worse: a no-snapshot mutation pushes nothing, so
/// after a conflict the *first* undo reached a pre-external document.
#[test]
fn undoing_after_a_conflicting_no_snapshot_mutation_cannot_destroy_it() {
	let harness = Harness::new();
	let id = harness.add("ours, first").unwrap();
	harness.add("ours, second").unwrap();
	external_note(&harness, "theirs");

	// `edit_note` takes no snapshot, so nothing is pushed to offset the rebase.
	store::lock(&harness.shared)
		.mutate_no_snapshot(|doc| ops::edit_note(doc, &id, "ours, first, revised"))
		.unwrap();

	assert!(
		!harness.status().can_undo,
		"a pre-external snapshot survived a no-snapshot rebase"
	);
	assert!(store::lock(&harness.shared).undo().unwrap().is_none());

	let on_disk = format::from_json(&harness.text()).unwrap();
	assert_eq!(
		body_set(&on_disk),
		["ours, first, revised", "ours, second", "theirs"],
		"the edit did not merge with the external change"
	);
}

/// The gap between reading a document and the watch going live produces no
/// event, so a write landing in it would sit unnoticed until the next change —
/// which, for a file nobody touches again, is forever.
#[test]
fn a_write_landing_before_the_watch_registers_is_reconciled() {
	let dir = tempfile::tempdir().unwrap();
	let config = dir.path().join("Copper");
	let sink = Arc::new(RecordingSink::new());
	let store = store::bootstrap_store(&config, sink.clone()).unwrap();
	let shared: SharedStore = Arc::new(Mutex::new(store));

	// Exactly the bootstrap-to-attach window: the document has been read, and
	// nothing is watching yet.
	let path = store::lock(&shared).active_path().unwrap().to_path_buf();
	let mut doc = format::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
	let section = doc.active_section.clone();
	ops::add_note(&mut doc, "written into the gap", Some(&section), &[], InsertionPoint::Bottom).unwrap();
	external_write(&path, &format::to_git_json(&doc).unwrap());

	let produced = store::attach_watcher(&shared);

	let held = store::lock(&shared).active_space().unwrap();
	assert_eq!(
		body_set(&held),
		["written into the gap"],
		"the write that landed before the watch went live was never noticed"
	);
	assert_eq!(reasons(&produced), [ChangeReason::External]);
	assert!(store::lock(&shared).status().watching);
}

#[test]
fn a_conflict_against_a_non_canonical_document_re_applies_correctly() {
	let harness = Harness::new();
	harness.add("first").unwrap();
	harness.add("second").unwrap();

	// Shuffled orders: an operation applied without normalising first would
	// target the wrong position.
	let mut doc = format::from_json(&harness.text()).unwrap();
	doc.notes[0].order = 40;
	doc.notes[1].order = 3;
	doc.sections[0].order = 7;
	external_write(&harness.path(), &format::to_git_json(&doc).unwrap());

	harness.add("third").unwrap();

	let on_disk = format::from_json(&harness.text()).unwrap();
	assert_eq!(
		on_disk.notes.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(),
		["second", "first", "third"],
		"the fresh document was not normalised before the op was re-applied"
	);
	for (index, note) in on_disk.notes.iter().enumerate() {
		assert_eq!(note.order, index as i64);
	}
}

#[test]
fn a_conflict_against_an_empty_sections_array_re_applies_correctly() {
	let harness = Harness::new();
	harness.add("existing").unwrap();

	let mut doc = format::from_json(&harness.text()).unwrap();
	doc.sections.clear();
	external_write(&harness.path(), &format::to_git_json(&doc).unwrap());

	harness.add("added after the hand edit").unwrap();

	let on_disk = format::from_json(&harness.text()).unwrap();
	assert_eq!(on_disk.sections.len(), 1);
	assert_eq!(
		body_set(&on_disk),
		["added after the hand edit", "existing"],
		"the hand-emptied sections array cost a note"
	);
}

/// A9.22. The comparison is repeated before every commit attempt, so an
/// external write landing during the sharing-violation backoff is not
/// overwritten.
#[test]
fn an_external_write_during_the_backoff_is_not_overwritten() {
	let harness = Harness::new();
	harness.add("ours, first").unwrap();
	let path = harness.path();

	// Readable but not renameable: the store's read succeeds and its rename hits
	// ERROR_SHARING_VIOLATION, which is exactly the window spec 2.2a is about.
	let lock = OpenOptions::new()
		.read(true)
		.share_mode(FILE_SHARE_READ)
		.open(&path)
		.unwrap();

	let writer_path = path.clone();
	let writer = thread::spawn(move || {
		thread::sleep(Duration::from_millis(60));
		drop(lock);
		let mut doc = format::from_json(&std::fs::read_to_string(&writer_path).unwrap()).unwrap();
		let section = doc.active_section.clone();
		ops::add_note(
			&mut doc,
			"theirs, during the backoff",
			Some(&section),
			&[],
			InsertionPoint::Bottom,
		).unwrap();
		external_write(&writer_path, &format::to_git_json(&doc).unwrap());
	});

	harness.add("ours, second").unwrap();
	writer.join().unwrap();

	let on_disk = format::from_json(&harness.text()).unwrap();
	assert_eq!(
		body_set(&on_disk),
		["ours, first", "ours, second", "theirs, during the backoff"],
		"a blind retry overwrote the external write"
	);
}

/// A9.23. Three exhausted attempts must leave everything mutually coherent.
///
/// The interfering writer is driven from the operation closure rather than from
/// a racing thread, which makes the interleaving exact instead of hoped for:
/// `op` runs at the top of every attempt, before the file is read, so each
/// attempt is guaranteed to find content it has not seen before and to conflict.
/// A background writer only reproduces this some of the time, and a test that
/// asserts the exhaustion path has to actually reach it every run.
#[test]
fn three_exhausted_attempts_change_nothing() {
	let harness = Harness::new();
	harness.add("original").unwrap();

	let before_doc = harness.doc();
	let before_text = harness.on_disk_text();
	let before_undo = harness.status().can_undo;

	let generation = AtomicUsize::new(0);
	let base = before_doc.clone();
	let write_path = harness.path();
	let result = store::lock(&harness.shared).mutate(move |doc| {
		let mut external = base.clone();
		external.name = format!("generation {}", generation.fetch_add(1, Ordering::SeqCst));
		external_write(&write_path, &format::to_git_json(&external).unwrap());
		ops::add_note(doc, "should never land", None, &[], InsertionPoint::Bottom).map(|_| ())
	});

	let err = result.unwrap_err();
	assert_eq!(err.kind(), "conflict", "{}", err.message());

	assert_eq!(harness.doc(), before_doc, "the in-memory document moved");
	assert_eq!(harness.on_disk_text(), before_text, "on_disk_text moved");
	assert_eq!(harness.status().can_undo, before_undo, "the undo stack moved");
	assert!(!harness.status().can_redo);

	// The file holds the last external write and none of ours.
	let on_disk = format::from_json(&harness.text()).unwrap();
	assert_eq!(on_disk.name, "generation 2", "the attempt count is not three");
	assert!(
		!on_disk.notes.iter().any(|note| note.body == "should never land"),
		"an exhausted conflict still wrote"
	);
}

#[test]
fn a_mutation_against_unparseable_content_fails_without_writing() {
	let harness = Harness::new();
	harness.add("keep me").unwrap();
	let path = harness.path();
	let poison = "<<<<<<< HEAD\n{ \"id\": \"spc_1\" }\n";
	external_write(&path, poison);
	let before = harness.doc();

	let started = Instant::now();
	let err = harness.add("should not land").unwrap_err();

	assert_eq!(err.kind(), "parse", "{}", err.message());
	assert_eq!(
		std::fs::read_to_string(&path).unwrap(),
		poison,
		"the store overwrote a file it could not parse"
	);
	assert_eq!(harness.doc(), before, "the in-memory document was discarded");
	// It retried rather than failing on first sight (spec 2.3b).
	assert!(started.elapsed() >= Duration::from_millis(400), "the read was not retried");
}

#[test]
fn a_failed_undo_leaves_both_stacks_and_the_document_untouched() {
	let harness = Harness::new();
	harness.add("first").unwrap();
	harness.add("second").unwrap();
	let before = harness.doc();
	let before_text = harness.on_disk_text();

	// A whole-document restore cannot merge, so a changed file must fail the undo
	// rather than clobber it (spec 4.8).
	let external_text = external_note(&harness, "theirs");

	let err = store::lock(&harness.shared).undo().unwrap_err();

	assert_eq!(err.kind(), "conflict");
	assert_eq!(harness.doc(), before);
	assert_eq!(harness.on_disk_text(), before_text);
	assert_eq!(harness.text(), external_text, "a failed undo still wrote");
	assert!(harness.status().can_undo, "the undo stack was consumed by a failure");
	assert!(!harness.status().can_redo);
}

// --- watching (A9.8, A9.9, A9.25, A9.26, A9.27, A9.28, A9.29) ------------------
#[test]
fn an_external_rewrite_produces_exactly_one_space_changed() {
	let harness = Harness::new();
	harness.add("before").unwrap();
	harness.sink.take();

	external_note(&harness, "added outside Copper");

	let events = wait_for_events(&harness.sink, 1);
	assert_eq!(events.len(), 1, "{:?}", harness.sink.names());
	assert_eq!(reasons(&events), [ChangeReason::External]);
	assert_eq!(
		body_set(&harness.doc()),
		["added outside Copper", "before"],
		"the in-memory document was not replaced"
	);
	// An external reload clears both stacks (spec 4.6).
	assert!(!harness.status().can_undo);
	assert!(!harness.status().can_redo);

	settle();
	assert_eq!(harness.sink.events().len(), 1, "a second event arrived late");
}

#[test]
fn the_stores_own_write_produces_no_events() {
	let harness = Harness::new();
	harness.sink.take();

	for index in 0..5 {
		harness.add(&format!("note {index}")).unwrap();
	}

	settle();
	assert!(
		harness.sink.events().is_empty(),
		"self-write suppression failed: {:?}",
		harness.sink.names()
	);
}

/// A9.9. Same document, different bytes — no UI churn for a semantic no-op.
#[test]
fn a_byte_different_but_semantically_identical_rewrite_is_silent() {
	let harness = Harness::new();
	harness.add("alpha").unwrap();
	harness.add("beta").unwrap();
	harness.sink.take();
	let before = harness.doc();

	// Shuffled array order and non-contiguous `order` values, which normalise to
	// exactly the document already in memory.
	let mut doc = format::from_json(&harness.text()).unwrap();
	doc.notes.reverse();
	doc.notes[0].order = 90;
	doc.notes[1].order = 20;
	let rewritten = format::to_git_json(&doc).unwrap();
	assert_ne!(rewritten, harness.text());
	external_write(&harness.path(), &rewritten);

	settle();
	assert!(
		harness.sink.events().is_empty(),
		"a semantic no-op emitted: {:?}",
		harness.sink.names()
	);
	assert_eq!(harness.doc(), before);
	// `on_disk_text` still tracked the change, so the next write does not conflict.
	assert_eq!(harness.on_disk_text(), rewritten);
	assert!(harness.add("gamma").is_ok());
}

/// A9.25. The 3.3a trap: an implementation that checks byte equality first stays
/// errored forever and passes every other watcher test.
#[test]
fn recovery_from_a_byte_identical_restore_clears_the_error() {
	let harness = Harness::new();
	harness.add("alpha").unwrap();
	let path = harness.path();
	let good = harness.text();
	harness.sink.take();

	external_write(&path, "{ not a document");
	assert!(wait_until(|| harness.status().errored), "the space never went errored");
	// Invalidation announces itself once and says nothing else.
	let names = harness.sink.names();
	assert!(!names.is_empty(), "going errored emitted nothing");
	assert!(
		names.iter().all(|name| *name == "store-error"),
		"invalidation emitted more than a store-error: {names:?}"
	);
	// The in-memory document survives, and mutations are refused.
	assert_eq!(harness.doc().notes.len(), 1);
	assert_eq!(harness.add("blocked").unwrap_err().kind(), "unavailable");
	harness.sink.take();

	// Byte-for-byte the original content.
	external_write(&path, &good);

	assert!(wait_until(|| !harness.status().errored), "the space never recovered");
	let events = wait_for_events(&harness.sink, 1);
	assert_eq!(
		reasons(&events),
		[ChangeReason::Reload],
		"recovery must announce itself: {:?}",
		harness.sink.names()
	);
	assert!(harness.add("writable again").is_ok());
}

#[test]
fn a_deleted_then_restored_file_follows_the_same_recovery_path() {
	let harness = Harness::new();
	harness.add("alpha").unwrap();
	let path = harness.path();
	let good = harness.text();
	harness.sink.take();

	std::fs::remove_file(&path).unwrap();
	assert!(wait_until(|| harness.status().errored), "a deleted file did not error");
	assert_eq!(harness.doc().notes.len(), 1, "the document was discarded");
	harness.sink.take();

	external_write(&path, &good);
	assert!(wait_until(|| !harness.status().errored), "a restored file did not recover");
	assert!(harness.add("writable again").is_ok());
}

#[test]
fn invalid_utf8_errors_and_then_recovers_when_fixed() {
	let harness = Harness::new();
	harness.add("alpha").unwrap();
	let path = harness.path();
	let good = harness.text();
	harness.sink.take();

	std::fs::write(&path, [0x7b, 0xff, 0xfe, 0x22]).unwrap();
	assert!(wait_until(|| harness.status().errored), "invalid UTF-8 did not error");
	assert_eq!(harness.doc().notes.len(), 1);

	external_write(&path, &good);
	assert!(wait_until(|| !harness.status().errored));
	assert_eq!(harness.doc().notes.len(), 1);
}

/// A9.27. Dropping a debouncer signals its worker but does not join it, so a
/// callback queued for the previous space can still run.
#[test]
fn a_callback_for_a_closed_space_is_inert() {
	let harness = Harness::new();
	let stale_path = harness.path();
	harness.add("in the first space").unwrap();

	let other = harness.config.join("spaces").join("other.copper");
	store::create_space(&harness.shared, &other, "other").unwrap();
	harness.add("in the second space").unwrap();
	let before = harness.doc();
	let before_text = harness.on_disk_text();
	harness.sink.take();

	// Deliver the stale callback by hand.
	copper_core::store::watch::handle_external_change(&harness.shared, &stale_path);

	assert_eq!(harness.doc(), before, "a stale callback touched the new space");
	assert_eq!(harness.on_disk_text(), before_text);
	assert!(!harness.status().errored);
	assert!(harness.sink.events().is_empty(), "a stale callback emitted");
}

/// A9.28. A single conflated error field fails this: the space would be
/// read-only, which is exactly what spec 3.7 exists to prevent.
#[test]
fn a_space_whose_watch_failed_is_still_writable() {
	let harness = Harness::new();

	// A directory the store is not already watching — the open space's own
	// directory has a live notify handle on it, which would make the exclusive
	// open below fail instead of the watch.
	let unwatchable = harness.config.join("unwatchable");
	std::fs::create_dir_all(&unwatchable).unwrap();
	let path = unwatchable.join("locked.copper");
	std::fs::write(&path, format::to_git_json(&golden_doc()).unwrap()).unwrap();

	// Hold the directory itself open with no sharing, so notify's own open of it
	// fails. This is only the device for reaching the state under test: Windows
	// routes file creation inside a directory through the directory object too,
	// so the lock is released before the write below.
	let blocked = OpenOptions::new()
		.read(true)
		.share_mode(0)
		.custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
		.open(&unwatchable)
		.unwrap();
	harness.sink.take();

	// The open succeeds: a watch that will not register is not a reason to refuse
	// a perfectly readable space.
	store::open_space(&harness.shared, &path).unwrap();

	let status = harness.status();
	assert_eq!(status.path, Some(store::path_string(&path)));
	assert!(!status.watching, "a failed watch reported as watching");
	assert!(!status.errored, "a failed watch was reported as an unreadable document");

	// Outside startup the failure is reported, alongside the usual recents change.
	assert_eq!(harness.sink.take_names(), ["settings-changed", "store-error"]);

	drop(blocked);
	// The obstruction is gone; the recorded failure is not. `watching` reports the
	// registration outcome and nothing re-registers on its own (spec 3.9), so the
	// space is still in the watch-failed state for the write below.
	assert!(!harness.status().watching, "the watch state healed by itself");

	// The point of keeping the two error fields apart: this still works. A single
	// conflated `error` field would have made this space read-only.
	assert!(harness.add("written without a watch").is_ok());
	assert_eq!(harness.doc().notes.len(), 3);
}

/// A9.29. Proves the guard is released before the emit (spec 2.10): a listener
/// that locks the store would otherwise hang the emitting thread forever.
#[test]
fn a_listener_that_locks_the_store_does_not_deadlock() {
	struct ReentrantSink {
		store: Mutex<Option<std::sync::Weak<Mutex<Store>>>>,
		seen: Mutex<usize>,
	}

	impl EventSink for ReentrantSink {
		fn emit(&self, _event: &StoreEvent) {
			let weak = self.store.lock().unwrap().clone();
			if let Some(shared) = weak.and_then(|weak| weak.upgrade()) {
				// The call this test exists for. If the store guard were still held
				// by the emitting thread this would never return.
				let _ = store::lock(&shared).status();
				*self.seen.lock().unwrap() += 1;
			}
		}
	}

	let dir = tempfile::tempdir().unwrap();
	let sink = Arc::new(ReentrantSink {
		store: Mutex::new(None),
		seen: Mutex::new(0),
	});
	let store = store::bootstrap_store(&dir.path().join("Copper"), sink.clone()).unwrap();
	let shared: SharedStore = Arc::new(Mutex::new(store));
	*sink.store.lock().unwrap() = Some(Arc::downgrade(&shared));

	// Any emit will do; `append_capture` is the one that emits from the same
	// thread that just held the guard.
	let (sender, receiver) = std::sync::mpsc::channel();
	let worker = {
		let shared = Arc::clone(&shared);
		thread::spawn(move || {
			let result = store::append_capture(&shared, "captured");
			sender.send(result.is_ok()).unwrap();
		})
	};

	let finished = receiver
		.recv_timeout(Duration::from_secs(5))
		.expect("emitting under the store guard deadlocked");
	assert!(finished);
	worker.join().unwrap();
	assert_eq!(*sink.seen.lock().unwrap(), 1);
}

// --- events and status (A9.18, A9.36) -----------------------------------------
#[test]
fn append_capture_emits_exactly_one_space_changed_with_reason_capture() {
	let harness = Harness::new();
	harness.sink.take();

	let id = store::append_capture(&harness.shared, "captured from the hook").unwrap().note;

	let events = harness.sink.take();
	assert_eq!(events.len(), 1);
	assert_eq!(reasons(&events), [ChangeReason::Capture]);
	assert!(harness.doc().note(&id).is_some());
	assert!(harness.status().can_undo, "a capture must be undoable");

	settle();
	assert!(
		harness.sink.events().is_empty(),
		"the capture's own write produced a watcher event: {:?}",
		harness.sink.names()
	);
}

#[test]
fn append_capture_carries_the_notification_setting_it_was_written_under() {
	let harness = Harness::new();
	assert!(
		store::append_capture(&harness.shared, "first").unwrap().notify,
		"capture notifications ship on"
	);

	store::lock(&harness.shared)
		.update_settings(serde_json::from_str(r#"{"captureNotifications":false}"#).unwrap())
		.unwrap();

	assert!(!store::append_capture(&harness.shared, "second").unwrap().notify);
}

/// The attachments' fallback writer: a file too large to attach leaves its path
/// behind as a note, through the same seam shape `append_capture` and
/// `move_notes` use.
#[test]
fn append_paths_note_emits_one_attach_and_is_undoable() {
	let harness = Harness::new();
	harness.sink.take();

	store::append_paths_note(&harness.shared, "C:\\Videos\\too-big.mp4").unwrap();

	let events = harness.sink.take();
	assert_eq!(events.len(), 1);
	// Not `Capture`: the panel answers that reason with a sound and a scroll
	// request, and the user is standing at the panel with their hands on the
	// drop that produced this.
	assert_eq!(reasons(&events), [ChangeReason::Attach]);

	let doc = harness.doc();
	let note = doc
		.notes
		.iter()
		.find(|note| note.body == "C:\\Videos\\too-big.mp4")
		.expect("the path landed as a note body, verbatim");
	assert_eq!(note.section, doc.active_section, "an unaddressed note lands in the active section");

	// One snapshot, one `Ctrl+Z` — the same cost as any note added from the panel.
	assert!(harness.status().can_undo);
	store::lock(&harness.shared).undo().unwrap();
	assert!(
		!harness.doc().notes.iter().any(|note| note.body == "C:\\Videos\\too-big.mp4"),
		"undo removes the path note"
	);
}

// --- device share: append_received (task-026) --------------------------------

fn received(body: &str) -> store::ReceivedNote {
	store::ReceivedNote {
		body: body.into(),
		attachments: Vec::new(),
	}
}

/// The bodies of the notes in the `Received` section, in document order.
fn received_bodies(doc: &Space) -> Vec<String> {
	let Some(section) = doc
		.sections
		.iter()
		.find(|section| section.name == store::RECEIVED_SECTION)
	else {
		return Vec::new();
	};
	let mut notes: Vec<&Note> = doc.notes.iter().filter(|note| note.section == section.id).collect();
	notes.sort_by_key(|note| note.order);
	notes.iter().map(|note| note.body.clone()).collect()
}

fn set_insertion(harness: &Harness, at: InsertionPoint) {
	let patch = match at {
		InsertionPoint::Top => r#"{"insertionPoint":"top"}"#,
		InsertionPoint::Bottom => r#"{"insertionPoint":"bottom"}"#,
	};
	store::lock(&harness.shared)
		.update_settings(serde_json::from_str(patch).unwrap())
		.unwrap();
}

#[test]
fn append_received_creates_the_received_section_and_reuses_it() {
	let harness = Harness::new();
	assert!(harness.section_named(store::RECEIVED_SECTION).is_none());

	let path = harness.path();
	store::append_received(&harness.shared, &path, &[received("from the laptop")]).unwrap();
	let created = harness
		.section_named(store::RECEIVED_SECTION)
		.expect("the section is created when absent");

	store::append_received(&harness.shared, &path, &[received("and again")]).unwrap();
	assert_eq!(
		harness.section_named(store::RECEIVED_SECTION).as_deref(),
		Some(created.as_str()),
		"a second message created a second Received section"
	);
	assert_eq!(
		harness
			.doc()
			.sections
			.iter()
			.filter(|section| section.name == store::RECEIVED_SECTION)
			.count(),
		1
	);
}

/// `ops::section_by_name` matches case-insensitively, so a hand-made `received`
/// section is the one a delivery files into rather than being shadowed by a
/// second header that looks identical.
#[test]
fn append_received_reuses_a_differently_cased_section() {
	let harness = Harness::new();
	let existing = store::lock(&harness.shared)
		.mutate(|doc| ops::add_section(doc, "received"))
		.unwrap()
		.0;

	store::append_received(&harness.shared, &harness.path(), &[received("x")]).unwrap();

	let doc = harness.doc();
	assert_eq!(doc.sections.iter().filter(|s| s.name.to_lowercase() == "received").count(), 1);
	assert!(doc.notes.iter().any(|note| note.section == existing && note.body == "x"));
}

/// `ops::add_note` with `Top` stacks consecutive adds newest-first
/// (ops.rs:202-211), so a message written front-to-back would arrive upside
/// down. Both settings have to produce the sender's order.
#[test]
fn a_multi_note_message_arrives_in_the_senders_order_under_both_insertion_points() {
	for at in [InsertionPoint::Top, InsertionPoint::Bottom] {
		let harness = Harness::new();
		set_insertion(&harness, at);

		store::append_received(
			&harness.shared,
			&harness.path(),
			&[received("first"), received("second"), received("third")],
		)
		.unwrap();

		assert_eq!(
			received_bodies(&harness.doc()),
			["first", "second", "third"],
			"a message arrived out of order under {at:?}"
		);
	}
}

#[test]
fn a_whole_message_is_one_undo_entry_and_one_received_event() {
	let harness = Harness::new();
	harness.sink.take();

	store::append_received(
		&harness.shared,
		&harness.path(),
		&[received("one"), received("two"), received("three")],
	)
	.unwrap();

	let events = harness.sink.take();
	assert_eq!(events.len(), 1, "a three-note message emitted more than once");
	assert_eq!(reasons(&events), [ChangeReason::Received]);

	assert!(harness.status().can_undo);
	store::lock(&harness.shared).undo().unwrap();
	assert!(
		received_bodies(&harness.doc()).is_empty(),
		"one Ctrl+Z did not remove the whole message"
	);

	settle();
	assert!(
		harness.sink.events().is_empty(),
		"the delivery's own write produced a watcher event: {:?}",
		harness.sink.names()
	);
}

/// A note arriving while the user is typing somewhere else must not move them.
/// `ops::add_section` sets `active_section` (ops.rs:425), which is right for the
/// section switcher and wrong here.
#[test]
fn delivery_does_not_move_the_active_section() {
	let harness = Harness::new();
	let before = harness.doc().active_section.clone();

	// Once creating the section, once reusing it: the create path is the one that
	// would move it.
	store::append_received(&harness.shared, &harness.path(), &[received("first")]).unwrap();
	assert_eq!(harness.doc().active_section, before, "creating Received moved the active section");

	store::append_received(&harness.shared, &harness.path(), &[received("second")]).unwrap();
	assert_eq!(harness.doc().active_section, before);
}

/// The blobs were ingested beside one space. Filing the notes into another would
/// leave them pointing at attachments that are not there — which is why the
/// check is inside `append_received` rather than at its call site.
#[test]
fn a_mismatched_expected_path_writes_nothing_and_emits_nothing() {
	let harness = Harness::new();
	let before = harness.text();
	harness.sink.take();

	let err = store::append_received(
		&harness.shared,
		Path::new("C:\\somewhere\\else.copper"),
		&[received("misfiled")],
	)
	.unwrap_err();

	assert_eq!(err.kind(), "unavailable");
	assert!(received_bodies(&harness.doc()).is_empty());
	assert_eq!(harness.text(), before, "a refused delivery rewrote the file");
	assert!(harness.sink.take().is_empty());
	assert!(!harness.status().can_undo, "a refused delivery pushed an undo snapshot");
}

#[test]
fn an_empty_message_is_refused_before_the_lock() {
	let harness = Harness::new();
	harness.sink.take();

	let err = store::append_received(&harness.shared, &harness.path(), &[]).unwrap_err();

	assert_eq!(err.kind(), "invalid");
	assert!(harness.sink.take().is_empty());
	assert!(!harness.status().can_undo);
}

#[test]
fn the_emit_matrix_holds_for_every_command_path() {
	let harness = Harness::new();
	let id = harness.add("alpha").unwrap();
	harness.add("beta").unwrap();
	let section = harness.doc().active_section.clone();
	harness.sink.take();

	// Every frontend-invoked mutation, all eighteen of them: nothing emits,
	// because the return value already describes the change (spec 8.4). Closures
	// resolve section ids from the document so the whole matrix runs under one
	// guard.
	let other_section = |doc: &Space| {
		doc.sections
			.iter()
			.find(|candidate| candidate.id != section)
			.expect("a second section")
			.id
			.clone()
	};
	let all_notes = |doc: &Space| -> Vec<String> {
		doc.notes.iter().map(|note| note.id.clone()).collect()
	};

	let mut guard = store::lock(&harness.shared);
	guard.mutate(|doc| ops::set_notes_done(doc, &[id.clone()], true)).unwrap();
	guard.mutate_no_snapshot(|doc| ops::edit_note(doc, &id, "alpha, revised")).unwrap();
	guard.mutate(|doc| ops::add_section(doc, "Later")).unwrap();
	guard
		.mutate(|doc| {
			let target = other_section(doc);
			ops::rename_section(doc, &target, "Renamed")
		})
		.unwrap();
	guard
		.mutate(|doc| {
			let target = other_section(doc);
			ops::reorder_section(doc, &target, 0)
		})
		.unwrap();
	guard.mutate(|doc| ops::reorder_note(doc, &id, &section, 0)).unwrap();
	guard
		.mutate(|doc| {
			let target = other_section(doc);
			ops::move_notes(doc, &[id.clone()], &target)
		})
		.unwrap();
	guard.mutate(|doc| ops::merge_notes(doc, &all_notes(doc))).unwrap();
	guard.mutate_no_snapshot(|doc| ops::set_active_section(doc, &section)).unwrap();
	guard.mutate(|doc| ops::delete_notes(doc, &all_notes(doc))).unwrap();
	guard
		.mutate(|doc| {
			let target = other_section(doc);
			ops::delete_section(doc, &target)
		})
		.unwrap();
	guard.undo().unwrap();
	guard.redo().unwrap();
	guard.update_settings(serde_json::from_str(r#"{"theme":"dark"}"#).unwrap()).unwrap();
	drop(guard);
	assert!(
		harness.sink.events().is_empty(),
		"a mutating command emitted: {:?}",
		harness.sink.names()
	);

	// Both recents-touching commands emit exactly one settings-changed.
	let other = harness.config.join("spaces").join("other.copper");
	store::create_space(&harness.shared, &other, "other").unwrap();
	assert_eq!(harness.sink.take_names(), ["settings-changed"]);

	store::open_space(&harness.shared, &harness.config.join("spaces").join("personal.copper"))
		.unwrap();
	assert_eq!(harness.sink.take_names(), ["settings-changed"]);

	// Failures emit nothing.
	store::open_space(&harness.shared, Path::new("D:\\nope\\missing.copper")).unwrap_err();
	store::create_space(&harness.shared, &other, "other").unwrap_err();
	store::lock(&harness.shared)
		.mutate(|doc| ops::delete_notes(doc, &["nte_nope".to_string()]))
		.unwrap_err();
	assert!(
		harness.sink.events().is_empty(),
		"a failed command emitted: {:?}",
		harness.sink.names()
	);
}

#[test]
fn get_status_tracks_every_producer() {
	let harness = Harness::new();

	let fresh = harness.status();
	assert!(!fresh.can_undo, "a freshly opened space has nothing to undo");
	assert!(!fresh.can_redo);
	assert!(!fresh.errored);
	assert!(fresh.watching);
	assert_eq!(fresh.path, Some(store::path_string(&harness.path())));
	assert!(fresh.startup_notice.is_none());

	harness.add("alpha").unwrap();
	assert!(harness.status().can_undo, "a structural mutation must enable undo");
	assert!(!harness.status().can_redo);

	store::lock(&harness.shared).undo().unwrap();
	assert!(!harness.status().can_undo);
	assert!(harness.status().can_redo, "undo must enable redo");

	store::lock(&harness.shared).redo().unwrap();
	assert!(harness.status().can_undo);
	assert!(!harness.status().can_redo);

	store::append_capture(&harness.shared, "captured").unwrap();
	assert!(harness.status().can_undo);

	// Invalidation, then recovery.
	let good = harness.text();
	external_write(&harness.path(), "{ broken");
	assert!(wait_until(|| harness.status().errored));
	assert!(harness.status().watching, "an errored space keeps its watch");

	external_write(&harness.path(), &good);
	assert!(wait_until(|| !harness.status().errored));
	// Recovering to the *same* document keeps the stacks: spec 4.6 clears them
	// because a reload means the stack describes a document that is no longer on
	// disk, and here it still does. Destroying a session's undo history over a
	// transient unreadable window would be a worse answer than the rule's letter.
	assert!(harness.status().can_undo, "a no-op recovery discarded the undo stack");

	// A reload that really did change the document does clear them.
	external_note(&harness, "somebody else's note");
	assert!(wait_until(|| !harness.status().can_undo));
	assert!(!harness.status().can_redo);
}

// --- performance guard (A9.14) ------------------------------------------------
#[test]
fn one_mutate_cycle_on_a_five_hundred_note_document_is_not_quadratic() {
	let harness = Harness::new();
	let path = harness.config.join("spaces").join("large.copper");

	let mut doc = Space {
		id: "spc_large001".into(),
		name: "large".into(),
		active_section: "sec_large001".into(),
		sections: (0..5)
			.map(|index| Section {
				id: format!("sec_large{index:03}"),
				name: format!("Section {index}"),
				order: index,
			})
			.collect(),
		notes: Vec::new(),
	};
	doc.active_section = doc.sections[0].id.clone();
	for index in 0..500 {
		doc.notes.push(Note {
			id: format!("nte_large{index:04}"),
			section: doc.sections[(index % 5) as usize].id.clone(),
			order: index / 5,
			done: index % 3 == 0,
			body: format!("Note number {index} with a body long enough to be realistic."),
			attachments: Vec::new(),
			created: "2026-07-30T14:00:00Z".into(),
			updated: "2026-07-30T14:00:00Z".into(),
		});
	}
	std::fs::write(&path, format::to_git_json(&doc).unwrap()).unwrap();
	store::open_space(&harness.shared, &path).unwrap();
	assert_eq!(harness.doc().notes.len(), 500);

	let started = Instant::now();
	harness.add("one more").unwrap();
	let elapsed = started.elapsed();

	assert_eq!(harness.doc().notes.len(), 501);
	// Deliberately loose: this catches accidental quadratic behaviour in
	// `normalise` or the ops, it does not benchmark, and it must not flake on a
	// slow disk.
	assert!(
		elapsed < Duration::from_millis(250),
		"one mutate cycle on 500 notes took {elapsed:?}"
	);
}

// --- settings round trip through the store ------------------------------------
#[test]
fn update_settings_persists_and_returns_the_new_settings() {
	let harness = Harness::new();

	let updated = store::lock(&harness.shared)
		.update_settings(
			serde_json::from_str(r#"{"panelPosition":{"x":2140,"y":180},"theme":"dark"}"#).unwrap(),
		)
		.unwrap();

	assert_eq!(updated.theme, "dark");
	assert_eq!(updated.panel_position.unwrap().x, 2140);

	let reloaded = settings::load(&harness.config.join("settings.json"));
	assert!(reloaded.notice.is_none());
	assert_eq!(reloaded.settings.theme, "dark");
	assert_eq!(reloaded.settings.panel_position, updated.panel_position);
	// Opening a space is still the only thing that touches recents.
	assert_eq!(reloaded.settings.recents, harness.settings().recents);
}


// --- read-path retry (task-005 review ruling, 2026-08-05) --------------------
// The write path has retried a sharing violation since spec 2.2; the read path
// did not. A brief hold by antivirus, the indexer or OneDrive during a watcher
// event therefore marked the space unreadable with no recovery until some other
// filesystem event happened to arrive.
/// Holds `path` open with no sharing for `hold`, then releases it.
///
/// Returns a join handle plus a barrier the caller waits on, so the hold is
/// provably in place before the read starts rather than racing it.
fn hold_exclusively(path: &Path, hold: Duration) -> thread::JoinHandle<()> {
	let path = path.to_path_buf();
	let (ready_tx, ready_rx) = std::sync::mpsc::channel();
	let handle = thread::spawn(move || {
		let file = OpenOptions::new()
			.read(true)
			.share_mode(0)
			.open(&path)
			.expect("the file exists and nothing else holds it");
		ready_tx.send(()).unwrap();
		thread::sleep(hold);
		drop(file);
	});
	ready_rx.recv().expect("the hold thread started");
	handle
}

#[test]
fn a_read_survives_a_transient_sharing_violation() {
	let harness = Harness::new();
	let path = harness.path();
	let expected = harness.text();

	// Well inside the 500 ms backoff budget, and long enough that the first
	// attempt is guaranteed to fail.
	let holder = hold_exclusively(&path, Duration::from_millis(200));

	let started = Instant::now();
	let text = atomic::read_with_backoff(&path).expect("a transient hold must not fail the read");
	let elapsed = started.elapsed();

	holder.join().unwrap();
	assert_eq!(text, expected);
	assert!(
		elapsed >= Duration::from_millis(25),
		"the read returned in {elapsed:?}, so it cannot have waited out the hold"
	);
	assert!(
		elapsed < Duration::from_millis(500),
		"the read took {elapsed:?}, past the whole backoff budget"
	);
}

#[test]
fn a_permanently_unreadable_file_still_fails_fast() {
	// The retry must not turn a missing file into half a second of waiting: the
	// recents fallback (spec 7.3) reads paths that are legitimately absent.
	let harness = Harness::new();
	let missing = harness.config.join("spaces").join("not-here.copper");

	let started = Instant::now();
	let err = atomic::read_with_backoff(&missing).unwrap_err();
	let elapsed = started.elapsed();

	assert_eq!(err.kind(), "not-found");
	assert!(
		elapsed < Duration::from_millis(100),
		"a missing file took {elapsed:?}; it must not be retried"
	);
}

#[test]
fn opening_a_space_survives_a_transient_sharing_violation() {
	// The `OpenSpace::load` read site.
	let harness = Harness::new();
	let other = harness.config.join("spaces").join("second.copper");
	std::fs::write(&other, format::to_git_json(&golden_doc()).unwrap()).unwrap();

	let holder = hold_exclusively(&other, Duration::from_millis(200));
	let opened = store::open_space(&harness.shared, &other);
	holder.join().unwrap();

	let opened = opened.expect("a transient hold must not fail the open");
	assert_eq!(body_set(&opened), body_set(&golden_doc()));
	assert!(!harness.status().errored);
}

#[test]
fn a_watcher_reload_survives_a_transient_sharing_violation() {
	// The `reload_from_disk` read site — the one the ruling is actually about,
	// since that is where a failed read marks the space errored with no recovery.
	let harness = Harness::new();
	let path = harness.path();

	let mut updated = harness.doc();
	updated.notes.push(Note {
		id: "n-held".to_owned(),
		section: updated.sections[0].id.clone(),
		order: 900,
		done: false,
		body: "written while the file was about to be held".to_owned(),
		attachments: Vec::new(),
		created: "2026-08-05T12:00:00Z".to_owned(),
		updated: "2026-08-05T12:00:00Z".to_owned(),
	});
	external_write(&path, &format::to_git_json(&updated).unwrap());

	// Held across the debounce, so the reload's first read attempt lands inside
	// the hold. Shorter than the backoff budget, so it must still recover.
	let holder = hold_exclusively(&path, Duration::from_millis(400));

	assert!(
		wait_until(|| harness
			.doc()
			.notes
			.iter()
			.any(|note| note.id == "n-held")),
		"the external write never arrived: {:?}",
		harness.status()
	);
	holder.join().unwrap();

	assert!(
		!harness.status().errored,
		"a transient hold left the space marked unreadable"
	);
}

// --- attachments (task-011) --------------------------------------------------
/// AC5. The one assertion that keeps every earlier phase's on-disk contract
/// intact: a document with no attachments must not gain so much as a key.
///
/// `skip_serializing_if` is what does it, and this is the test that fails if
/// somebody removes it — the golden fixture test above would fail too, but this
/// one says *why*.
#[test]
fn a_document_without_attachments_gains_no_key() {
	let text = format::to_git_json(&golden_doc()).unwrap();
	assert!(
		!text.contains("attachments"),
		"a note with no attachments serialised an attachments key"
	);
	assert_eq!(text, golden_text(), "the golden fixture is no longer byte-identical");
}
