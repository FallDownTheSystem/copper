//! The CLI at the process boundary: real argv, real exit codes, real stdout.
//!
//! Everything below runs the built `copper.exe`. That is the point — the unit
//! tests in `copper-core` already prove the pipeline, and what these add is the
//! layer above it: the resolution chain, the exit-code map, the JSON shapes and
//! the clap grammar, none of which exist below `main`.
//!
//! **Every child gets its own `APPDATA`.** `settings.json` and `cli-state.json`
//! are both located from that variable, so overriding it per child is what keeps
//! the suite from reading — or, for `space use`, writing — the developer's own
//! Copper configuration. `COPPER_SPACE` is removed for the same reason: a
//! developer with it set in their shell would otherwise get different results
//! than CI.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use copper_core::store::settings;

const GOLDEN: &str = include_str!("../../copper-core/tests/fixtures/space-golden.copper");
/// The golden fixture's last section. A note added here is appended at the end
/// of the notes array, which is what the minimal-diff assertion describes.
const LAST_SECTION: &str = "Configuration Formats";

/// A temp directory holding a fake `%APPDATA%` and a working directory.
struct Cli {
	_dir: tempfile::TempDir,
	appdata: PathBuf,
	cwd: PathBuf,
}

impl Cli {
	fn new() -> Self {
		let dir = tempfile::tempdir().unwrap();
		let appdata = dir.path().join("AppData");
		let cwd = dir.path().join("work");
		std::fs::create_dir_all(&appdata).unwrap();
		std::fs::create_dir_all(&cwd).unwrap();
		Self {
			_dir: dir,
			appdata,
			cwd,
		}
	}

	/// The constants rather than the literals, so this fixture follows the
	/// directory the CLI actually reads. `copper-core` already pins
	/// `APP_IDENTIFIER` to `tauri.conf.json`'s value in a test of its own, which
	/// is where that assertion belongs.
	fn config_dir(&self) -> PathBuf {
		self.appdata.join(settings::APP_IDENTIFIER)
	}

	fn settings_path(&self) -> PathBuf {
		self.config_dir().join(settings::FILE_NAME)
	}

	/// Writes a `settings.json` whose `recents` are the given paths.
	fn write_settings(&self, recents: &[&Path]) -> PathBuf {
		std::fs::create_dir_all(self.config_dir()).unwrap();
		let entries: Vec<String> = recents
			.iter()
			.map(|path| path.to_string_lossy().into_owned())
			.collect();
		let path = self.settings_path();
		let document = serde_json::json!({
			"recents": entries,
			"activeSpace": 0,
			"theme": "dark",
		});
		std::fs::write(&path, format!("{document}\n")).unwrap();
		path
	}

	fn command(&self) -> Command {
		// `copper-cli`, the cargo target name. What the user types is `copper` —
		// clap's own `name`, and the filename the installer ships it under.
		let mut command = Command::new(env!("CARGO_BIN_EXE_copper-cli"));
		command
			.current_dir(&self.cwd)
			.env("APPDATA", &self.appdata)
			.env_remove("COPPER_SPACE");
		command
	}

	fn run(&self, args: &[&str]) -> Run {
		Run::of(self.command().args(args).output().unwrap())
	}

	/// `run`, with `--space <path>` in front of every invocation.
	///
	/// Twelve tests below own a space and address it on every call. One copy of
	/// the argument assembly means each of them reads as the commands it runs.
	fn at<'a>(&'a self, space: &'a Path) -> impl Fn(&[&str]) -> Run + 'a {
		move |args| {
			let mut all = vec!["--space", space.to_str().unwrap()];
			all.extend_from_slice(args);
			self.run(&all)
		}
	}

	/// A space file in the working directory, created through the CLI itself.
	fn space(&self, name: &str) -> PathBuf {
		let path = self.cwd.join(format!("{name}.copper"));
		self.run(&["space", "create", path.to_str().unwrap()]).ok();
		path
	}

	/// A copy of the golden fixture in the working directory.
	fn golden(&self) -> PathBuf {
		let path = self.cwd.join("golden.copper");
		std::fs::write(&path, GOLDEN).unwrap();
		path
	}
}

struct Run {
	code: i32,
	stdout: String,
	stderr: String,
}

impl Run {
	fn of(output: Output) -> Self {
		Self {
			code: output.status.code().unwrap_or(-1),
			stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
			stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
		}
	}

	fn ok(&self) -> &Self {
		assert_eq!(self.code, 0, "stderr: {}", self.stderr);
		self
	}

	fn failed(&self, code: i32) -> &Self {
		assert_eq!(self.code, code, "stdout: {}\nstderr: {}", self.stdout, self.stderr);
		self
	}

	fn out(&self) -> &str {
		self.stdout.trim_end_matches(['\r', '\n'])
	}

	fn json(&self) -> serde_json::Value {
		serde_json::from_str(&self.stdout)
			.unwrap_or_else(|err| panic!("stdout is not JSON ({err}): {:?}", self.stdout))
	}
}

// --- the resolution chain --------------------------------------------------------

/// The acceptance criterion: with nothing set anywhere, the refusal names all
/// four places Copper looked, in order.
#[test]
fn no_space_anywhere_exits_four_and_names_the_whole_chain() {
	let cli = Cli::new();

	let run = cli.run(&["note", "list"]);

	run.failed(4);
	assert!(run.stderr.starts_with("unavailable:"), "{}", run.stderr);
	for step in ["--space", "COPPER_SPACE", "copper space use", "settings.json"] {
		assert!(run.stderr.contains(step), "{step} unnamed in: {}", run.stderr);
	}
}

/// An empty `recents` is the same answer as no settings file: the fourth step
/// exists but names nothing.
#[test]
fn an_empty_recents_still_exits_four() {
	let cli = Cli::new();
	cli.write_settings(&[]);

	cli.run(&["note", "list"]).failed(4);
}

#[test]
fn the_flag_outranks_the_environment_which_outranks_the_state_file() {
	let cli = Cli::new();
	let selected = cli.space("selected");
	let from_env = cli.space("from-env");
	let from_flag = cli.space("from-flag");

	cli.run(&["space", "use", selected.to_str().unwrap()]).ok();
	cli.run(&["note", "add", "in the selected space"]).ok();
	assert_eq!(cli.run(&["note", "list"]).out().matches("selected").count(), 1);

	// The variable beats the state file.
	let with_env = Run::of(
		cli.command()
			.env("COPPER_SPACE", &from_env)
			.args(["note", "add", "in the env space"])
			.output()
			.unwrap(),
	);
	with_env.ok();
	assert!(std::fs::read_to_string(&from_env).unwrap().contains("in the env space"));

	// The flag beats both.
	let with_flag = Run::of(
		cli.command()
			.env("COPPER_SPACE", &from_env)
			.args(["--space", from_flag.to_str().unwrap()])
			.args(["note", "add", "in the flag space"])
			.output()
			.unwrap(),
	);
	with_flag.ok();
	assert!(std::fs::read_to_string(&from_flag).unwrap().contains("in the flag space"));
	assert!(
		!std::fs::read_to_string(&from_env).unwrap().contains("in the flag space"),
		"the flag did not outrank the variable"
	);
}

/// The app's active space is the last resort, and reaching it must not change it.
#[test]
fn the_apps_active_space_is_the_final_fallback() {
	let cli = Cli::new();
	let space = cli.space("app");
	cli.write_settings(&[&space]);

	cli.run(&["note", "add", "via the app's active space"]).ok();

	assert!(std::fs::read_to_string(&space).unwrap().contains("via the app"));
}

/// Relative paths resolve against the invocation's working directory, not against
/// the config directory or anything else.
#[test]
fn a_relative_space_path_resolves_against_the_working_directory() {
	let cli = Cli::new();
	cli.run(&["space", "create", "relative.copper"]).ok();
	assert!(cli.cwd.join("relative.copper").is_file());

	cli.run(&["--space", "relative.copper", "note", "add", "here"]).ok();
	assert!(std::fs::read_to_string(cli.cwd.join("relative.copper"))
		.unwrap()
		.contains("here"));
}

/// `space use` stores an absolute path, so a later invocation from anywhere else
/// still means the same file.
#[test]
fn the_state_file_holds_an_absolute_path() {
	let cli = Cli::new();
	cli.run(&["space", "create", "state.copper"]).ok();
	cli.run(&["space", "use", "state.copper"]).ok();

	let state = std::fs::read_to_string(cli.config_dir().join("cli-state.json")).unwrap();
	let parsed: serde_json::Value = serde_json::from_str(&state).unwrap();
	let stored = parsed["space"].as_str().expect("a stored path");
	assert!(Path::new(stored).is_absolute(), "{stored}");

	// From a different directory, the selection still resolves.
	let elsewhere = cli.cwd.join("nested");
	std::fs::create_dir_all(&elsewhere).unwrap();
	let run = Run::of(
		cli.command()
			.current_dir(&elsewhere)
			.args(["note", "add", "from elsewhere"])
			.output()
			.unwrap(),
	);
	run.ok();
	assert!(std::fs::read_to_string(cli.cwd.join("state.copper"))
		.unwrap()
		.contains("from elsewhere"));
}

#[test]
fn space_clear_deletes_the_state_file_and_falls_through() {
	let cli = Cli::new();
	cli.run(&["space", "create", "cleared.copper"]).ok();
	cli.run(&["space", "use", "cleared.copper"]).ok();
	assert!(cli.config_dir().join("cli-state.json").exists());

	cli.run(&["space", "clear"]).ok();

	assert!(!cli.config_dir().join("cli-state.json").exists());
	assert_eq!(cli.run(&["space", "current"]).ok().out(), "No space is selected for the CLI.");
	// Nothing else in the chain is set, so the chain runs out.
	cli.run(&["note", "list"]).failed(4);
}

/// A state file whose `space` is not a rooted path did not come from `space use`,
/// which only ever writes absolute ones.
///
/// Honouring it would be worse than ignoring it. `""` joins onto the working
/// directory and resolves to the directory itself; a relative entry names a
/// different file from every directory the user runs in, so a selection that is
/// supposed to be durable quietly becomes one that is not.
#[test]
fn a_state_entry_that_is_not_an_absolute_path_falls_through() {
	let cli = Cli::new();
	std::fs::create_dir_all(cli.config_dir()).unwrap();
	cli.run(&["space", "create", "sideways.copper"]).ok();

	// `C:foo` and `\foo` are the two a naive "is it rooted" check lets through:
	// `paths::is_rooted` says yes to both, because its job is to stop `join` from
	// discarding a base — but `C:foo` resolves against drive C's own current
	// directory and `\foo` against whatever the current drive is, so neither names
	// a fixed file, which is the whole contract of a stored selection.
	for entry in [
		"",
		"sideways.copper",
		".\\sideways.copper",
		"C:sideways.copper",
		"\\sideways.copper",
	] {
		std::fs::write(
			cli.config_dir().join("cli-state.json"),
			format!("{{\"space\": {}}}\n", serde_json::to_string(entry).unwrap()),
		)
		.unwrap();

		// Nothing else in the chain is set, so ignoring the entry runs it out.
		cli.run(&["note", "list"]).failed(4);
		assert_eq!(
			cli.run(&["space", "current"]).ok().out(),
			"No space is selected for the CLI.",
			"{entry:?} was reported as a selection"
		);
	}
}

#[test]
fn a_corrupt_state_file_is_ignored_rather_than_repaired() {
	let cli = Cli::new();
	std::fs::create_dir_all(cli.config_dir()).unwrap();
	let state = cli.config_dir().join("cli-state.json");
	std::fs::write(&state, "{ not json").unwrap();

	cli.run(&["note", "list"]).failed(4);

	assert_eq!(
		std::fs::read_to_string(&state).unwrap(),
		"{ not json",
		"a corrupt state file was rewritten"
	);
}

// --- space list ------------------------------------------------------------------

/// The acceptance criterion: a listing command has no side effects, so running it
/// twice leaves the file it read byte-identical.
#[test]
fn space_list_run_twice_leaves_settings_byte_identical() {
	let cli = Cli::new();
	let present = cli.space("present");
	let settings = cli.write_settings(&[&present, Path::new("D:\\gone\\missing.copper")]);
	let before = std::fs::read(&settings).unwrap();

	cli.run(&["space", "list"]).ok();
	cli.run(&["space", "list"]).ok();

	assert_eq!(std::fs::read(&settings).unwrap(), before, "settings.json was rewritten");
	assert_eq!(
		std::fs::read_dir(cli.config_dir()).unwrap().count(),
		1,
		"a listing created a second file"
	);
}

/// The stronger form of the same rule: a `settings.json` that is not valid JSON
/// is *renamed* by `settings::load`, and a listing must use the loader that does
/// not.
#[test]
fn space_list_does_not_quarantine_an_unreadable_settings_file() {
	let cli = Cli::new();
	std::fs::create_dir_all(cli.config_dir()).unwrap();
	std::fs::write(cli.settings_path(), "{ not json").unwrap();

	cli.run(&["space", "list"]).ok();

	assert_eq!(std::fs::read_to_string(cli.settings_path()).unwrap(), "{ not json");
	assert_eq!(
		std::fs::read_dir(cli.config_dir()).unwrap().count(),
		1,
		"the file was set aside"
	);
}

#[test]
fn space_list_classifies_each_entry() {
	let cli = Cli::new();
	let present = cli.space("present");
	cli.write_settings(&[&present, Path::new("D:\\gone\\missing.copper")]);

	let run = cli.run(&["--json", "space", "list"]);
	run.ok();
	let spaces = run.json();
	let rows = spaces["spaces"].as_array().expect("an array");

	assert_eq!(rows.len(), 2);
	assert_eq!(rows[0]["active"], true);
	assert_eq!(rows[0]["name"], "present");
	assert_eq!(rows[0]["availability"]["state"], "available");
	assert_eq!(rows[1]["active"], false);
	assert_eq!(rows[1]["availability"]["state"], "unavailable");
	assert!(rows[1]["availability"]["message"].is_string());
}

/// `name` means "what this document calls itself", so an entry whose file could
/// not be read has none. The human listing substitutes the file stem so a row
/// still reads as something; the JSON does not, because a stem is a guess from
/// the `path` a consumer already has.
#[test]
fn an_unavailable_entry_has_a_null_name_in_json_and_a_stem_in_text() {
	let cli = Cli::new();
	cli.write_settings(&[Path::new("D:\\gone\\missing.copper")]);

	let json = cli.run(&["--json", "space", "list"]);
	json.ok();
	assert_eq!(json.json()["spaces"][0]["name"], serde_json::Value::Null);

	let text = cli.run(&["space", "list"]);
	text.ok();
	assert!(text.out().contains("missing"), "{}", text.out());
}

// --- writing ---------------------------------------------------------------------

/// The acceptance criterion, end to end: a note added through the binary changes
/// exactly its own nine lines plus the preceding brace's comma.
#[test]
fn a_note_added_through_the_binary_produces_a_minimal_diff() {
	let cli = Cli::new();
	let golden = cli.golden();
	let before = std::fs::read_to_string(&golden).unwrap();

	cli.run(&[
		"--space",
		golden.to_str().unwrap(),
		"note",
		"add",
		"a captured note",
		"--section",
		LAST_SECTION,
	])
	.ok();

	assert_minimal_note_diff(&before, &std::fs::read_to_string(&golden).unwrap());
}

/// `format.rs`'s own assertion, applied to two versions of a file.
///
/// Reconstructing the expected result is stricter than comparing common prefixes
/// and suffixes, and does not depend on which of two identical `}` lines a diff
/// algorithm chooses to align.
fn assert_minimal_note_diff(before: &str, after: &str) {
	let before_lines: Vec<&str> = before.lines().collect();
	let after_lines: Vec<&str> = after.lines().collect();

	let closing = before_lines.len() - 3;
	assert_eq!(before_lines[closing].trim(), "}", "not the last note's closing brace");

	let added = &after_lines[closing + 1..after_lines.len() - 2];
	assert_eq!(added.len(), 9, "the added region is not one note object: {added:#?}");
	assert_eq!(added[0].trim(), "{");
	assert_eq!(added[8].trim(), "}");

	let mut expected: Vec<String> = before_lines.iter().map(|line| line.to_string()).collect();
	expected[closing] = format!("{},", expected[closing]);
	expected.splice(closing + 1..closing + 1, added.iter().map(|l| l.to_string()));

	let after_owned: Vec<String> = after_lines.iter().map(|line| line.to_string()).collect();
	assert_eq!(expected, after_owned, "something outside the new note changed");
}

#[test]
fn a_body_can_come_from_stdin_verbatim() {
	use std::io::Write;

	let cli = Cli::new();
	let space = cli.space("piped");

	let mut child = cli
		.command()
		.args(["--space", space.to_str().unwrap(), "note", "add", "--stdin"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.unwrap();
	child
		.stdin
		.take()
		.unwrap()
		.write_all(b"# Heading\n\n```rust\nlet x = 1;\n```\n")
		.unwrap();
	Run::of(child.wait_with_output().unwrap()).ok();

	let run = cli.run(&["--space", space.to_str().unwrap(), "--json", "note", "list"]);
	let body = run.json()["notes"][0]["body"].as_str().unwrap().to_string();
	assert_eq!(body, "# Heading\n\n```rust\nlet x = 1;\n```");
}

/// `# Name` is a composer-only directive. A CLI note whose first line is a
/// heading is a note with a heading in it, and must not become a section.
#[test]
fn note_add_never_creates_a_section_from_a_heading() {
	let cli = Cli::new();
	let space = cli.space("headings");

	cli.run(&["--space", space.to_str().unwrap(), "note", "add", "# Research"])
		.ok();

	let run = cli.run(&["--space", space.to_str().unwrap(), "--json", "section", "list"]);
	let sections = run.json();
	assert_eq!(sections["sections"].as_array().unwrap().len(), 1);
	assert_eq!(sections["sections"][0]["name"], "Notes");
}

#[test]
fn top_puts_a_note_above_the_others() {
	let cli = Cli::new();
	let space = cli.space("ordering");
	let at = cli.at(&space);

	at(&["note", "add", "first"]).ok();
	at(&["note", "add", "second", "--top"]).ok();

	let run = at(&["--json", "note", "list"]);
	let notes = run.json();
	assert_eq!(notes["notes"][0]["body"], "second");
	assert_eq!(notes["notes"][1]["body"], "first");
}

// --- references -------------------------------------------------------------------

#[test]
fn a_note_id_prefix_resolves_and_an_ambiguous_one_is_refused() {
	let cli = Cli::new();
	let space = cli.space("prefixes");
	let at = cli.at(&space);

	let id = at(&["note", "add", "findable"]).ok().out().to_string();
	let hex = id.strip_prefix("nte_").unwrap();

	// The full id, the hex part, and a prefix of the hex part all name it.
	at(&["note", "done", &id]).ok();
	at(&["note", "undone", hex]).ok();
	at(&["note", "done", &hex[..3]]).ok();

	// A prefix matching nothing is not-found; an empty one is invalid.
	at(&["note", "done", "zzzzzzzz"]).failed(3);
	at(&["note", "done", "nte_"]).failed(2);
}

#[test]
fn an_ambiguous_section_name_is_refused_with_the_matches_listed() {
	let cli = Cli::new();
	let space = cli.space("ambiguous");
	let at = cli.at(&space);

	at(&["section", "add", "Later"]).ok();
	// A second section of the same name cannot be made through `section add`
	// alone, so the ambiguity is built by renaming one onto the other's name.
	let third = at(&["section", "add", "Temp"]).ok().out().to_string();
	at(&["section", "rename", &third, "later"]).ok();

	let run = at(&["note", "add", "somewhere", "--section", "LATER"]);
	run.failed(2);
	assert!(run.stderr.contains("matches 2 sections"), "{}", run.stderr);

	// The exact id is never ambiguous.
	at(&["note", "add", "somewhere", "--section", &third]).ok();
}

/// A name that happens to look like an id falls through to the name path rather
/// than failing on the prefix.
#[test]
fn a_section_named_like_an_id_is_still_reachable_by_name() {
	let cli = Cli::new();
	let space = cli.space("lookalike");
	let at = cli.at(&space);

	at(&["section", "add", "sec_notreal"]).ok();
	at(&["note", "add", "in the lookalike", "--section", "sec_notreal"]).ok();
}

// --- exit codes ---------------------------------------------------------------------

#[test]
fn every_failure_maps_to_its_documented_exit_code() {
	let cli = Cli::new();
	let space = cli.space("codes");
	let at = cli.at(&space);

	// invalid — an empty body is refused by `ops::clean_body`.
	at(&["note", "add", "   "]).failed(2);
	// not-found — no note by that id.
	at(&["note", "done", "deadbeef"]).failed(3);
	// unavailable — a space that resolves to nothing.
	cli.run(&["--space", "no-such.copper", "note", "list"]).failed(3);
	// parse — a file that is not a document.
	let broken = cli.cwd.join("broken.copper");
	std::fs::write(&broken, "{ not a space").unwrap();
	cli.run(&["--space", broken.to_str().unwrap(), "note", "list"]).failed(6);
	// invalid — a folder is not a space.
	let folder = cli.cwd.join("folder.copper");
	std::fs::create_dir(&folder).unwrap();
	cli.run(&["--space", folder.to_str().unwrap(), "note", "list"]).failed(2);
	// invalid — a usage error, sharing clap's code.
	cli.run(&["note", "add"]).failed(2);
	cli.run(&["note", "add", "a body", "--stdin"]).failed(2);
	at(&["note", "list", "--done", "--open"]).failed(2);
	cli.run(&["copy"]).failed(2);
	cli.run(&["copy", "--all", "--section", "Notes"]).failed(2);
}

#[test]
fn an_error_under_json_is_the_documented_envelope() {
	let cli = Cli::new();

	let run = cli.run(&["--json", "note", "list"]);

	run.failed(4);
	let envelope: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
	assert_eq!(envelope["kind"], "unavailable");
	assert!(envelope["message"].as_str().unwrap().contains("--space"));
	assert_eq!(envelope.as_object().unwrap().len(), 2);
}

// --- JSON shapes ---------------------------------------------------------------------

#[test]
fn every_json_output_parses_and_carries_its_documented_keys() {
	let cli = Cli::new();
	let space = cli.space("shapes");
	let at = cli.at(&space);

	let id = at(&["note", "add", "Send HTTP requests to the API"]).ok().out().to_string();

	let notes = at(&["--json", "note", "list"]).ok().json();
	let note = &notes["notes"][0];
	for key in [
		"id",
		"section",
		"sectionName",
		"order",
		"done",
		"body",
		"attachments",
		"created",
		"updated",
	] {
		assert!(note.get(key).is_some(), "{key} missing from {note}");
	}
	assert!(note["attachments"].is_array(), "attachments must be present even when empty");

	let sections = at(&["--json", "section", "list"]).ok().json();
	assert!(sections["sections"].is_array());
	assert!(sections["sections"][0]["id"].is_string());

	let found = at(&["--json", "search", "http req"]).ok().json();
	assert_eq!(found["query"], "http req");
	assert_eq!(found["exact"], false);
	assert_eq!(found["results"][0]["id"], id.as_str());
	assert!(found["results"][0]["body"].is_string());

	let added = at(&["--json", "note", "add", "another"]).ok().json();
	assert!(added["id"].as_str().unwrap().starts_with("nte_"));

	let done = at(&["--json", "note", "done", &id]).ok().json();
	assert_eq!(done["ids"][0], id.as_str());

	let current = cli.run(&["--json", "space", "current"]).ok().json();
	assert!(current.get("space").is_some());
}

#[test]
fn space_create_reports_the_path_id_and_name_under_json() {
	let cli = Cli::new();

	let run = cli.run(&["--json", "space", "create", "made.copper", "--name", "Made"]);

	run.ok();
	let created = run.json();
	assert!(created["path"].as_str().unwrap().ends_with("made.copper"));
	assert!(created["id"].as_str().unwrap().starts_with("spc_"));
	assert_eq!(created["name"], "Made");
}

#[test]
fn space_create_refuses_to_overwrite() {
	let cli = Cli::new();
	let path = cli.cwd.join("taken.copper");
	std::fs::write(&path, "precious").unwrap();

	cli.run(&["space", "create", "taken.copper"]).failed(2);

	assert_eq!(std::fs::read_to_string(&path).unwrap(), "precious");
}

// --- copy -----------------------------------------------------------------------------

#[test]
fn each_copy_format_renders_what_its_name_says() {
	let cli = Cli::new();
	let space = cli.space("copying");
	let at = cli.at(&space);

	at(&["note", "add", "alpha"]).ok();
	let beta = at(&["note", "add", "beta"]).ok().out().to_string();
	at(&["note", "done", &beta]).ok();

	assert_eq!(at(&["copy", "--all", "--format", "bodies"]).ok().out(), "alpha\n\nbeta");
	assert_eq!(at(&["copy", "--all", "--format", "list"]).ok().out(), "- alpha\n- beta");
	assert_eq!(
		at(&["copy", "--all"]).ok().out(),
		"# Notes\n- [ ] alpha\n- [x] beta",
		"markdown is the default"
	);

	let bare = at(&["copy", "--all", "--format", "json"]);
	let array: serde_json::Value = serde_json::from_str(bare.ok().out()).unwrap();
	assert_eq!(array[0]["body"], "alpha");
	assert_eq!(array[1]["done"], true);
}

/// The two flags that share a word. `--format` chooses the content, `--json`
/// chooses the envelope, and `--format json --json` nests rather than
/// double-encoding.
#[test]
fn the_content_format_and_the_output_envelope_compose() {
	let cli = Cli::new();
	let space = cli.space("envelopes");
	let at = cli.at(&space);
	at(&["note", "add", "alpha"]).ok();

	let wrapped = at(&["--json", "copy", "--all"]).ok().json();
	assert_eq!(wrapped["format"], "markdown");
	assert_eq!(wrapped["clipboard"], false);
	assert_eq!(wrapped["text"], "# Notes\n- [ ] alpha");

	let nested = at(&["--json", "copy", "--all", "--format", "json"]).ok().json();
	assert!(nested["text"].is_array(), "the array was double-encoded: {nested}");
	assert_eq!(nested["text"][0]["body"], "alpha");
}

/// `copper copy --format bodies > notes.md` has to write the notes and not one
/// byte more.
///
/// Every other command's stdout is a listing, where a trailing newline is right;
/// `copy`'s is a payload, and the same string goes to the clipboard. A newline is
/// added only when stdout is a terminal, which it is not here — the test captures
/// it through a pipe, which is exactly the case that must stay exact.
#[test]
fn copy_writes_exactly_the_rendering_with_no_trailing_newline() {
	let cli = Cli::new();
	let space = cli.space("exact");
	let at = cli.at(&space);
	at(&["note", "add", "alpha"]).ok();

	let run = at(&["copy", "--all", "--format", "bodies"]);
	run.ok();
	assert_eq!(run.stdout, "alpha", "a byte was added to the payload");

	// An empty selection writes nothing at all, rather than a bare newline the
	// selection did not contain.
	let empty = at(&["copy", "--query", "zzzznothing", "--format", "bodies"]);
	empty.ok();
	assert_eq!(empty.stdout, "");
}

/// The rendering is a function of *which* notes were selected, not of the order
/// they were named — the property the app's copy scopes have.
#[test]
fn copying_by_id_uses_document_order_whatever_order_the_ids_were_given() {
	let cli = Cli::new();
	let space = cli.space("stable");
	let at = cli.at(&space);

	let first = at(&["note", "add", "alpha"]).ok().out().to_string();
	let second = at(&["note", "add", "beta"]).ok().out().to_string();

	let forwards = at(&["copy", &first, &second, "--format", "bodies"]).ok().out().to_string();
	let backwards = at(&["copy", &second, &first, "--format", "bodies"]).ok().out().to_string();

	assert_eq!(forwards, "alpha\n\nbeta");
	assert_eq!(backwards, forwards);
}

// --- attachment export -------------------------------------------------------------------

#[test]
fn exporting_attachments_writes_them_under_their_original_names() {
	let cli = Cli::new();
	let space = cli.space("attached");
	let at = cli.at(&space);
	let note = at(&["note", "add", "has files"]).ok().out().to_string();

	// The CLI cannot ingest, so the sidecar and the document's attachment entries
	// are written directly — which is also a fair test of what the command reads.
	let assets = cli.cwd.join("attached.copper.assets");
	std::fs::create_dir_all(&assets).unwrap();
	std::fs::write(assets.join("00000000deadbeef.png"), b"first bytes").unwrap();
	std::fs::write(assets.join("11111111deadbeef.png"), b"second bytes").unwrap();
	attach(
		&space,
		&[
			("att_00000001", "00000000deadbeef.png", "shot.png"),
			("att_00000002", "11111111deadbeef.png", "shot.png"),
		],
	);

	let out = cli.cwd.join("exported");
	let run = at(&[
		"--json",
		"attachment",
		"export",
		&note,
		"--out",
		out.to_str().unwrap(),
	]);
	run.ok();

	let report = run.json();
	assert_eq!(report["exported"].as_array().unwrap().len(), 2);
	assert!(report["failed"].as_array().unwrap().is_empty());

	// The second one collided, so it took Explorer's ` (2)` convention.
	assert_eq!(std::fs::read_to_string(out.join("shot.png")).unwrap(), "first bytes");
	assert_eq!(
		std::fs::read_to_string(out.join("shot (2).png")).unwrap(),
		"second bytes",
		"a collision overwrote the first file"
	);
}

/// One missing blob must not sink the others, and the command still reports
/// failure through its exit code.
#[test]
fn a_failed_attachment_does_not_stop_the_rest_but_does_change_the_exit_code() {
	let cli = Cli::new();
	let space = cli.space("partial");
	let at = cli.at(&space);
	let note = at(&["note", "add", "one good one bad"]).ok().out().to_string();

	let assets = cli.cwd.join("partial.copper.assets");
	std::fs::create_dir_all(&assets).unwrap();
	std::fs::write(assets.join("00000000deadbeef.png"), b"good").unwrap();
	attach(
		&space,
		&[
			("att_00000001", "00000000deadbeef.png", "good.png"),
			("att_00000002", "ffffffffdeadbeef.png", "gone.png"),
		],
	);

	let out = cli.cwd.join("partial-out");
	let run = at(&[
		"--json",
		"attachment",
		"export",
		&note,
		"--out",
		out.to_str().unwrap(),
	]);

	run.failed(7);
	assert_eq!(std::fs::read_to_string(out.join("good.png")).unwrap(), "good");
	let report: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
	assert_eq!(report["exported"].as_array().unwrap().len(), 1);
	assert_eq!(report["failed"][0]["name"], "gone.png");

	// The payload says what happened, and stderr still carries the ordinary error
	// envelope — as JSON here, because JSON is what was asked for. A caller
	// watching stderr must not have to parse prose under one flag and JSON under
	// the other.
	let envelope: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
	assert_eq!(envelope["kind"], "io");
	assert!(
		envelope["message"].as_str().unwrap().contains("gone.png"),
		"{}",
		run.stderr
	);
}

/// Adds attachment entries to the document's single note, by rewriting the file.
///
/// The CLI has no ingest command by design, so a test that needs attachments has
/// to write them the way any other external editor would.
fn attach(space: &Path, entries: &[(&str, &str, &str)]) {
	let text = std::fs::read_to_string(space).unwrap();
	let mut doc: serde_json::Value = serde_json::from_str(&text).unwrap();
	let attachments: Vec<serde_json::Value> = entries
		.iter()
		.map(|(id, file, name)| {
			serde_json::json!({
				"id": id, "file": file, "name": name,
				"mime": "image/png", "bytes": 11
			})
		})
		.collect();
	doc["notes"][0]["attachments"] = serde_json::Value::Array(attachments);
	std::fs::write(space, format!("{}\n", serde_json::to_string_pretty(&doc).unwrap())).unwrap();
}

// --- concurrency ---------------------------------------------------------------------

/// A conflict that resolves, driven end to end with a deterministic sync point.
///
/// `note add --stdin` opens the space *before* it blocks reading stdin, so the
/// test knows exactly when the process is holding a stale read: overwrite the
/// file then, and the commit is guaranteed to find content it has not seen. The
/// re-apply lands our note on top of theirs.
#[test]
fn a_conflicting_write_re_applies_and_keeps_both_changes() {
	use std::io::Write;

	let cli = Cli::new();
	let space = cli.space("racing");
	cli.run(&["--space", space.to_str().unwrap(), "note", "add", "original"])
		.ok();

	let mut child = cli
		.command()
		.args(["--space", space.to_str().unwrap(), "note", "add", "--stdin"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.unwrap();

	// **A smoke test, not a proof, and the distinction is worth stating.** The
	// child opens the space and closes the handle before it blocks on stdin, so
	// there is no portable signal the parent can wait for — this sleep is a
	// heuristic. Were it ever too short, the child would simply read the external
	// content up front and these assertions would still pass, silently testing
	// nothing.
	//
	// The deterministic proof of the same behaviour is
	// `open_headless.rs::a_conflicting_headless_write_keeps_both_changes`, which
	// drives the interfering writer from inside the operation closure and so
	// controls the interleaving exactly. What this adds is that the *process* does
	// it too.
	std::thread::sleep(std::time::Duration::from_millis(400));
	let theirs = std::fs::read_to_string(&space)
		.unwrap()
		.replace("original", "theirs, written externally");
	std::fs::write(&space, &theirs).unwrap();

	child.stdin.take().unwrap().write_all(b"ours").unwrap();
	Run::of(child.wait_with_output().unwrap()).ok();

	let after = std::fs::read_to_string(&space).unwrap();
	assert!(after.contains("theirs, written externally"), "the external change was lost");
	assert!(after.contains("ours"), "our change was lost");
}

/// The acceptance criterion's other half: a file that keeps moving exhausts the
/// three attempts, exits 5, and leaves the external writer's content.
///
/// The writer runs for the whole of the child's life rather than once, because
/// the exhaustion path needs *three* different reads. Its interval is far shorter
/// than the write pipeline's backoff, so every attempt meets fresh content.
#[test]
fn a_file_that_keeps_moving_exits_five_and_keeps_the_external_content() {
	use std::io::Write;
	use std::sync::atomic::{AtomicBool, Ordering};
	use std::sync::Arc;

	let cli = Cli::new();
	let space = cli.space("hammered");
	cli.run(&["--space", space.to_str().unwrap(), "note", "add", "original"])
		.ok();
	let base = std::fs::read_to_string(&space).unwrap();

	let mut child = cli
		.command()
		.args(["--space", space.to_str().unwrap(), "note", "add", "--stdin"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.unwrap();
	std::thread::sleep(std::time::Duration::from_millis(400));

	let stop = Arc::new(AtomicBool::new(false));
	let writer = {
		let stop = Arc::clone(&stop);
		let path = space.clone();
		let scratch = cli.cwd.join("racer.tmp");
		std::thread::spawn(move || {
			let mut generation = 0u32;
			while !stop.load(Ordering::SeqCst) {
				generation += 1;
				let text = base.replace("original", &format!("generation {generation}"));
				// **Write-then-rename, not `fs::write`.** A plain write is not atomic,
				// so the child can read a half-written file — and it reports that as
				// `parse`, not `conflict`, which is a different (and correct) answer to
				// a different question. An external writer that behaves like git or an
				// editor is the one this test means to simulate.
				//
				// Both calls can lose a race against the child's own rename over the
				// same path. That is ordinary here, and the next iteration retries.
				if std::fs::write(&scratch, &text).is_ok() {
					let _ = std::fs::rename(&scratch, &path);
				}
				// No sleep. The child has to find different content on all three of
				// its attempts, and the gap between two of its reads spans a
				// serialise and an fsync — so the writer only has to be faster than
				// that, and being much faster is what keeps this from being a coin
				// flip.
				std::thread::yield_now();
			}
			generation
		})
	};

	child.stdin.take().unwrap().write_all(b"never lands").unwrap();
	let run = Run::of(child.wait_with_output().unwrap());
	stop.store(true, Ordering::SeqCst);
	writer.join().unwrap();

	run.failed(5);
	assert!(run.stderr.starts_with("conflict:"), "{}", run.stderr);
	let after = std::fs::read_to_string(&space).unwrap();
	assert!(after.contains("generation "), "the external content was replaced");
	assert!(!after.contains("never lands"), "an exhausted conflict still wrote");
}

// --- the help text ---------------------------------------------------------------------

/// The acceptance criterion says the JSON shapes are documented in `--help`, not
/// only in the project's own docs — a script author has the binary in front of
/// them and nothing else.
#[test]
fn every_json_shape_is_documented_in_help() {
	let cli = Cli::new();

	for (command, keys) in [
		(vec!["space", "list", "--help"], vec!["spaces", "availability"]),
		(vec!["space", "current", "--help"], vec!["space"]),
		(vec!["space", "create", "--help"], vec!["path", "id", "name"]),
		(vec!["section", "list", "--help"], vec!["sections", "active"]),
		(vec!["note", "list", "--help"], vec!["notes", "sectionName", "attachments"]),
		(vec!["note", "add", "--help"], vec!["id"]),
		(vec!["note", "delete", "--help"], vec!["ids"]),
		(vec!["search", "--help"], vec!["query", "exact", "results"]),
		(vec!["copy", "--help"], vec!["format", "text", "clipboard"]),
		(vec!["attachment", "export", "--help"], vec!["exported", "failed"]),
	] {
		let run = cli.run(&command);
		run.ok();
		assert!(
			run.stdout.contains("--json:"),
			"{command:?} does not document its JSON shape:\n{}",
			run.stdout
		);
		for key in keys {
			assert!(
				run.stdout.contains(key),
				"{command:?} does not name {key:?} in its JSON shape:\n{}",
				run.stdout
			);
		}
	}
}

/// The binary is `copper-cli.exe` in this repository and `copper.exe` once
/// installed. Neither name should ever reach the user: clap's `bin_name` is
/// pinned, so usage lines read `copper` in both.
#[test]
fn help_and_usage_always_name_the_command_copper() {
	let cli = Cli::new();

	let help = cli.run(&["--help"]);
	help.ok();
	assert!(help.stdout.contains("Usage: copper "), "{}", help.stdout);
	assert!(!help.stdout.contains("copper-cli"), "{}", help.stdout);

	let usage_error = cli.run(&["note", "add"]);
	usage_error.failed(2);
	assert!(usage_error.stderr.contains("Usage: copper note add"), "{}", usage_error.stderr);
}
