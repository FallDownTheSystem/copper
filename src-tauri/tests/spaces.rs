//! The structural rules that only the source can answer.
//!
//! These read `src/` as text, because what they assert is an *ordering* between
//! calls that no runtime-free test can reach: both entry points that swap the
//! active document need an `AppHandle`. They stay in this crate because the
//! sources they read are this crate's — `lib.rs`, `spaces/mod.rs`, `tray.rs`,
//! `capture/mod.rs` — and because the policy they protect is the app's.
//!
//! The durability half of the old `tests/spaces.rs` — recents order across a
//! restart, path identity, availability classification — is
//! `copper-core/tests/spaces.rs` now, with the modules it exercises.
//!
//! One narrowing is worth stating rather than discovering: `crate_sources()`
//! walks this crate's `src/` only, so the sweep census below no longer sees
//! `copper-core`. That is sound rather than lucky — the sweep primitive itself
//! lives here, in `attachments/sweep.rs`, and `copper-core` has no way to call
//! it — but a future move of the sweep would have to move this test with it.

use std::path::PathBuf;

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

/// Every `.rs` file under `src/`, keyed by its path relative to `src/`, with
/// comment lines and the `#[cfg(test)]` tail removed.
///
/// The `include_str!` constants above cover the four modules whose *ordering*
/// matters. A census — "nothing anywhere else does this" — cannot be written
/// against a hand-listed four, because the whole claim is about the files
/// nobody thought to list.
///
/// The test tail goes because a unit test calling a function it is testing is
/// not a caller in the sense any of these rules mean. Truncating at a
/// column-zero `#[cfg(test)]` is exact for this crate, where every test module
/// is the last item in its file.
fn crate_sources() -> Vec<(String, String)> {
	let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
	let mut sources = Vec::new();
	let mut stack = vec![root.clone()];

	while let Some(dir) = stack.pop() {
		for entry in std::fs::read_dir(&dir).expect("src/ is unreadable") {
			let path = entry.expect("unreadable directory entry").path();
			if path.is_dir() {
				stack.push(path);
			} else if path.extension().is_some_and(|extension| extension == "rs") {
				let name = path
					.strip_prefix(&root)
					.unwrap_or(&path)
					.to_string_lossy()
					.replace('\\', "/");
				let text = std::fs::read_to_string(&path).expect("unreadable source file");
				let body = text.split("\n#[cfg(test)]").next().unwrap_or(&text);
				sources.push((name, code(body)));
			}
		}
	}

	sources.sort();
	sources
}

/// Calls to the bare `attachments::sweep`, however it was brought into scope.
///
/// Matching the call rather than any one spelling of the path is the point: a
/// `use crate::attachments::sweep;` followed by a bare `sweep(...)` is the way
/// a new caller would slip past a check written against `crate::attachments::`.
/// The two exclusions are the definition itself and longer names that merely
/// end in the same word — `detach_for_sweep(` being the one that actually
/// exists.
fn calls_to_sweep(source: &str) -> usize {
	source
		.match_indices("sweep(")
		.filter(|(at, _)| {
			let before = &source[..*at];
			!before.ends_with("fn ")
				&& before
					.chars()
					.next_back()
					.is_none_or(|previous| !previous.is_alphanumeric() && previous != '_')
		})
		.count()
}

/// Whether `line` opens a top-level `fn`, its modifiers included.
///
/// The modifiers are what makes this more than a `starts_with("fn ")`. A scope
/// that ended only at a bare `fn` ran straight through the next
/// `pub(crate) fn` and read a neighbouring function's body as if it were part
/// of the one being measured — which is a test that quietly stops asserting
/// what it says it asserts.
///
/// Indentation is the other half: an `impl` block's methods and a nested
/// closure are inside the scope, not boundaries of it.
fn opens_a_fn(line: &str) -> bool {
	let mut rest = line;
	loop {
		if rest.starts_with("fn ") {
			return true;
		}
		let Some((word, tail)) = rest.split_once(' ') else {
			return false;
		};
		if !matches!(word, "pub" | "pub(crate)" | "pub(super)" | "async" | "unsafe" | "const") {
			return false;
		}
		rest = tail;
	}
}

/// The text from `body` up to the next top-level `fn`, so an assertion about
/// "inside this function" cannot silently read the one after it.
///
/// `body` always starts part-way through a function, so its first line is never
/// itself a boundary.
fn until_next_fn(body: &str) -> &str {
	let mut offset = 0;
	for line in body.split_inclusive('\n') {
		if offset > 0 && opens_a_fn(line) {
			return &body[..offset];
		}
		offset += line.len();
	}
	body
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
	// Bounded first, and every `find` below is against the bounded text. Both
	// names are also *definitions* a dozen lines further down, so an unbounded
	// search would match those and keep reporting a correct order for a teardown
	// that had stopped calling either of them.
	let scope = until_next_fn(body);
	let handoffs = scope
		.find("end_handoffs_before_switching(")
		.expect("leaving a space no longer ends editor handoffs");
	let detach = scope
		.find("detach_for_sweep(")
		.expect("leaving a space no longer captures the outgoing document");
	assert!(
		handoffs < detach,
		"the outgoing document is captured before editor handoffs have ended"
	);
	assert!(
		!scope.contains("crate::attachments::sweep("),
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

/// **The sweep primitive has exactly one caller in the whole crate**, and that
/// is what turns `attachments::sweep`'s "at space close and at startup only,
/// never mid-session" from a description into a rule.
///
/// The counts above are per-module and stay: they say each of the two swap
/// sites and startup reaches the sweep through the right door, in the right
/// order. This says there is no other door. A mid-session caller added anywhere
/// else in the crate — the one that silently turns a restorable `Ctrl+Z` into a
/// note whose attachments are gone — satisfies every other assertion in this
/// file, because none of them looks outside `spaces/mod.rs`.
#[test]
fn the_sweep_primitive_has_exactly_one_caller_in_the_crate() {
	let callers: Vec<String> = crate_sources()
		.into_iter()
		.filter(|(_, source)| calls_to_sweep(source) > 0)
		.flat_map(|(name, source)| std::iter::repeat_n(name, calls_to_sweep(&source)))
		.collect();

	assert_eq!(
		callers,
		["spaces/mod.rs"],
		"the sweep primitive is called from somewhere other than spaces::sweep_detached"
	);

	// And within that file it is `sweep_detached`, not merely somewhere.
	let code = code(SPACES);
	let body = code
		.split("fn sweep_detached(")
		.nth(1)
		.expect("sweep_detached is gone; this test needs rewriting");
	assert!(
		calls_to_sweep(until_next_fn(body)) == 1,
		"sweep_detached no longer calls the sweep primitive"
	);
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
	const WRAPPERS: &str = include_str!("../src/store/commands.rs");

	for (name, source) in [
		("lib.rs", LIB),
		("tray.rs", TRAY),
		("capture/mod.rs", CAPTURE),
		("editor.rs", include_str!("../src/editor.rs")),
		("store/commands.rs", WRAPPERS),
	] {
		let code = code(source);
		assert!(
			!code.contains("open_space_at("),
			"{name} activates a space without going through the spaces layer"
		);
	}

	// `store/commands.rs` is the one file outside the layer that reaches the
	// store's open at all, because it *is* the command — the webview's re-open-by-
	// path retry has to land somewhere. It was previously kept out of the check
	// above by spelling the call `super::open_space`, which stopped being possible
	// when the store moved to `copper-core` and `super` stopped naming it. So the
	// exception is now stated rather than arranged: exactly one delegation, in the
	// file that defines the command.
	assert_eq!(
		code(WRAPPERS).matches("store::open_space(").count(),
		1,
		"the command wrappers reach the store's open more than once"
	);
	for (name, source) in [
		("lib.rs", LIB),
		("tray.rs", TRAY),
		("capture/mod.rs", CAPTURE),
		("editor.rs", include_str!("../src/editor.rs")),
	] {
		assert!(
			!code(source).contains("store::open_space("),
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
