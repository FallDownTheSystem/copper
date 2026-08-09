//! `open_headless` and `create_headless`: the constructors a second process uses.
//!
//! The claim under test is a pair. The write pipeline is **the same one** the app
//! uses — same canonical bytes, same compare-and-swap, same three attempts — and
//! everything else the app's startup does is **absent**: no `settings.json` is
//! read, written or created, no space is invented, no recents entry is promoted,
//! no watcher is registered.
//!
//! The second half is the one worth testing at this level, because its failure
//! mode is silent. A CLI that reordered the user's recents while answering
//! `copper note list` would look like it worked.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use copper_core::store::events::NullSink;
use copper_core::store::model::Space;
use copper_core::store::settings::InsertionPoint;
use copper_core::store::{self, format, ops};

const GOLDEN: &str = include_str!("fixtures/space-golden.copper");

/// The golden fixture's *last* section, "Configuration Formats". A note added
/// here is appended at the end of the notes array, which is what the minimal-diff
/// assertion describes.
const LAST_SECTION: &str = "sec_b2000002";

/// A temp directory holding a copy of the golden fixture.
///
/// The fixture rather than a fresh document on purpose: it is the same file the
/// byte-stability tests read, so "the CLI's write is byte-indistinguishable from
/// the app's" is checked against the exact bytes that claim is already pinned on.
struct Fixture {
	_dir: tempfile::TempDir,
	path: PathBuf,
}

impl Fixture {
	fn new() -> Self {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("golden.copper");
		std::fs::write(&path, GOLDEN).unwrap();
		Self { _dir: dir, path }
	}

	fn dir(&self) -> &Path {
		self.path.parent().unwrap()
	}

	fn text(&self) -> String {
		std::fs::read_to_string(&self.path).unwrap()
	}

	fn open(&self) -> store::Store {
		store::open_headless(&self.path, Arc::new(NullSink)).expect("the fixture opens")
	}
}

/// The error from an open that must fail.
///
/// `Result::unwrap_err` cannot be used here: it needs `T: Debug`, and `Store`
/// holds an `Arc<dyn EventSink>` that has no reasonable one.
fn open_err(path: &Path) -> copper_core::store::error::StoreError {
	match store::open_headless(path, Arc::new(NullSink)) {
		Ok(_) => panic!("{} opened when it should not have", path.display()),
		Err(err) => err,
	}
}

/// Every file anywhere under `dir`, as paths relative to it.
fn tree(dir: &Path) -> Vec<String> {
	fn walk(dir: &Path, root: &Path, into: &mut Vec<String>) {
		for entry in std::fs::read_dir(dir).unwrap() {
			let entry = entry.unwrap();
			let path = entry.path();
			if path.is_dir() {
				walk(&path, root, into);
			} else {
				into.push(
					path.strip_prefix(root)
						.unwrap()
						.to_string_lossy()
						.into_owned(),
				);
			}
		}
	}
	let mut found = Vec::new();
	walk(dir, dir, &mut found);
	found.sort();
	found
}

// --- open_headless ------------------------------------------------------------

/// The whole point, in one assertion pair: the note lands through the ordinary
/// pipeline, and nothing else appears on disk.
#[test]
fn a_headless_mutation_writes_canonical_bytes_and_creates_no_settings() {
	let fixture = Fixture::new();
	let mut store = fixture.open();

	let (id, doc) = store
		.mutate(|space| ops::add_note(space, "from the cli", None, &[], InsertionPoint::Bottom))
		.expect("the mutation lands");

	assert_eq!(
		fixture.text(),
		format::to_git_json(&doc).unwrap(),
		"the file is not the canonical serialisation of the document"
	);
	assert!(doc.note(&id).is_some());
	assert_eq!(
		tree(fixture.dir()),
		["golden.copper"],
		"a headless store touched something other than the space file"
	);
}

/// Spec: a CLI write must be byte-indistinguishable from an app write. The
/// strongest available form of that is the minimal-diff property `format.rs`
/// already pins — an appended note adds exactly its own nine lines and a comma.
///
/// The section is named rather than left to the document's active one, and has to
/// be: the fixture's active section is its *first*, so a note added there lands in
/// the middle of the notes array. The diff is just as minimal, but it is no longer
/// an append, and "an append is nine lines at the end" is the property the shipped
/// assertion states.
#[test]
fn appending_a_note_headless_produces_the_same_minimal_diff() {
	let fixture = Fixture::new();
	let before = fixture.text();
	let mut store = fixture.open();

	store
		.mutate(|space| {
			ops::add_note(
				space,
				"appended",
				Some(LAST_SECTION),
				&[],
				InsertionPoint::Bottom,
			)
		})
		.expect("the mutation lands");

	let after = fixture.text();
	assert_minimal_note_diff(&before, &after);
}

/// `format.rs`'s `appending_a_note_produces_a_minimal_diff`, applied to two
/// versions of a file rather than to two serialisations of a document.
///
/// Reconstructing the expected result is stricter than comparing common prefixes
/// and suffixes, and it does not depend on which of two identical `}` lines a
/// diff algorithm chooses to align.
///
/// `pub` because the end-to-end CLI test makes the same assertion about the same
/// file, one process boundary further out.
pub fn assert_minimal_note_diff(before: &str, after: &str) {
	let before_lines: Vec<&str> = before.lines().collect();
	let after_lines: Vec<&str> = after.lines().collect();

	let closing = before_lines.len() - 3;
	assert_eq!(
		before_lines[closing].trim(),
		"}",
		"not the last note's closing brace"
	);

	let added = &after_lines[closing + 1..after_lines.len() - 2];
	assert_eq!(
		added.len(),
		9,
		"the added region is not exactly one note object: {added:#?}"
	);
	assert_eq!(added[0].trim(), "{");
	assert_eq!(added[8].trim(), "}");

	let mut expected: Vec<String> = before_lines.iter().map(|line| line.to_string()).collect();
	// The one unavoidable change to an existing line, and the reason the criterion
	// permits it: JSON gives the previous last element a comma.
	expected[closing] = format!("{},", expected[closing]);
	expected.splice(
		closing + 1..closing + 1,
		added.iter().map(|line| line.to_string()),
	);

	let after_owned: Vec<String> = after_lines.iter().map(|line| line.to_string()).collect();
	assert_eq!(expected, after_owned, "something outside the new note changed");
}

#[test]
fn opening_a_missing_path_is_not_found() {
	let dir = tempfile::tempdir().unwrap();
	let missing = dir.path().join("nowhere.copper");

	let err = open_err(&missing);

	assert_eq!(err.kind(), "not-found", "{}", err.message());
	assert!(tree(dir.path()).is_empty(), "a failed open created something");
}

#[test]
fn opening_a_directory_is_invalid() {
	let dir = tempfile::tempdir().unwrap();
	let folder = dir.path().join("a-folder.copper");
	std::fs::create_dir(&folder).unwrap();

	let err = open_err(&folder);

	assert_eq!(err.kind(), "invalid", "{}", err.message());
	assert!(err.message().contains("folder"), "{}", err.message());
}

#[test]
fn opening_an_unparseable_file_is_a_parse_error() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("broken.copper");
	std::fs::write(&path, "{ not a space").unwrap();

	let err = open_err(&path);

	assert_eq!(err.kind(), "parse", "{}", err.message());
}

/// The acceptance criterion, at the unit level: three attempts, then `conflict`,
/// and the file holds the external writer's content rather than a merge.
///
/// The interfering writer is driven from the operation closure rather than from a
/// racing thread, exactly as `store_fs.rs` drives its own: `op` runs at the top of
/// every attempt, before the file is read, so each attempt is guaranteed to find
/// content it has not seen and the exhaustion path is reached every run.
#[test]
fn three_exhausted_attempts_leave_the_external_content_and_report_a_conflict() {
	let fixture = Fixture::new();
	let mut store = fixture.open();
	let base: Space = format::parse_normalised(&fixture.text()).unwrap();

	let generation = AtomicUsize::new(0);
	let write_path = fixture.path.clone();
	let result = store.mutate(move |space| {
		let mut external = base.clone();
		external.name = format!("generation {}", generation.fetch_add(1, Ordering::SeqCst));
		std::fs::write(&write_path, format::to_git_json(&external).unwrap()).unwrap();
		ops::add_note(space, "should never land", None, &[], InsertionPoint::Bottom).map(|_| ())
	});

	let err = result.unwrap_err();
	assert_eq!(err.kind(), "conflict", "{}", err.message());

	let on_disk = format::from_json(&fixture.text()).unwrap();
	assert_eq!(on_disk.name, "generation 2", "the attempt count is not three");
	assert!(
		!on_disk.notes.iter().any(|note| note.body == "should never land"),
		"an exhausted conflict still wrote"
	);
}

/// A conflict that resolves: the external change survives and ours is re-applied
/// on top of it. Same pipeline as the app, so this needs no new logic — which is
/// exactly what it is here to confirm.
#[test]
fn a_conflicting_headless_write_keeps_both_changes() {
	let fixture = Fixture::new();
	let mut store = fixture.open();
	let base: Space = format::parse_normalised(&fixture.text()).unwrap();

	let once = AtomicUsize::new(0);
	let write_path = fixture.path.clone();
	store
		.mutate(move |space| {
			if once.fetch_add(1, Ordering::SeqCst) == 0 {
				let mut external = base.clone();
				ops::add_note(&mut external, "theirs", None, &[], InsertionPoint::Bottom).unwrap();
				std::fs::write(&write_path, format::to_git_json(&external).unwrap()).unwrap();
			}
			ops::add_note(space, "ours", None, &[], InsertionPoint::Bottom).map(|_| ())
		})
		.expect("the second attempt commits");

	let on_disk = format::from_json(&fixture.text()).unwrap();
	let bodies: Vec<&str> = on_disk.notes.iter().map(|note| note.body.as_str()).collect();
	assert!(bodies.contains(&"theirs"), "the conflict path lost the external change");
	assert!(bodies.contains(&"ours"), "the conflict path lost our change");
}

// --- the headless refusals ------------------------------------------------------

/// The flag exists so that a future caller cannot reach a settings write by
/// accident. Each of these would otherwise run against an empty `settings_path`.
#[test]
fn a_headless_store_refuses_every_settings_write() {
	let fixture = Fixture::new();
	let mut store = fixture.open();

	let patched = store.update_settings(Default::default()).unwrap_err();
	assert_eq!(patched.kind(), "invalid", "{}", patched.message());

	let forgotten = store.remove_recent(&fixture.path).unwrap_err();
	assert_eq!(forgotten.kind(), "invalid", "{}", forgotten.message());

	assert_eq!(
		tree(fixture.dir()),
		["golden.copper"],
		"a refused settings write still created a file"
	);
}

/// It carries no settings to report, and says so rather than reporting the
/// defaults as though they were the user's.
#[test]
fn a_headless_store_reports_no_watcher_and_the_open_path() {
	let fixture = Fixture::new();
	let store = fixture.open();

	let status = store.status();
	assert!(!status.watching, "a headless store registered a watch");
	assert!(!status.errored);
	assert!(!status.can_undo);
	assert!(status.startup_notice.is_none());
	assert!(status.path.is_some());
	assert_eq!(store.recents(), Vec::<String>::new());
}

// --- create_headless ------------------------------------------------------------

#[test]
fn creating_a_space_writes_a_document_that_parses_under_the_given_name() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("fresh.copper");

	store::create_headless(&path, "fresh").expect("the create lands");

	let doc = format::parse_normalised(&std::fs::read_to_string(&path).unwrap()).unwrap();
	assert_eq!(doc.name, "fresh");
	assert_eq!(doc.sections.len(), 1);
	assert!(doc.notes.is_empty());
	assert_eq!(tree(dir.path()), ["fresh.copper"], "a create wrote something else");
}

/// `commit_new` semantics: the filesystem refuses, so a file appearing between a
/// check and a write has no window in which to be destroyed.
#[test]
fn creating_over_an_existing_file_is_invalid_and_keeps_its_bytes() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("taken.copper");
	std::fs::write(&path, "precious").unwrap();

	let err = store::create_headless(&path, "taken").unwrap_err();

	assert_eq!(err.kind(), "invalid", "{}", err.message());
	assert!(err.message().contains("already exists"), "{}", err.message());
	assert_eq!(
		std::fs::read_to_string(&path).unwrap(),
		"precious",
		"a refused create overwrote the file"
	);
}

#[test]
fn creating_a_space_with_an_empty_name_is_invalid() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("unnamed.copper");

	let err = store::create_headless(&path, "   ").unwrap_err();

	assert_eq!(err.kind(), "invalid", "{}", err.message());
	assert!(!path.exists(), "a refused create left a file behind");
}

/// The parent directory is created, because `copper space create` naming a path
/// two levels down is an ordinary thing to type.
#[test]
fn creating_a_space_makes_its_parent_directory() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("nested").join("deep").join("new.copper");

	store::create_headless(&path, "new").expect("the create lands");

	assert!(path.is_file());
}
