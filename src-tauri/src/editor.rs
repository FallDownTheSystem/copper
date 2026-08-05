//! The `$EDITOR` handoff: a note's body written to a temp `.md`, the user's own
//! editor spawned on it, and the file watched so a save comes back into the
//! store.
//!
//! Four things here are load-bearing, and each of them is a plausible
//! implementation away from losing work.
//!
//! 1. **The watch and the registry entry go in before the editor is spawned.**
//!    An already-running VS Code can open and save the file within milliseconds,
//!    and a watch registered after the spawn would miss that first save
//!    entirely. A spawn that then fails rolls all of it back.
//! 2. **A save is never applied blindly.** Two different questions are asked,
//!    which is why [`Handoff`] carries two baselines: `file_seen` answers "did we
//!    write these bytes ourselves?", and `body_baseline` answers "did the note
//!    move underneath the editor since it opened?". If the note moved — an undo,
//!    a merge, an inline edit, an external reload — the save is **refused**, the
//!    handoff is marked conflicted and the temp file is left in place, so neither
//!    version is lost. Applying it would be a lost update, and rewriting the temp
//!    file after the fact cannot help a save already in flight.
//! 3. **Return timing carries no information.** `notepad.exe` blocks until its
//!    window closes, `code.exe` returns immediately, `code --wait` returns on tab
//!    close. Nothing here awaits the child; the file watch is the only trigger.
//! 4. **The store mutex and the registry mutex are never held at the same time**,
//!    in either order. Every function below collects what it needs under one
//!    lock, releases it, and only then takes the other.
//!
//! `%EDITOR%` is user-controlled text, so it is parsed into an executable plus an
//! argument *vector* and handed to `std::process::Command`, which reaches
//! `CreateProcessW` with the arguments still separate. Copper never concatenates
//! a command line and **never constructs a `cmd` invocation of its own** — not
//! even for console editors, which get a window from `CREATE_NEW_CONSOLE`
//! instead. That is still not a claim that `cmd.exe` never appears in the process
//! tree: the common `EDITOR=code` resolves through `code.cmd`, and Rust's own
//! `Command` runs `.cmd` targets via the shell. The difference is who builds the
//! command line — std's escaping keeps the arguments separated there, which a
//! hand-built string would not, and which a `cmd /c start` wrapper of our own
//! would have put back in our hands.

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::diagnostics;
use crate::store::events::{AppSink, ChangeReason, EventSink, SpaceChanged, StoreEvent};
use crate::store::model::Space;
use crate::store::{self, ops, SharedStore};

/// Reaching it **refuses** the next open rather than ending the oldest handoff:
/// eviction would delete a temp file the user may have unsaved edits in, which
/// is silent data loss to save a few kilobytes.
const MAX_HANDOFFS: usize = 8;

/// The same value and the same reasoning as the space-file watcher: long enough
/// to absorb the several events one atomic replace produces.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// A read landing mid-write gets exactly one more chance, one debounce window
/// later.
const READ_RETRY: Duration = Duration::from_millis(300);

const TEMP_DIR_NAME: &str = "Copper";
const MAX_SLUG_CHARS: usize = 40;
const FALLBACK_SLUG: &str = "note";

const HANDOFF_CHANGED: &str = "editor-handoff-changed";

type FileWatcher = Debouncer<RecommendedWatcher, RecommendedCache>;

/// Console editors need a terminal to draw in; every other candidate is spawned
/// detached.
const CONSOLE_EDITORS: [&str; 8] = ["vi", "vim", "nvim", "nano", "micro", "helix", "hx", "emacs"];

/// `CREATE_NEW_CONSOLE`. Declared here rather than pulled from the `windows`
/// crate: it is one ABI-fixed integer from `winbase.h`, and the alternative is
/// widening the feature set of a crate this module otherwise does not touch.
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// A body larger than this is not read back. A temp file that has grown to tens
/// of megabytes is a runaway process or a mistaken paste, not a note, and the
/// store would hold the whole thing in memory for the rest of the session.
const MAX_READ_BACK_BYTES: u64 = 8 * 1024 * 1024;

// --- state -------------------------------------------------------------------

struct Handoff {
	/// Distinguishes *this* handoff from a later one on the same note. Stopping a
	/// handoff cannot cancel a debounce callback already in flight, so the guard
	/// has to be in the callback rather than in the teardown.
	handoff_id: String,
	dir: PathBuf,
	file: PathBuf,
	/// Held so it is not dropped early; dropping a debouncer stops the watch.
	_watcher: FileWatcher,
	/// The bytes Copper last wrote to the file — "was this event our own write?"
	file_seen: Vec<u8>,
	/// The store body this handoff's editor buffer started from — "did the note
	/// move underneath it?" A different question, and a different answer.
	body_baseline: String,
	conflicted: bool,
}

#[derive(Default)]
pub struct HandoffRegistry {
	/// Keyed by `note_id`, which is unique only *within* one space document —
	/// which is why switching space must call [`end_all`] rather than letting a
	/// handoff silently rebind to a same-id note in a different space.
	entries: Mutex<HashMap<String, Handoff>>,
}

/// What crosses to the webview. Temp paths and baselines deliberately do not.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffState {
	pub note_id: String,
	pub conflicted: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum OpenOutcome {
	Opened,
	/// Opened, but the handoff this replaced had a refused save whose temp file
	/// was kept. The path is carried so the panel can say where that text is —
	/// otherwise the only copy of it would be somewhere the user cannot find.
	OpenedWithRetainedFile {
		path: String,
	},
	NoEditor,
	AtCapacity {
		limit: usize,
	},
}

/// `%VISUAL%` and `%EDITOR%` and `notepad.exe` are an executable plus arguments;
/// the OS default `.md` handler is a shell association with no executable path
/// to parse. Modelling all four as one shape would force a fiction.
#[derive(Debug, PartialEq, Eq)]
pub enum EditorTarget {
	Executable { path: PathBuf, args: Vec<String> },
	OsAssociation,
}

fn entries(app: &AppHandle) -> MutexGuard<'_, HashMap<String, Handoff>> {
	// Tolerating poison for the same reason `store::lock` does: one panic must not
	// turn a recoverable failure into permanent unavailability.
	let state = app.state::<HandoffRegistry>();
	let guard = state.inner().entries.lock();
	// SAFETY of the unwrap-free form: `into_inner` yields the map either way.
	match guard {
		Ok(guard) => guard,
		Err(poisoned) => poisoned.into_inner(),
	}
}

// --- pure helpers ------------------------------------------------------------

/// Splits an `%EDITOR%` value into an executable and an argument vector,
/// honouring double-quoted segments so `"C:\Program Files\X\x.exe" --wait`
/// parses as one executable plus one argument.
pub fn parse_editor_command(raw: &str) -> (PathBuf, Vec<String>) {
	let mut tokens: Vec<String> = Vec::new();
	let mut current = String::new();
	let mut quoted = false;
	let mut started = false;

	for character in raw.chars() {
		match character {
			'"' => {
				quoted = !quoted;
				// A quoted segment that is genuinely empty is still a token: dropping
				// it would silently shift every argument after it.
				started = true;
			}
			c if c.is_whitespace() && !quoted => {
				if started {
					tokens.push(std::mem::take(&mut current));
					started = false;
				}
			}
			c => {
				current.push(c);
				started = true;
			}
		}
	}
	if started {
		tokens.push(current);
	}

	let mut tokens = tokens.into_iter();
	let program = tokens.next().unwrap_or_default();
	(PathBuf::from(program), tokens.collect())
}

/// A filesystem-safe name from the note's first line, so the editor's tab says
/// something about the note rather than a UUID.
pub fn slugify_first_line(body: &str) -> String {
	let first = body.lines().find(|line| !line.trim().is_empty()).unwrap_or("");

	let mut slug = String::new();
	let mut pending_dash = false;
	for character in first.chars() {
		if character.is_alphanumeric() {
			if pending_dash && !slug.is_empty() {
				slug.push('-');
			}
			pending_dash = false;
			// Lowercased so the name is stable across filesystems that differ on
			// case, and so two notes cannot collide only by capitalisation.
			let folded: String = character.to_lowercase().collect();
			// Checked *before* appending, not after: `İ` folds to two chars, so a
			// post-append check lets the slug overshoot the cap by one.
			if slug.chars().count() + folded.chars().count() > MAX_SLUG_CHARS {
				break;
			}
			slug.push_str(&folded);
		} else {
			pending_dash = true;
		}
		if slug.chars().count() >= MAX_SLUG_CHARS {
			break;
		}
	}

	if slug.is_empty() {
		return FALLBACK_SLUG.to_string();
	}
	// `CON.md`, `NUL.md` and friends are device names on Windows whatever the
	// extension and whatever directory they are in: creating one either fails or
	// opens the device. A note beginning with the word "con" is not exotic enough
	// to leave that to chance.
	if is_reserved_device_name(&slug) {
		return format!("{slug}-note");
	}
	slug
}

/// The DOS device names, which Win32 still resolves ahead of any real path.
fn is_reserved_device_name(slug: &str) -> bool {
	const RESERVED: [&str; 4] = ["con", "prn", "aux", "nul"];
	if RESERVED.contains(&slug) {
		return true;
	}
	// COM1–COM9 and LPT1–LPT9.
	let Some(digit) = slug.strip_prefix("com").or_else(|| slug.strip_prefix("lpt")) else {
		return false;
	};
	matches!(digit, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
}

/// The editors to try, in order. Each falls through to the next on a spawn
/// failure, ending at `notepad.exe`, which is always present.
pub fn resolve_editor() -> Vec<EditorTarget> {
	let mut targets = Vec::new();

	for name in ["VISUAL", "EDITOR"] {
		let Ok(raw) = std::env::var(name) else { continue };
		if raw.trim().is_empty() {
			continue;
		}
		let (path, args) = parse_editor_command(&raw);
		if !path.as_os_str().is_empty() {
			targets.push(EditorTarget::Executable { path, args });
		}
	}

	targets.push(EditorTarget::OsAssociation);
	targets.push(EditorTarget::Executable {
		path: PathBuf::from("notepad.exe"),
		args: Vec::new(),
	});
	targets
}

fn is_console_editor(path: &Path) -> bool {
	let stem = path
		.file_stem()
		.and_then(|stem| stem.to_str())
		.unwrap_or_default()
		.to_ascii_lowercase();
	CONSOLE_EDITORS.contains(&stem.as_str())
}

/// Resolves a bare program name against `PATH` × `PATHEXT`.
///
/// Rust's `Command` searches `PATH` but appends only `.exe`, so the single most
/// common value on Windows — `EDITOR=code`, which is really `code.cmd` — fails
/// with `NotFound` and the candidate list falls silently through to the OS
/// association. Resolving to a concrete path first is what makes `code` and
/// `code --wait` work, and it is also what lets std's own hardening apply: a
/// `.cmd` target goes through the shell with its arguments still escaped
/// separately, which is exactly the case AC36 anticipates.
///
/// A name that already carries an extension, or any path with a separator, is
/// returned untouched — `Command` handles those itself.
fn resolve_program(program: &Path, path_var: Option<&str>, pathext: Option<&str>) -> PathBuf {
	let has_separator = program
		.as_os_str()
		.to_string_lossy()
		.contains(['/', '\\']);
	if has_separator || program.extension().is_some() {
		return program.to_path_buf();
	}

	let extensions: Vec<String> = pathext
		.unwrap_or(".COM;.EXE;.BAT;.CMD")
		.split(';')
		.map(|ext| ext.trim().to_ascii_lowercase())
		.filter(|ext| !ext.is_empty())
		.collect();

	for directory in path_var.unwrap_or_default().split(';') {
		let directory = directory.trim();
		if directory.is_empty() {
			continue;
		}
		for extension in &extensions {
			let mut name = program.as_os_str().to_os_string();
			name.push(extension);
			let candidate = Path::new(directory).join(name);
			if candidate.is_file() {
				return candidate;
			}
		}
	}

	// Nothing matched. Handed back unchanged so `Command` reports the failure and
	// the caller falls through to the next candidate, rather than this inventing
	// an error of its own.
	program.to_path_buf()
}

fn resolve_from_environment(program: &Path) -> PathBuf {
	resolve_program(
		program,
		std::env::var("PATH").ok().as_deref(),
		std::env::var("PATHEXT").ok().as_deref(),
	)
}

fn temp_root() -> PathBuf {
	std::env::temp_dir().join(TEMP_DIR_NAME)
}

/// Deletes a temp tree, tolerating one that is already gone.
///
/// Cleanup is **best-effort** at every call site: a detached editor or an
/// antivirus scanner can hold a handle open, so a failure is logged and never
/// surfaced — [`scavenge`] completes the guarantee on the next startup. Factored
/// out of the four places that each spelled this out, two of them silently.
fn remove_tree(path: &Path) {
	if let Err(err) = std::fs::remove_dir_all(path) {
		if err.kind() != std::io::ErrorKind::NotFound {
			diagnostics::log_error(&format!("[copper] could not remove {}: {err}", path.display()));
		}
	}
}

/// Editors are happier with a file that ends in a newline, and the store trims
/// trailing whitespace on the way back in, so this round-trips cleanly.
fn file_contents(body: &str) -> String {
	format!("{body}\n")
}

// --- launching ---------------------------------------------------------------

fn launch(target: &EditorTarget, app: &AppHandle, file: &Path) -> Result<(), String> {
	match target {
		EditorTarget::OsAssociation => {
			use tauri_plugin_opener::OpenerExt;
			app.opener()
				.open_path(file.to_string_lossy().to_string(), None::<&str>)
				.map_err(|err| err.to_string())
		}
		EditorTarget::Executable { path, args } => {
			// The editor is spawned **directly**. An earlier form wrapped console
			// editors in `cmd /c start`, which was wrong twice over: it put a shell
			// between Copper and a program named by user-controlled text, and — worse
			// in practice — `cmd` itself spawns successfully whatever it is asked to
			// run, so a missing editor reported success and the fall-through to the
			// next candidate never happened.
			let resolved = resolve_from_environment(path);
			let mut command = Command::new(&resolved);
			command.args(args).arg(file);

			// A console editor needs a window to draw in, which it gets from the
			// creation flag rather than from a shell.
			if is_console_editor(&resolved) {
				command.creation_flags(CREATE_NEW_CONSOLE);
			}

			// Never awaited: `notepad.exe` blocks until its window closes and
			// `code.exe` returns at once, so an exit status would mean two different
			// things. The file watch is the trigger.
			command.spawn().map(|_| ()).map_err(|err| err.to_string())
		}
	}
}

// --- store access ------------------------------------------------------------
// Each of these takes the store lock and releases it before returning. Nothing
// here may be called while the registry lock is held.

fn note_body(app: &AppHandle, note_id: &str) -> Option<String> {
	let state = app.state::<SharedStore>();
	let guard = store::lock(&state);
	let space = guard.active_space().ok()?;
	space.note(note_id).map(|note| note.body.clone())
}

enum WriteOutcome {
	Written,
	/// The note already held exactly this text — nothing to write, and no reason
	/// to bump `updated` or churn the file.
	AlreadyEqual,
	/// The note no longer exists.
	Missing,
	/// The note moved underneath the editor: the save is refused, not applied.
	Moved,
	Refused(String),
}

/// Compares the note against the handoff's baseline and writes only if it still
/// matches — **both under one store lock**.
///
/// Splitting the compare from the write is the mistake this exists to prevent:
/// the check is the whole of AC34a's protection, and with the lock released in
/// between, an undo landing in that window would be silently overwritten by a
/// save the check had already blessed.
///
/// The announcement is not optional either. A mutation the *frontend* invokes
/// needs no event because its return value is the change (task-003 §8.4) — but
/// this one has no caller on that side, so without an emit the panel would keep
/// rendering the pre-save body. The watcher cannot rescue it: the write is
/// Copper's own, so it is correctly suppressed as a self-write.
fn write_if_unchanged(
	app: &AppHandle,
	note_id: &str,
	baseline: &str,
	body: &str,
) -> WriteOutcome {
	let state = app.state::<SharedStore>();
	let mut guard = store::lock(&state);

	let current = match guard.active_space() {
		Ok(space) => space.note(note_id).map(|note| note.body.clone()),
		Err(err) => return WriteOutcome::Refused(err.message()),
	};
	let Some(current) = current else {
		return WriteOutcome::Missing;
	};
	if current != baseline {
		return WriteOutcome::Moved;
	}
	if current == body {
		return WriteOutcome::AlreadyEqual;
	}

	// No snapshot, by design: task-003 §4.3 excludes `edit_note` from the undo
	// stack, and snapshot undo covers structural operations only.
	let written = guard
		.mutate_no_snapshot(|doc| ops::edit_note(doc, note_id, body))
		.map_err(|err| err.message());

	let announcement = written.as_ref().ok().map(|(_, space): &(_, Space)| {
		StoreEvent::SpaceChanged(SpaceChanged {
			id: space.id.clone(),
			path: guard.active_path().map(store::path_string).unwrap_or_default(),
			reason: ChangeReason::Editor,
		})
	});

	// Never emitted under the store guard: Tauri dispatches to Rust-side
	// listeners synchronously, and a listener touching store state would deadlock
	// against a non-reentrant mutex.
	drop(guard);
	if let Some(event) = announcement {
		// The store's own sink rather than a second copy of it: this is an ordinary
		// `StoreEvent`, and the emit-and-log-never-propagate policy — including its
		// message — belongs in one place.
		AppSink::new(app.clone()).emit(&event);
	}

	match written {
		Ok(_) => WriteOutcome::Written,
		Err(message) => WriteOutcome::Refused(message),
	}
}

// --- registry operations -----------------------------------------------------

fn states(app: &AppHandle) -> Vec<HandoffState> {
	let mut list: Vec<HandoffState> = entries(app)
		.iter()
		.map(|(note_id, handoff)| HandoffState {
			note_id: note_id.clone(),
			conflicted: handoff.conflicted,
		})
		.collect();
	// Stable order, so the frontend's list does not reshuffle on every emit.
	list.sort_by(|a, b| a.note_id.cmp(&b.note_id));
	list
}

fn emit_state(app: &AppHandle) {
	let payload = serde_json::json!({ "handoffs": states(app) });
	if let Err(err) = app.emit(HANDOFF_CHANGED, payload) {
		diagnostics::log_error(&format!("[copper] could not emit {HANDOFF_CHANGED}: {err}"));
	}
}

/// Drops the watcher and, unless the handoff is conflicted, deletes the temp
/// tree. Does not emit — callers decide when the batch is done.
///
/// **A conflicted handoff keeps its file.** Its contents are a save Copper
/// refused to apply, so the temp file is the only copy of that work; deleting it
/// on stop, on reopen or on exit would destroy exactly what AC34a set out to
/// preserve. The retained path is returned so the caller can say where it is.
///
/// Cleanup is otherwise **best-effort**: a detached editor or an antivirus
/// scanner can hold a handle open. The guarantee is completed on the other side,
/// by [`scavenge`] on startup — which is also why a retained file is not a leak
/// so much as a deferred one.
fn remove(app: &AppHandle, note_id: &str) -> Removed {
	let Some(handoff) = entries(app).remove(note_id) else {
		return Removed::Absent;
	};
	// The watcher is dropped with the entry, before the directory would go.
	drop(handoff._watcher);

	if keeps_its_file(handoff.conflicted) {
		return Removed::Retained(handoff.file);
	}

	remove_tree(&handoff.dir);
	Removed::Deleted
}

/// Whether ending a handoff keeps its temp file.
///
/// A conflicted handoff's file holds a save Copper refused to apply, and that
/// text exists nowhere else — so stopping, reopening or exiting must not take it
/// with them. Factored out from the four call sites that used to delete
/// unconditionally, and testable without a Tauri runtime.
fn keeps_its_file(conflicted: bool) -> bool {
	conflicted
}

/// Whether `reconcile_handoffs` may refresh a temp file from the note.
///
/// Never while conflicted, for the same reason: overwriting the refused text to
/// "catch the editor up" would destroy exactly what the refusal protected.
fn should_rewrite_temp_file(conflicted: bool, baseline: &str, body: &str) -> bool {
	!conflicted && baseline != body
}

enum Removed {
	Absent,
	Deleted,
	/// The handoff was conflicted, so its temp file was kept. Carries the path,
	/// which is the only way the user can reach the refused text.
	Retained(PathBuf),
}

impl Removed {
	fn existed(&self) -> bool {
		!matches!(self, Removed::Absent)
	}

	fn retained(&self) -> Option<&Path> {
		match self {
			Removed::Retained(path) => Some(path.as_path()),
			_ => None,
		}
	}
}

/// Everything a save handler needs, taken in one pass under the registry lock so
/// the store lock can be taken afterwards rather than alongside.
struct Pending {
	handoff_id: String,
	file: PathBuf,
	file_seen: Vec<u8>,
	body_baseline: String,
}

fn pending_for(app: &AppHandle, note_id: &str, handoff_id: Option<&str>) -> Option<Pending> {
	let guard = entries(app);
	let handoff = guard.get(note_id)?;
	// Step 7: a callback from a superseded handoff must not mutate the note or
	// corrupt the new handoff's baseline.
	if handoff_id.is_some_and(|id| id != handoff.handoff_id) {
		return None;
	}
	Some(Pending {
		handoff_id: handoff.handoff_id.clone(),
		file: handoff.file.clone(),
		file_seen: handoff.file_seen.clone(),
		body_baseline: handoff.body_baseline.clone(),
	})
}

/// Every mutation re-checks the `handoff_id` it was resolved under.
///
/// The read and the decision are not atomic — the store lock is taken between
/// them, and the registry lock is deliberately not held across it — so a stop or
/// a reopen can land in the gap. Without this the callback would write its
/// verdict onto a *different* handoff's entry.
fn with_handoff<T>(
	app: &AppHandle,
	note_id: &str,
	handoff_id: &str,
	edit: impl FnOnce(&mut Handoff) -> T,
) -> Option<T> {
	let mut guard = entries(app);
	let handoff = guard.get_mut(note_id)?;
	if handoff.handoff_id != handoff_id {
		return None;
	}
	Some(edit(handoff))
}

fn mark_conflicted(app: &AppHandle, note_id: &str, handoff_id: &str) -> bool {
	with_handoff(app, note_id, handoff_id, |handoff| {
		!std::mem::replace(&mut handoff.conflicted, true)
	})
	.unwrap_or(false)
}

/// Returns whether the card's state changed — that is, whether an earlier
/// refusal has just stopped being true.
fn accept_save(
	app: &AppHandle,
	note_id: &str,
	handoff_id: &str,
	bytes: Vec<u8>,
	body: String,
) -> bool {
	with_handoff(app, note_id, handoff_id, |handoff| {
		handoff.file_seen = bytes;
		handoff.body_baseline = body;
		std::mem::replace(&mut handoff.conflicted, false)
	})
	.unwrap_or(false)
}

// --- the save path -----------------------------------------------------------

/// Why a save did not reach the note. Each variant is surfaced rather than
/// logged: a handoff that silently stops applying saves while the card still
/// reads "Editing externally" is the worst of both states.
enum SaveProblem {
	Unreadable,
	NotText,
	TooLarge(u64),
	Empty,
	Rejected(String),
}

impl SaveProblem {
	fn describe(&self, note_id: &str) -> String {
		match self {
			SaveProblem::Unreadable => format!("could not read the temp file for {note_id}"),
			SaveProblem::NotText => format!("the temp file for {note_id} is not UTF-8 text"),
			SaveProblem::TooLarge(size) => {
				format!("the temp file for {note_id} is {size} bytes, past the read-back limit")
			}
			SaveProblem::Empty => format!("the temp file for {note_id} is empty"),
			SaveProblem::Rejected(message) => format!("the store refused the save for {note_id}: {message}"),
		}
	}
}

/// Reads the file, size-capped. The second attempt covers an editor caught
/// mid-write; it is skipped on the way out, where the sleep would only delay a
/// process the user has already closed.
fn read_body_file(file: &Path) -> Result<Vec<u8>, SaveProblem> {
	let attempts = if EXITING.load(std::sync::atomic::Ordering::Relaxed) {
		1
	} else {
		2
	};
	for attempt in 0..attempts {
		match std::fs::metadata(file) {
			Ok(metadata) if metadata.len() > MAX_READ_BACK_BYTES => {
				return Err(SaveProblem::TooLarge(metadata.len()))
			}
			Ok(_) => {}
			Err(_) if attempt + 1 < attempts => {
				std::thread::sleep(READ_RETRY);
				continue;
			}
			Err(_) => return Err(SaveProblem::Unreadable),
		}
		match std::fs::read(file) {
			Ok(bytes) => return Ok(bytes),
			Err(_) if attempt + 1 < attempts => std::thread::sleep(READ_RETRY),
			Err(_) => return Err(SaveProblem::Unreadable),
		}
	}
	Err(SaveProblem::Unreadable)
}

/// The debounced save handler, and [`end_all`]'s per-handoff step. Returns true
/// when the registry changed and the frontend needs telling.
fn apply_saved_file(app: &AppHandle, note_id: &str, handoff_id: Option<&str>) -> bool {
	let Some(pending) = pending_for(app, note_id, handoff_id) else {
		return false;
	};
	let id = pending.handoff_id.as_str();

	let bytes = match read_body_file(&pending.file) {
		Ok(bytes) => bytes,
		Err(problem) => return report_problem(app, note_id, id, &problem),
	};
	// Our own write coming back as an event.
	if bytes == pending.file_seen {
		return false;
	}
	let Ok(text) = std::str::from_utf8(&bytes) else {
		return report_problem(app, note_id, id, &SaveProblem::NotText);
	};

	// Matches what `edit_note` will actually store, so the baseline this sets is
	// the store's own text rather than a version of it that only agrees by luck.
	let body = text.trim_end().to_string();
	if body.is_empty() {
		// The store refuses an empty body rather than treating it as a delete, and a
		// cleared buffer is far more likely to be an accident than an intent.
		return report_problem(app, note_id, id, &SaveProblem::Empty);
	}

	// One store lock for the compare *and* the write. Reading the body, deciding,
	// and writing under three separate acquisitions left a window in which an undo
	// could land between the check and the write — and the check is the whole of
	// AC34a's protection.
	match write_if_unchanged(app, note_id, &pending.body_baseline, &body) {
		WriteOutcome::Written | WriteOutcome::AlreadyEqual => {
			accept_save(app, note_id, id, bytes, body)
		}
		// The note ceased to exist while the editor held it.
		WriteOutcome::Missing => remove(app, note_id).existed(),
		// An upstream change landed that the editor's buffer never started from.
		WriteOutcome::Moved => mark_conflicted(app, note_id, id),
		WriteOutcome::Refused(message) => {
			report_problem(app, note_id, id, &SaveProblem::Rejected(message))
		}
	}
}

/// Marks the handoff conflicted and says why. Returns whether the card changed.
fn report_problem(app: &AppHandle, note_id: &str, handoff_id: &str, problem: &SaveProblem) -> bool {
	let detail = problem.describe(note_id);
	diagnostics::log_error(&format!("[copper] editor read-back: {detail}"));
	// Conflicted rather than silent: the card keeps its temp file and says the
	// save did not land, instead of reading "Editing externally" forever while
	// every save is quietly dropped.
	mark_conflicted(app, note_id, handoff_id)
}

fn spawn_watch(
	app: &AppHandle,
	note_id: &str,
	handoff_id: &str,
	dir: &Path,
	file: &Path,
) -> Result<FileWatcher, String> {
	let file_name = file
		.file_name()
		.ok_or_else(|| format!("{} names no file", file.display()))?
		.to_os_string();
	let app = app.clone();
	let note_id = note_id.to_string();
	let handoff_id = handoff_id.to_string();

	let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
		let Ok(events) = result else { return };
		// Event kind is not consulted: an editor's atomic save is a create plus a
		// rename, and `need_rescan` means the backend lost track entirely.
		let touched = events.iter().any(|event| {
			event.need_rescan()
				|| event
					.paths
					.iter()
					.any(|path| path.file_name() == Some(file_name.as_os_str()))
		});
		if touched && apply_saved_file(&app, &note_id, Some(&handoff_id)) {
			emit_state(&app);
		}
	})
	.map_err(|err| format!("could not watch {}: {err}", dir.display()))?;

	debouncer
		.watch(dir, RecursiveMode::NonRecursive)
		.map_err(|err| format!("could not watch {}: {err}", dir.display()))?;

	Ok(debouncer)
}

// --- commands ----------------------------------------------------------------

#[tauri::command]
pub async fn editor_handoffs(app: AppHandle) -> Result<Vec<HandoffState>, String> {
	Ok(states(&app))
}

#[tauri::command]
pub async fn editor_open_note(id: String, app: AppHandle) -> Result<OpenOutcome, String> {
	let Some(body) = note_body(&app, &id) else {
		return Err(format!("note {id} no longer exists"));
	};

	// Reopening the same note replaces its handoff rather than stacking a second
	// temp file on one note. Its pending save is applied first, for the same
	// reason the cap refuses rather than evicts — and if that save was refused,
	// `remove` keeps the file rather than taking the refused text with it.
	let mut retained: Option<PathBuf> = None;
	if entries(&app).contains_key(&id) {
		apply_saved_file(&app, &id, None);
		retained = remove(&app, &id).retained().map(Path::to_path_buf);
	} else if entries(&app).len() >= MAX_HANDOFFS {
		return Ok(OpenOutcome::AtCapacity {
			limit: MAX_HANDOFFS,
		});
	}

	let handoff_id = uuid::Uuid::new_v4().simple().to_string();
	let dir = temp_root().join(&handoff_id);
	std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;

	let file = dir.join(format!("{}.md", slugify_first_line(&body)));
	let contents = file_contents(&body);
	if let Err(err) = std::fs::write(&file, contents.as_bytes()) {
		remove_tree(&dir);
		return Err(err.to_string());
	}

	// The watch and the registry entry go in *before* the spawn: an already-open
	// editor can save within milliseconds of the file appearing.
	let watcher = match spawn_watch(&app, &id, &handoff_id, &dir, &file) {
		Ok(watcher) => watcher,
		Err(message) => {
			remove_tree(&dir);
			return Err(message);
		}
	};

	entries(&app).insert(
		id.clone(),
		Handoff {
			handoff_id,
			dir,
			file: file.clone(),
			_watcher: watcher,
			file_seen: contents.into_bytes(),
			body_baseline: body,
			conflicted: false,
		},
	);

	let mut failures: Vec<String> = Vec::new();
	for target in resolve_editor() {
		match launch(&target, &app, &file) {
			Ok(()) => {
				emit_state(&app);
				return Ok(match retained {
					Some(path) => OpenOutcome::OpenedWithRetainedFile {
						path: path.to_string_lossy().into_owned(),
					},
					None => OpenOutcome::Opened,
				});
			}
			Err(message) => failures.push(message),
		}
	}

	// Every candidate failed: roll the whole thing back rather than leaving a
	// watched temp file nothing will ever open. This handoff has written nothing
	// and cannot be conflicted, so nothing is retained.
	remove(&app, &id);
	diagnostics::log_error(&format!(
		"[copper] no editor could be launched: {}",
		failures.join("; ")
	));
	Ok(OpenOutcome::NoEditor)
}

/// Ends a handoff, reporting the temp file's path when a refused save meant it
/// was kept.
#[tauri::command]
pub async fn editor_stop_handoff(id: String, app: AppHandle) -> Result<Option<String>, String> {
	// A pending save is applied — or refused and reported — before the file goes.
	// Ending a handoff is not consent to discard unsaved work, and `remove` keeps
	// a conflicted handoff's file for the same reason.
	apply_saved_file(&app, &id, None);
	let outcome = remove(&app, &id);
	let retained = outcome.retained().map(|path| path.to_string_lossy().into_owned());
	emit_state(&app);
	Ok(retained)
}

/// Called by the frontend after every **applied document**, which is the only
/// signal that covers every writer.
///
/// Task-003 §8.4 emits nothing for a command the frontend invoked, so a
/// Rust-side hook on the change event would miss exactly the undo, merge and
/// mark-done cases this exists for. The frontend's own document-applied path
/// sees all of them, and the work itself still happens here, where the temp
/// paths and the baselines are.
#[tauri::command]
pub async fn editor_reconcile(app: AppHandle) -> Result<(), String> {
	reconcile_handoffs(&app);
	Ok(())
}

// --- reconciliation ----------------------------------------------------------

/// For each live handoff: end it if the note is gone (AC48), and rewrite the
/// temp file if the note's body moved underneath it (AC47).
///
/// What a third-party editor does with a file that changed on disk is not
/// Copper's to guarantee — most prompt, some do not. The protection against a
/// stale save is the baseline check in [`apply_saved_file`], not the editor's
/// cooperation.
pub fn reconcile_handoffs(app: &AppHandle) {
	let ids: Vec<String> = entries(app).keys().cloned().collect();
	if ids.is_empty() {
		return;
	}

	let mut changed = false;
	for id in ids {
		// The store lock is taken here, with the registry lock released.
		match note_body(app, &id) {
			None => changed |= remove(app, &id).existed(),
			// A refreshed temp file changes no field of `HandoffState`, so there is
			// nothing here for the frontend to re-render.
			Some(body) => rewrite_temp_file(app, &id, &body),
		}
	}

	if changed {
		emit_state(app);
	}
}

/// Refreshes one handoff's temp file from the note (AC47). Reports nothing: no
/// field of `HandoffState` depends on the file's contents, so there is never an
/// emit owed for this.
fn rewrite_temp_file(app: &AppHandle, note_id: &str, body: &str) {
	let mut guard = entries(app);
	let Some(handoff) = guard.get_mut(note_id) else {
		return;
	};

	// The card stays conflicted until the user resolves it by ending the handoff
	// or reopening the note.
	if !should_rewrite_temp_file(handoff.conflicted, &handoff.body_baseline, body) {
		return;
	}

	let contents = file_contents(body);
	if let Err(err) = std::fs::write(&handoff.file, contents.as_bytes()) {
		diagnostics::log_error(&format!(
			"[copper] could not refresh {}: {err}",
			handoff.file.display()
		));
		return;
	}

	handoff.file_seen = contents.into_bytes();
	handoff.body_baseline = body.to_string();
}

/// The one way to end every live handoff at once — Phase 6 calls it on a space
/// switch, and the exit hook calls it too.
///
/// Handoffs are keyed by `note_id`, which is unique only within one document, so
/// a handoff surviving a space switch could silently rebind to a same-id note in
/// a different space. Each one is **reconciled before it is stopped** rather than
/// blindly deleted, for the same reason the concurrency cap refuses rather than
/// evicts: the temp file may hold work the user has not saved, and switching
/// space is not consent to discard it.
///
/// It must complete **before** the store swaps documents: reconciliation writes
/// through `edit_note` against the space that is still active. Idempotent and
/// safe on an empty registry.
///
/// A handoff whose save is refused keeps its temp file, and the retained paths
/// come back so the caller can report them. At exit nobody is listening, which is
/// why `scavenge` runs afterwards and the startup sweep is the real guarantee.
pub fn end_all(app: &AppHandle) -> Vec<PathBuf> {
	let ids: Vec<String> = entries(app).keys().cloned().collect();
	if ids.is_empty() {
		return Vec::new();
	}

	let mut retained = Vec::new();
	for id in ids {
		apply_saved_file(app, &id, None);
		if let Some(path) = remove(app, &id).retained() {
			retained.push(path.to_path_buf());
		}
	}
	emit_state(app);
	retained
}

/// [`end_all`] with the read retries turned off.
///
/// The retry in [`read_body_file`] exists for an editor caught mid-write, and it
/// sleeps a debounce window per attempt. At the cap of eight handoffs that is
/// several seconds of a process the user has already asked to close, for a case
/// that cannot arise at exit anyway — nothing is going to save into those files
/// while the app is going down.
pub fn end_all_at_exit(app: &AppHandle) {
	EXITING.store(true, std::sync::atomic::Ordering::Relaxed);
	let retained = end_all(app);
	for path in retained {
		diagnostics::log_error(&format!(
			"[copper] kept {} — it holds an editor save Copper could not apply",
			path.display()
		));
	}
}

/// Set once, on the way out. Read by [`read_body_file`] only.
static EXITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Removes the whole `%TEMP%\Copper` tree.
///
/// Run on startup **before any handoff can be registered**, which is what makes
/// the cleanup guarantee true after a crash or a file an editor held open — an
/// exit hook alone cannot, because a crash runs no exit hook at all. A failed
/// delete is logged, never surfaced.
pub fn scavenge() {
	remove_tree(&temp_root());
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_bare_executable_parses_with_no_arguments() {
		let (path, args) = parse_editor_command("notepad.exe");
		assert_eq!(path, PathBuf::from("notepad.exe"));
		assert!(args.is_empty());
	}

	#[test]
	fn arguments_stay_separate() {
		let (path, args) = parse_editor_command("code --wait --new-window");
		assert_eq!(path, PathBuf::from("code"));
		assert_eq!(args, ["--wait", "--new-window"]);
	}

	#[test]
	fn a_quoted_path_with_spaces_is_one_token() {
		// The case that decides whether an environment variable can become a
		// shell-injection vector: it must parse to one executable plus one
		// argument, never to a string somebody later concatenates.
		let (path, args) = parse_editor_command(r#""C:\Program Files\X\x.exe" --wait"#);
		assert_eq!(path, PathBuf::from(r"C:\Program Files\X\x.exe"));
		assert_eq!(args, ["--wait"]);
	}

	#[test]
	fn quotes_around_an_argument_are_stripped() {
		let (path, args) = parse_editor_command(r#"editor "--file name" plain"#);
		assert_eq!(path, PathBuf::from("editor"));
		assert_eq!(args, ["--file name", "plain"]);
	}

	#[test]
	fn runs_of_whitespace_do_not_produce_empty_arguments() {
		let (_, args) = parse_editor_command("editor   --a\t--b  ");
		assert_eq!(args, ["--a", "--b"]);
	}

	#[test]
	fn an_empty_value_parses_to_an_empty_program() {
		let (path, args) = parse_editor_command("   ");
		assert_eq!(path, PathBuf::new());
		assert!(args.is_empty());
	}

	#[test]
	fn a_slug_comes_from_the_first_non_empty_line() {
		assert_eq!(slugify_first_line("\n\nHello World\nsecond"), "hello-world");
	}

	#[test]
	fn punctuation_collapses_to_single_dashes_without_leading_or_trailing_ones() {
		assert_eq!(slugify_first_line("### Fix: the *thing*!"), "fix-the-thing");
	}

	#[test]
	fn a_slug_is_capped_and_never_empty() {
		let long = "a".repeat(200);
		assert_eq!(slugify_first_line(&long).chars().count(), MAX_SLUG_CHARS);
		assert_eq!(slugify_first_line("###   ***"), FALLBACK_SLUG);
		assert_eq!(slugify_first_line(""), FALLBACK_SLUG);
	}

	#[test]
	fn a_multi_scalar_fold_cannot_overshoot_the_cap() {
		// `İ` lowercases to two chars, so a cap checked after appending lets the
		// slug run one past it.
		let body = "İ".repeat(60);
		assert!(slugify_first_line(&body).chars().count() <= MAX_SLUG_CHARS);
	}

	#[test]
	fn reserved_device_names_are_never_produced() {
		// `CON.md` is a device on Windows whatever the directory and whatever the
		// extension, so creating the file either fails or opens the device.
		for name in ["con", "PRN", "Aux", "nul", "com1", "LPT9"] {
			let slug = slugify_first_line(name);
			assert!(
				!is_reserved_device_name(&slug),
				"{name} slugged to the reserved name {slug}"
			);
		}
		// A name that merely starts the same way is left alone.
		assert_eq!(slugify_first_line("console output"), "console-output");
		assert_eq!(slugify_first_line("com10 notes"), "com10-notes");
	}

	#[test]
	fn a_program_with_an_extension_or_a_path_is_left_alone() {
		let untouched = Path::new(r"C:\tools\vim.exe");
		assert_eq!(
			resolve_program(untouched, Some(r"C:\nowhere"), Some(".EXE")),
			untouched
		);
		assert_eq!(
			resolve_program(Path::new("notepad.exe"), Some(r"C:\nowhere"), Some(".EXE")),
			Path::new("notepad.exe")
		);
	}

	#[test]
	fn an_extensionless_program_resolves_through_path_and_pathext() {
		// The case that decides whether `EDITOR=code` works at all: `code` on PATH
		// is `code.cmd`, and Rust's own Command appends only `.exe`.
		let dir = std::env::temp_dir().join(format!("copper-resolve-{}", uuid::Uuid::new_v4()));
		std::fs::create_dir_all(&dir).expect("temp dir");
		let target = dir.join("code.cmd");
		std::fs::write(&target, b"@echo off").expect("write");

		let resolved = resolve_program(
			Path::new("code"),
			Some(&dir.to_string_lossy()),
			Some(".EXE;.CMD"),
		);
		assert_eq!(resolved, target);

		// Unresolvable names come back unchanged, so `Command` reports the failure
		// and the caller falls through to the next candidate.
		assert_eq!(
			resolve_program(Path::new("nosucheditor"), Some(&dir.to_string_lossy()), Some(".EXE")),
			Path::new("nosucheditor")
		);

		std::fs::remove_dir_all(&dir).ok();
	}

	#[test]
	fn pathext_order_decides_which_extension_wins() {
		let dir = std::env::temp_dir().join(format!("copper-pathext-{}", uuid::Uuid::new_v4()));
		std::fs::create_dir_all(&dir).expect("temp dir");
		std::fs::write(dir.join("ed.cmd"), b"").expect("write");
		std::fs::write(dir.join("ed.exe"), b"").expect("write");

		assert_eq!(
			resolve_program(Path::new("ed"), Some(&dir.to_string_lossy()), Some(".EXE;.CMD")),
			dir.join("ed.exe")
		);
		assert_eq!(
			resolve_program(Path::new("ed"), Some(&dir.to_string_lossy()), Some(".CMD;.EXE")),
			dir.join("ed.cmd")
		);

		std::fs::remove_dir_all(&dir).ok();
	}

	#[test]
	fn a_slug_carries_no_path_separators_or_reserved_characters() {
		let slug = slugify_first_line(r#"..\..\Windows\System32 <bad> "x" |y| ?z*"#);
		assert!(slug
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || c == '-' || !c.is_ascii()));
	}

	#[test]
	fn console_editors_are_detected_by_stem_not_by_full_path() {
		assert!(is_console_editor(Path::new(r"C:\tools\vim.exe")));
		assert!(is_console_editor(Path::new("nano")));
		assert!(!is_console_editor(Path::new("code")));
		assert!(!is_console_editor(Path::new(r"C:\Windows\notepad.exe")));
	}

	#[test]
	fn resolution_always_ends_at_notepad_with_the_os_handler_before_it() {
		let targets = resolve_editor();
		let os = targets
			.iter()
			.position(|target| matches!(target, EditorTarget::OsAssociation))
			.expect("the OS handler is always a candidate");
		assert_eq!(targets.len() - 1, os + 1, "notepad.exe must be last");
		assert!(matches!(
			targets.last(),
			Some(EditorTarget::Executable { path, args })
				if path == Path::new("notepad.exe") && args.is_empty()
		));
	}

	#[test]
	fn a_body_reaches_the_file_with_exactly_one_trailing_newline() {
		assert_eq!(file_contents("one\ntwo"), "one\ntwo\n");
	}

	#[test]
	fn a_refused_save_keeps_its_temp_file_on_every_ending() {
		// Stopping, reopening the same note, and exiting all funnel through
		// `remove`, and all three used to delete unconditionally — taking with them
		// the only copy of a save Copper had refused to apply.
		assert!(keeps_its_file(true));
		assert!(!keeps_its_file(false));
	}

	#[test]
	fn a_conflicted_handoff_is_never_rewritten_from_the_note() {
		// The other half of the same hole: reconciliation refreshed the temp file
		// whenever the note moved, ignoring the flag — so the refused text was
		// overwritten by the very body the refusal was protecting it from, and the
		// flag was cleared on the way out.
		assert!(!should_rewrite_temp_file(true, "baseline", "moved"));
		// Unconflicted, and the note actually moved: refresh it (AC47).
		assert!(should_rewrite_temp_file(false, "baseline", "moved"));
		// Unconflicted and unchanged: nothing to do, and no needless write.
		assert!(!should_rewrite_temp_file(false, "same", "same"));
	}

	#[test]
	fn every_save_problem_says_which_note_it_is_about() {
		// Each of these used to be a silent drop that left the card reading
		// "Editing externally" while every save was discarded.
		let problems = [
			SaveProblem::Unreadable,
			SaveProblem::NotText,
			SaveProblem::TooLarge(MAX_READ_BACK_BYTES + 1),
			SaveProblem::Empty,
			SaveProblem::Rejected("a note cannot be empty".into()),
		];
		for problem in problems {
			let described = problem.describe("nte_01000001");
			assert!(described.contains("nte_01000001"), "{described}");
		}
	}
}
