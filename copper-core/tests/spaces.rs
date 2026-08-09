//! Space identity against real temp directories: what survives a restart, and
//! what the recents list does when spaces are opened and forgotten.
//!
//! Everything here runs without a Tauri runtime, and now without Tauri in the
//! crate at all. The policy layer above this — `open_space_at`, the switcher's
//! commands — needs an `AppHandle` and is asserted structurally from
//! `src-tauri/tests/spaces.rs`; the durability half is ordinary `cargo test`
//! territory and is where the ordinary cases actually get asserted.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use copper_core::spaces::availability::{self, Availability, RealFs, UnavailableReason};
use copper_core::spaces::paths::{comparison_key, same_path};
use copper_core::store::events::RecordingSink;
use copper_core::store::settings::Settings;
use copper_core::store::{self, SharedStore};

struct Rig {
	_dir: tempfile::TempDir,
	root: PathBuf,
	config: PathBuf,
	shared: SharedStore,
}

impl Rig {
	fn new() -> Self {
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path().to_path_buf();
		let config = root.join("Copper");
		let shared = bootstrap(&config);
		Self {
			_dir: dir,
			root,
			config,
			shared,
		}
	}

	/// A restart in the only sense that matters here: the store is rebuilt from
	/// what is on disk, with nothing carried over in memory.
	fn restart(&mut self) {
		self.shared = bootstrap(&self.config);
	}

	fn create(&self, name: &str) -> PathBuf {
		let path = self.root.join(format!("{name}.copper"));
		store::create_space(&self.shared, &path, name).unwrap();
		store::canonical(&path).unwrap()
	}

	fn open(&self, path: &Path) {
		store::open_space(&self.shared, path).unwrap();
	}

	fn recents(&self) -> Vec<String> {
		store::lock(&self.shared).recents().to_vec()
	}

	fn active(&self) -> PathBuf {
		store::lock(&self.shared)
			.active_path()
			.expect("a space is always open")
			.to_path_buf()
	}

	fn settings(&self) -> Settings {
		store::lock(&self.shared).settings().clone()
	}
}

fn bootstrap(config: &Path) -> SharedStore {
	let sink = Arc::new(RecordingSink::new());
	let shared: SharedStore = Arc::new(Mutex::new(store::bootstrap_store(config, sink).unwrap()));
	store::attach_watcher(&shared);
	shared
}

/// Whether the settings file agrees with the store about which space is open.
/// `activeSpace` is an index into a list that promotion reorders, so "internally
/// consistent" is a real property rather than a tautology.
fn index_points_at_the_open_space(rig: &Rig) -> bool {
	let settings = rig.settings();
	settings
		.active_recent()
		.is_some_and(|entry| same_path(Path::new(entry), &rig.active()))
}

// --- the ordinary cases -------------------------------------------------------

/// A31. Not merely the same `activeSpace` integer round-tripping: the document
/// has to actually load.
#[test]
fn the_active_space_and_the_recents_order_survive_a_restart() {
	let mut rig = Rig::new();
	let alpha = rig.create("alpha");
	let beta = rig.create("beta");
	let gamma = rig.create("gamma");

	rig.open(&alpha);
	rig.open(&gamma);
	let order = rig.recents();
	assert_eq!(order[0], gamma.to_string_lossy());

	rig.restart();

	assert_eq!(rig.active(), gamma);
	assert_eq!(
		store::lock(&rig.shared).active_space().unwrap().name,
		"gamma",
		"the index came back but the document did not"
	);
	assert_eq!(rig.recents(), order);
	assert!(index_points_at_the_open_space(&rig));
	assert!(rig.recents().iter().any(|entry| entry == &beta.to_string_lossy()));
}

/// The switcher labels the active row on every menu open and on every
/// `settings-changed`, so it reads the name through the cheap accessor. That
/// accessor has to agree with the expensive one it replaced.
#[test]
fn the_active_name_agrees_with_the_open_document() {
	let rig = Rig::new();
	let alpha = rig.create("alpha");
	rig.open(&alpha);

	let guard = store::lock(&rig.shared);
	let doc = guard.active_space().unwrap();
	assert_eq!(doc.name, "alpha");
	assert_eq!(guard.active_name(), Some(doc.name.as_str()));
}

/// A32. Removing a non-active entry drops exactly that entry, leaves the rest in
/// order, does not change the active space, and sticks.
#[test]
fn a_removed_entry_stays_removed_and_the_active_space_is_untouched() {
	let mut rig = Rig::new();
	let alpha = rig.create("alpha");
	let beta = rig.create("beta");
	rig.open(&alpha);
	let before = rig.recents();

	store::remove_recent(&rig.shared, &beta).unwrap();

	assert_eq!(rig.active(), alpha, "removing an entry closed the open space");
	let after = rig.recents();
	assert!(!after.iter().any(|entry| entry == &beta.to_string_lossy()));
	assert_eq!(after.len(), before.len() - 1);
	assert_eq!(
		after,
		before
			.into_iter()
			.filter(|entry| entry != &beta.to_string_lossy())
			.collect::<Vec<_>>(),
		"removal reordered the entries it kept"
	);

	rig.restart();

	assert!(!rig.recents().iter().any(|entry| entry == &beta.to_string_lossy()));
	assert_eq!(rig.active(), alpha);
	assert!(index_points_at_the_open_space(&rig));
}

/// Spec 6.7, inherited: removing an absent path is a successful no-op rather
/// than an error, because the desired end state already holds.
#[test]
fn removing_a_path_that_is_not_listed_succeeds() {
	let rig = Rig::new();
	let before = rig.recents();

	store::remove_recent(&rig.shared, &rig.root.join("never-there.copper")).unwrap();

	assert_eq!(rig.recents(), before);
}

/// A2, through the real open path rather than through `touch_recent` alone.
#[test]
fn re_opening_a_listed_space_promotes_it_instead_of_duplicating_it() {
	let rig = Rig::new();
	let alpha = rig.create("alpha");
	let beta = rig.create("beta");

	rig.open(&alpha);
	rig.open(&beta);
	rig.open(&alpha);

	assert_eq!(rig.recents()[0], alpha.to_string_lossy());
	assert_eq!(
		rig.recents()
			.iter()
			.filter(|entry| same_path(Path::new(entry), &alpha))
			.count(),
		1,
		"the same space is listed twice"
	);
	assert!(index_points_at_the_open_space(&rig));
}

/// Windows paths are case-insensitive, so a differently-cased spelling of a
/// listed space is the same entry, not a second one.
#[test]
fn a_differently_cased_path_is_the_same_entry() {
	let rig = Rig::new();
	let alpha = rig.create("alpha");
	rig.open(&alpha);
	let shouted = PathBuf::from(alpha.to_string_lossy().to_uppercase());

	rig.open(&shouted);

	assert_eq!(
		rig.recents()
			.iter()
			.filter(|entry| same_path(Path::new(entry), &alpha))
			.count(),
		1,
		"one file is listed twice under two spellings: {:?}",
		rig.recents()
	);
	// Alpha plus the default space bootstrap created — the shouted spelling added
	// nothing of its own.
	assert_eq!(rig.recents().len(), 2);
	assert!(same_path(&rig.active(), &alpha));
}

/// A3. Twenty, and the entry that falls off is the tail-most one.
#[test]
fn the_recents_list_is_capped_at_twenty() {
	let rig = Rig::new();
	let mut created = Vec::new();
	for index in 0..21 {
		let path = rig.create(&format!("space{index:02}"));
		created.push(path);
	}

	let recents = rig.recents();
	assert_eq!(recents.len(), 20);
	assert_eq!(recents[0], created[20].to_string_lossy());
	// The default space bootstrap made is the oldest entry, so it is what the cap
	// evicted — and it is not the active one.
	assert!(index_points_at_the_open_space(&rig));
	assert!(!recents.iter().any(|entry| entry.ends_with("personal.copper")));
}

/// Spec 8.1b, relied on rather than reimplemented: a failed open leaves the
/// previous space open and unchanged, and the path it failed on is not listed.
#[test]
fn a_failed_open_changes_nothing() {
	let rig = Rig::new();
	let alpha = rig.create("alpha");
	rig.open(&alpha);
	let before = rig.recents();

	let broken = rig.root.join("broken.copper");
	std::fs::write(&broken, "<<<<<<< HEAD\nnot json\n").unwrap();

	let err = store::open_space(&rig.shared, &broken).unwrap_err();

	assert_eq!(err.kind(), "parse");
	assert_eq!(rig.active(), alpha);
	assert_eq!(rig.recents(), before);
	assert!(!rig.recents().iter().any(|entry| entry.contains("broken")));
}

/// A9. Availability is probed and never cached to disk, which is the whole
/// reason a space that comes back needs no repair step.
#[test]
fn an_entry_that_comes_back_opens_again_with_no_repair_step() {
	let rig = Rig::new();
	let alpha = rig.create("alpha");
	let beta = rig.create("beta");
	rig.open(&alpha);

	let text = std::fs::read_to_string(&beta).unwrap();
	std::fs::remove_file(&beta).unwrap();
	assert_eq!(
		unavailable_reason(&beta),
		Some(UnavailableReason::Missing),
		"a deleted file must report as missing, not as unreadable"
	);
	// Still listed. An entry is never dropped for being unavailable.
	assert!(rig.recents().iter().any(|entry| same_path(Path::new(entry), &beta)));

	std::fs::write(&beta, text).unwrap();

	assert_eq!(availability::probe(&RealFs, &beta).0, Availability::Available);
	rig.open(&beta);
	assert!(same_path(&rig.active(), &beta));
}

/// A6b. The comparison key is what dedupe, promotion and the already-active
/// check all use, and it has to work for a file that is not there.
#[test]
fn identity_is_lexical_and_needs_no_file() {
	let rig = Rig::new();
	let alpha = rig.create("alpha");
	rig.open(&alpha);
	let gone = rig.root.join("gone.copper");

	assert_eq!(comparison_key(&gone), comparison_key(&gone));
	assert!(!same_path(&gone, &alpha));
	// And the stored form never carries the verbatim prefix.
	assert!(rig.recents().iter().all(|entry| !entry.starts_with(r"\\?\")));
}

fn unavailable_reason(path: &Path) -> Option<UnavailableReason> {
	match availability::probe(&RealFs, path).0 {
		Availability::Unavailable { reason, .. } => Some(reason),
		_ => None,
	}
}
