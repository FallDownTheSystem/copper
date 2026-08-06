//! Phase 6 against real temp directories: what survives a restart, what the
//! recents list does when spaces are opened and forgotten, and the structural
//! rules that only the source can answer.
//!
//! Everything here runs without a Tauri runtime. `open_space_at` itself needs an
//! `AppHandle` and so is exercised by hand (see the task's verification list),
//! but the durability half of it — the store's recents bookkeeping across a
//! restart — is ordinary `cargo test` territory and is where the ordinary cases
//! actually get asserted.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use copper_lib::spaces::availability::{self, Availability, RealFs, UnavailableReason};
use copper_lib::spaces::paths::{comparison_key, same_path};
use copper_lib::store::events::RecordingSink;
use copper_lib::store::settings::Settings;
use copper_lib::store::{self, SharedStore};

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

// --- structural rules ---------------------------------------------------------

const LIB: &str = include_str!("../src/lib.rs");
const SPACES: &str = include_str!("../src/spaces/mod.rs");
const TRAY: &str = include_str!("../src/tray.rs");
const CAPTURE: &str = include_str!("../src/capture/mod.rs");

fn code(source: &str) -> String {
	source
		.lines()
		.filter(|line| !line.trim_start().starts_with("//"))
		.collect::<Vec<_>>()
		.join("\n")
}

/// The text from `body` up to the next top-level `fn`, so an assertion about
/// "inside this function" cannot silently read the one after it.
fn until_next_fn(body: &str) -> &str {
	&body[..body.find("\nfn ").unwrap_or(body.len())]
}

/// A document swap's offset, paired with the teardown it belongs to. Both
/// structural tests below locate the same pair, and a second copy of the panics
/// is a second thing to rewrite when a name changes.
fn swap_and_teardown(code: &str, swap: &str) -> (usize, usize) {
	let at = code
		.find(swap)
		.unwrap_or_else(|| panic!("{swap} is no longer called; this test needs rewriting"));
	let teardown = code[..at]
		.rfind("leave_current_space(")
		.unwrap_or_else(|| panic!("{swap} runs without leaving the outgoing space first"));
	(at, teardown)
}

/// A18. A correctness property, not a style choice: registering the plugin
/// lazily inside the setup closure — as several of Tauri's own examples do —
/// would put window creation before the instance check.
#[test]
fn single_instance_is_the_literal_first_plugin() {
	let code = code(LIB);
	let first = code
		.find(".plugin(")
		.expect("no plugin is registered at all");
	assert!(
		code[first..].starts_with(".plugin(tauri_plugin_single_instance::init("),
		"another plugin is registered before single-instance"
	);
	assert!(
		first < code.find(".setup(").expect("no setup closure"),
		"a plugin is registered after .setup()"
	);
	assert_eq!(
		code.matches("tauri_plugin_single_instance::init").count(),
		1,
		"the plugin is registered more than once"
	);
}

/// A27, for the path that is easy to forget: **creating** a space switches the
/// active document just as opening one does, so it owes the outgoing space's
/// editor sessions the same teardown. The ordering is what matters and it is not
/// reachable off-runtime — both entry points need an `AppHandle` — so it is
/// asserted structurally: every call that replaces the active document is
/// preceded by the teardown, under the guard that makes the pair indivisible.
#[test]
fn every_document_swap_leaves_the_current_space_first() {
	let code = code(SPACES);
	for swap in ["store::open_space(", "store::create_space("] {
		let (at, teardown) = swap_and_teardown(&code, swap);
		let guard = code[..at]
			.rfind("activation()")
			.unwrap_or_else(|| panic!("{swap} runs without the activation guard held"));
		assert!(
			guard < teardown,
			"{swap} leaves the current space before taking the guard, so an activation can interleave"
		);
	}
}

/// Task-011's teardown ends editor handoffs before it detaches the outgoing
/// space, or a handoff still writing its note back could have that note's
/// attachments collected out from under it.
#[test]
fn leaving_a_space_ends_handoffs_before_it_detaches_for_the_sweep() {
	let code = code(SPACES);
	let body = code
		.split("fn leave_current_space(")
		.nth(1)
		.expect("leave_current_space is gone; this test needs rewriting");
	let handoffs = body
		.find("end_handoffs_before_switching(")
		.expect("leaving a space no longer ends editor handoffs");
	let detach = body
		.find("detach_for_sweep(")
		.expect("leaving a space no longer captures the outgoing document");
	assert!(
		handoffs < detach,
		"the outgoing document is captured before editor handoffs have ended"
	);
	assert!(
		!until_next_fn(body).contains("crate::attachments::sweep("),
		"the sweep runs inside the teardown again, before the swap can fail"
	);
}

/// **The sweep must not run until the swap has succeeded.**
///
/// Sweeping first means a *failed* open leaves the outgoing space still active,
/// its session-scoped undo stack still alive, and the blobs those snapshots
/// reference already collected — which is exactly the "undo restores a note
/// whose attachments are gone" outcome the no-delete-on-mutation rule exists to
/// prevent. Asserted structurally, because neither entry point is reachable
/// without an `AppHandle`.
#[test]
fn the_sweep_runs_only_after_a_swap_has_succeeded() {
	let code = code(SPACES);
	for swap in ["store::open_space(", "store::create_space("] {
		// From the teardown this swap belongs to, up to the swap itself. Scoped to
		// that window rather than to the whole file above the swap, which would
		// also see the *other* entry point's sweep.
		let (at, teardown) = swap_and_teardown(&code, swap);
		assert!(
			!code[teardown..at].contains("sweep_detached("),
			"{swap} sweeps between leaving and swapping, so a failed open collects a live space's \
			 blobs"
		);
		let after = &code[at..];
		let sweep = after
			.find("sweep_detached(")
			.unwrap_or_else(|| panic!("{swap} never sweeps the space it replaced"));
		// Within the same function: the next `fn` must come after the sweep.
		assert!(
			sweep < until_next_fn(after).len(),
			"{swap}'s sweep landed in a different function"
		);
	}
}

/// The sweep policy in one assertion: after a space swap, and at startup, and
/// nowhere else. A third caller would almost certainly be a mid-session one,
/// which is what makes an undo unrestorable.
#[test]
fn attachments_are_swept_only_after_a_swap_and_at_startup() {
	let code = code(SPACES);
	assert_eq!(
		code.matches("sweep_detached(").count(),
		// Once in its own definition, once per swap site.
		3,
		"a swap site is missing its sweep, or something else sweeps"
	);
	assert_eq!(
		code.matches("sweep_active_space(").count(),
		1,
		"the active space is swept from somewhere other than startup"
	);

	// Startup's sweep is off the setup() path: enumerating an assets directory on
	// a network share must not sit between launch and the first capture.
	let body = code
		.split("fn start_dispatcher(")
		.nth(1)
		.expect("start_dispatcher is gone");
	let scope = until_next_fn(body);
	let spawn = scope.find("thread::spawn").expect("the startup sweep is not on a thread");
	let sweep = scope
		.find("sweep_active_space(")
		.expect("start_dispatcher no longer sweeps");
	assert!(spawn < sweep, "the startup sweep runs inline on the setup path");
}

/// A30. Nothing switches the active space on its own, so the activate path may
/// not have callers outside the layer that owns the policy — the switcher's
/// command, the cold argv open and the dispatcher's host are all here.
///
/// What this proves is narrower than "only the switcher can activate a space",
/// and the difference is worth stating rather than implying. The store's
/// `open_space` **command** stays registered and reachable from the webview, and
/// one caller uses it deliberately: task-004's error-state retry re-opens by path
/// (`useSpace.ts`, `retry()`), because `get_active_space` returns the in-memory
/// document and would appear to succeed while rereading nothing. That is a named
/// exception, not a hole. What the assertions below actually establish is that no
/// *Rust* module outside `spaces/` reaches either entry point.
#[test]
fn the_activate_path_has_no_callers_outside_the_spaces_layer() {
	for (name, source) in [
		("lib.rs", LIB),
		("tray.rs", TRAY),
		("capture/mod.rs", CAPTURE),
		("editor.rs", include_str!("../src/editor.rs")),
		("store/commands.rs", include_str!("../src/store/commands.rs")),
	] {
		let code = code(source);
		assert!(
			!code.contains("open_space_at("),
			"{name} activates a space without going through the spaces layer"
		);
		// `store/commands.rs` defines the command and calls `super::open_space`; the
		// fully-qualified spelling is what another module would have to write.
		assert!(
			!code.contains("store::open_space("),
			"{name} reaches the store's open command behind the spaces layer's back"
		);
	}
	// And within the layer, exactly one delegation to the store: a second would be
	// a second implementation of opening.
	assert_eq!(
		code(SPACES).matches("store::open_space(").count(),
		1,
		"the spaces layer delegates to the store's open in more than one place"
	);
	// The frontend's one legitimate direct use, asserted so that removing it is a
	// deliberate act and adding a second one is visible.
	let frontend = include_str!("../../src/composables/useSpace.ts");
	assert_eq!(
		frontend.matches("invoke<Space>('open_space'").count(),
		1,
		"the re-open-by-path retry is the only frontend caller of the store's open"
	);
}
