//! The IPC surface: what is registered, and how it is spelled.
//!
//! `tests/store_fs.rs` calls `Store` methods directly and never crosses the IPC
//! boundary, so it would pass unchanged if a command were defined but never
//! registered, or if a parameter grew an underscore and quietly acquired two
//! spellings on the JavaScript side. Those are the failures this file catches,
//! and `doc-store-api.md` is the contract it restates.
//!
//! **Why not Tauri's mock runtime.** `tauri::test::mock_builder` would let the
//! real invoke handler be driven end to end, which is strictly better. It does
//! not work here: referencing `WebviewWindowBuilder` pulls WebView2's COM
//! imports into the test binary, and the resulting executable will not load
//! from `target/debug/deps` because `WebView2Loader.dll` is not beside it
//! (`STATUS_ENTRYPOINT_NOT_FOUND`). Hand-copying a DLL into a build directory
//! would make the suite pass here and fail on a clean checkout, which is worse
//! than not having it. The two guarantees that approach would have added —
//! registration and argument spelling — are checked against the source below
//! instead, and every result shape is checked through serde, which is the same
//! code that runs over the real boundary.

use copper_lib::store::commands::AddNoteResult;
use copper_lib::store::error::StoreError;
use copper_lib::store::model::{Section, Space};
use copper_lib::store::settings::Settings;
use copper_lib::store::StoreStatus;

/// Spec 8.1, verbatim. Twenty.
const COMMANDS: [&str; 20] = [
	"get_settings",
	"update_settings",
	"get_status",
	"get_active_space",
	"open_space",
	"create_space",
	"add_note",
	"edit_note",
	"set_notes_done",
	"delete_notes",
	"reorder_note",
	"move_notes",
	"merge_notes",
	"add_section",
	"rename_section",
	"delete_section",
	"reorder_section",
	"set_active_section",
	"undo",
	"redo",
];

/// The commands later phases added beside the store's twenty.
const EXTRA_COMMANDS: [&str; 11] = [
	"clipboard_write_text",
	"editor_handoffs",
	"editor_open_note",
	"editor_stop_handoff",
	"editor_reconcile",
	// Phase 6. Note what is *not* here: no `open_space` or `create_space` of its
	// own. Those are the store's, and the spaces layer wraps them rather than
	// growing a second way to open a document.
	"list_recents",
	"refresh_recents",
	"activate_space",
	"pick_and_open_space",
	"create_space_interactive",
	"remove_recent",
];

/// Spec 8.1c. Every argument name in the whole surface.
const PARAMETERS: [&str; 10] = [
	"patch", "path", "name", "body", "section", "id", "ids", "done", "index", "text",
];

const SOURCE: &str = include_str!("../src/store/commands.rs");

/// Command *wrappers* live next to the module they serve; only the registration
/// is central, because Tauri accepts one `invoke_handler` and the closure
/// `generate_handler!` builds consumes the `Invoke` it is handed.
const OTHER_SOURCES: [&str; 3] = [
	include_str!("../src/clipboard.rs"),
	include_str!("../src/editor.rs"),
	include_str!("../src/spaces/mod.rs"),
];

const REGISTRY: &str = include_str!("../src/commands.rs");

/// Every `#[tauri::command]` in the module, as `(name, parameter names)`.
///
/// `state` and `app` are dropped: both are injected by Tauri and never appear on
/// the JavaScript side.
fn defined_commands() -> Vec<(String, Vec<String>)> {
	commands_in(SOURCE)
}

/// Every command the crate defines, wherever its wrapper lives.
fn all_defined_commands() -> Vec<(String, Vec<String>)> {
	let mut commands = defined_commands();
	for source in OTHER_SOURCES {
		commands.extend(commands_in(source));
	}
	commands
}

fn commands_in(source: &str) -> Vec<(String, Vec<String>)> {
	let mut commands = Vec::new();
	// The attribute on a line of its own, not the bare token: prose mentioning
	// `#[tauri::command]` in a module doc is not a command definition, and
	// matching it would turn the guard below into a failure about a comment.
	for block in source.split("\n#[tauri::command]\n").skip(1) {
		let Some(signature) = block.split_once("pub async fn ") else {
			panic!("a #[tauri::command] is not followed by `pub async fn`");
		};
		let (name, rest) = signature
			.1
			.split_once('(')
			.expect("a command signature has no argument list");

		let arguments = rest
			.split_once(')')
			.expect("a command signature has no closing parenthesis")
			.0;
		let parameters = arguments
			.split(',')
			.filter_map(|argument| argument.split_once(':'))
			.map(|(name, _)| name.trim().to_string())
			.filter(|name| !name.is_empty() && name != "state" && name != "app")
			.collect();

		commands.push((name.trim().to_string(), parameters));
	}
	commands
}

/// The names inside `generate_handler!`, with their module paths stripped.
fn registered_commands() -> Vec<String> {
	let block = REGISTRY
		.split_once("tauri::generate_handler![")
		.expect("no generate_handler! block")
		.1
		.split_once(']')
		.expect("the generate_handler! block is not closed")
		.0;
	block
		.split(',')
		.map(str::trim)
		.filter(|name| !name.is_empty())
		.map(|path| path.rsplit("::").next().unwrap_or(path).to_string())
		.collect()
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
	names.sort();
	names
}

/// Defining a command and registering it are separate acts, and only the second
/// one makes it callable. Nothing else in the suite would notice the difference.
#[test]
fn every_defined_command_is_registered_and_matches_the_documented_twenty() {
	let store_defined: Vec<String> = defined_commands().into_iter().map(|(name, _)| name).collect();
	let defined: Vec<String> = all_defined_commands()
		.into_iter()
		.map(|(name, _)| name)
		.collect();
	let registered = registered_commands();
	let documented: Vec<String> = COMMANDS
		.iter()
		.chain(EXTRA_COMMANDS.iter())
		.map(|name| name.to_string())
		.collect();

	assert_eq!(
		sorted(defined),
		sorted(registered.clone()),
		"a command is defined but not registered, or registered but not defined"
	);
	assert_eq!(
		sorted(registered),
		sorted(documented),
		"the command surface has drifted from spec 8.1 and doc-store-api.md"
	);
	assert_eq!(
		store_defined.len(),
		20,
		"the store's own surface is no longer twenty commands"
	);
}

/// Spec 8.1c. Tauri converts snake_case argument names to camelCase on the
/// JavaScript side, so a multi-word parameter would have two spellings and the
/// contract would depend on which one Phase 3 guessed. Keeping every one of them
/// a single word makes the conversion a no-op — which stays true only for as
/// long as something checks.
#[test]
fn every_command_parameter_is_a_single_word() {
	for (command, parameters) in all_defined_commands() {
		for parameter in parameters {
			assert!(
				!parameter.contains('_'),
				"{command}'s parameter `{parameter}` is multi-word, so its JavaScript spelling \
				 differs from its Rust one. Either rename it or document the camelCase form in \
				 doc-store-api.md (spec 8.1c)."
			);
			assert!(
				parameter.chars().all(|c| c.is_ascii_lowercase()),
				"{command}'s parameter `{parameter}` is not plain lowercase"
			);
			assert!(
				PARAMETERS.contains(&parameter.as_str()),
				"{command} introduced the parameter `{parameter}`, which is not in the documented \
				 set {PARAMETERS:?}"
			);
		}
	}
}

// --- the shapes that cross the boundary --------------------------------------

fn space() -> Space {
	Space {
		id: "spc_00000001".into(),
		name: "test".into(),
		active_section: "sec_00000001".into(),
		sections: vec![Section {
			id: "sec_00000001".into(),
			name: "Notes".into(),
			order: 0,
		}],
		notes: Vec::new(),
	}
}

/// The only camelCase conversion in the whole surface, and therefore the only
/// one that can be got wrong silently.
#[test]
fn add_note_returns_note_id_in_camel_case() {
	let payload = serde_json::to_value(AddNoteResult {
		space: space(),
		note_id: "nte_00000001".into(),
	})
	.unwrap();

	assert!(payload.get("noteId").is_some(), "add_note must return noteId: {payload}");
	assert!(payload.get("note_id").is_none(), "the snake_case spelling leaked over IPC");
	assert_eq!(payload.as_object().unwrap().len(), 2);
	assert!(payload["space"].get("activeSection").is_some());
}

#[test]
fn store_status_crosses_the_boundary_in_camel_case() {
	let payload = serde_json::to_value(StoreStatus {
		path: Some("C:\\notes.copper".into()),
		errored: false,
		watching: true,
		can_undo: true,
		can_redo: false,
		startup_notice: None,
	})
	.unwrap();

	for key in ["path", "errored", "watching", "canUndo", "canRedo", "startupNotice"] {
		assert!(payload.get(key).is_some(), "get_status is missing {key}: {payload}");
	}
	assert_eq!(payload.as_object().unwrap().len(), 6, "get_status grew a field");
	assert!(payload["startupNotice"].is_null());
}

#[test]
fn settings_cross_the_boundary_in_camel_case() {
	let payload = serde_json::to_value(Settings::default()).unwrap();

	for key in ["recents", "activeSpace", "panelPosition", "shortcuts", "theme"] {
		assert!(payload.get(key).is_some(), "get_settings is missing {key}: {payload}");
	}
	assert_eq!(payload.as_object().unwrap().len(), 5, "get_settings grew a field");
	assert_eq!(payload["shortcuts"]["capture"], "Shift Shift");
	assert_eq!(payload["shortcuts"]["summon"], "Ctrl+Shift+Space");
}

#[test]
fn a_space_crosses_the_boundary_in_camel_case() {
	// Asserted against the serialised text rather than `to_value`, which collects
	// into a sorted map and so cannot show declaration order at all.
	let text = serde_json::to_string(&space()).unwrap();

	let mut previous = 0;
	for key in ["\"id\"", "\"name\"", "\"activeSection\"", "\"sections\"", "\"notes\""] {
		let at = text
			.find(key)
			.unwrap_or_else(|| panic!("{key} is missing from {text}"));
		assert!(at >= previous, "{key} is out of declaration order in {text}");
		previous = at;
	}
	assert!(!text.contains("active_section"), "the snake_case spelling leaked over IPC");
}

/// Spec 8.6. The frontend branches on `kind`, so every variant has to arrive as
/// the same flat shape rather than as serde's default enum encoding.
#[test]
fn every_error_kind_crosses_the_boundary_as_kind_and_message() {
	let errors = [
		(StoreError::NotFound("no such note".into()), "not-found"),
		(StoreError::Io("could not write".into()), "io"),
		(StoreError::Parse("not a document".into()), "parse"),
		(StoreError::Conflict("kept changing".into()), "conflict"),
		(StoreError::Invalid("a note cannot be empty".into()), "invalid"),
		(StoreError::Unavailable("no space is open".into()), "unavailable"),
	];

	for (error, kind) in errors {
		let message = error.message();
		let payload = serde_json::to_value(&error).unwrap();
		assert_eq!(payload["kind"], kind);
		assert_eq!(payload["message"], message);
		assert_eq!(payload.as_object().unwrap().len(), 2, "{kind} carries extra fields");
	}
}

/// `undo` and `redo` return `Space | null`, not an absent value.
#[test]
fn an_empty_undo_stack_serialises_to_null() {
	let empty: Option<Space> = None;
	assert_eq!(serde_json::to_value(empty).unwrap(), serde_json::Value::Null);
	assert!(serde_json::to_value(Some(space())).unwrap().is_object());
}
