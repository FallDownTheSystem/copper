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
//! a command line. That is not a claim that `cmd.exe` never appears in the
//! process tree — the common `EDITOR=code` resolves through `code.cmd`, and
//! Rust's own `Command` runs `.cmd` targets via the shell — but std's escaping
//! keeps the arguments separated there, which a hand-built string would not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::diagnostics;
use crate::store::events::{ChangeReason, SpaceChanged, StoreEvent};
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
	NoEditor,
	AtCapacity { limit: usize },
	Error { message: String },
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
			slug.extend(character.to_lowercase());
		} else {
			pending_dash = true;
		}
		if slug.chars().count() >= MAX_SLUG_CHARS {
			break;
		}
	}

	if slug.is_empty() {
		FALLBACK_SLUG.to_string()
	} else {
		slug
	}
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

fn temp_root() -> PathBuf {
	std::env::temp_dir().join(TEMP_DIR_NAME)
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
			// A terminal is created only for editors that need one to draw in.
			// `start`'s first quoted argument is the window title, hence the empty
			// one — and every part of this is a separate `Command` argument, never a
			// concatenated command line.
			let mut command = if is_console_editor(path) {
				let mut command = Command::new("cmd");
				command.args(["/c", "start", ""]);
				command.arg(path);
				command
			} else {
				Command::new(path)
			};

			command.args(args).arg(file);
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

/// Writes the note and announces it.
///
/// The announcement is not optional. A mutation the *frontend* invokes needs no
/// event because its return value is the change (task-003 §8.4) — but this one
/// has no caller on that side, so without an emit the panel would keep rendering
/// the pre-save body until something else happened to refresh it. The watcher
/// cannot rescue it either: the write is Copper's own, so it is correctly
/// suppressed as a self-write.
fn write_body(app: &AppHandle, note_id: &str, body: &str) -> Result<(), String> {
	let state = app.state::<SharedStore>();
	let mut guard = store::lock(&state);
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
		if let Err(err) = app.emit(event.name(), event.payload()) {
			diagnostics::log_error(&format!("[copper] could not emit {}: {err}", event.name()));
		}
	}

	written.map(|_| ())
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

/// Drops the watcher and deletes the temp tree. Does not emit — callers decide
/// when the batch is done.
///
/// Cleanup is **best-effort**: a detached editor or an antivirus scanner can
/// hold a handle open. The guarantee is completed on the other side, by
/// [`scavenge`] on startup.
fn remove(app: &AppHandle, note_id: &str) -> bool {
	let Some(handoff) = entries(app).remove(note_id) else {
		return false;
	};
	// The watcher is dropped with the entry, before the directory goes.
	drop(handoff._watcher);
	if let Err(err) = std::fs::remove_dir_all(&handoff.dir) {
		if err.kind() != std::io::ErrorKind::NotFound {
			diagnostics::log_error(&format!(
				"[copper] could not remove {}: {err}",
				handoff.dir.display()
			));
		}
	}
	true
}

/// Everything a save handler needs, taken in one pass under the registry lock so
/// the store lock can be taken afterwards rather than alongside.
struct Pending {
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
		file: handoff.file.clone(),
		file_seen: handoff.file_seen.clone(),
		body_baseline: handoff.body_baseline.clone(),
	})
}

fn mark_conflicted(app: &AppHandle, note_id: &str) {
	if let Some(handoff) = entries(app).get_mut(note_id) {
		handoff.conflicted = true;
	}
}

/// Returns whether the card's state changed — that is, whether an earlier
/// refusal has just stopped being true.
fn accept_save(app: &AppHandle, note_id: &str, bytes: Vec<u8>, body: String) -> bool {
	let mut guard = entries(app);
	let Some(handoff) = guard.get_mut(note_id) else {
		return false;
	};
	handoff.file_seen = bytes;
	handoff.body_baseline = body;
	std::mem::replace(&mut handoff.conflicted, false)
}

// --- the save path -----------------------------------------------------------

fn read_with_retry(file: &Path) -> Option<Vec<u8>> {
	match std::fs::read(file) {
		Ok(bytes) => Some(bytes),
		// The editor may be mid-write. One more chance, a debounce window later.
		Err(_) => {
			std::thread::sleep(READ_RETRY);
			std::fs::read(file).ok()
		}
	}
}

/// The debounced save handler, and [`end_all`]'s per-handoff step. Returns true
/// when the registry changed and the frontend needs telling.
fn apply_saved_file(app: &AppHandle, note_id: &str, handoff_id: Option<&str>) -> bool {
	let Some(pending) = pending_for(app, note_id, handoff_id) else {
		return false;
	};
	let Some(bytes) = read_with_retry(&pending.file) else {
		return false;
	};
	// Our own write coming back as an event.
	if bytes == pending.file_seen {
		return false;
	}
	let Ok(text) = String::from_utf8(bytes.clone()) else {
		diagnostics::log_error(&format!(
			"[copper] {} is not UTF-8; the save was not applied",
			pending.file.display()
		));
		return false;
	};

	let Some(current) = note_body(app, note_id) else {
		// The note ceased to exist while the editor held it.
		return remove(app, note_id);
	};

	// Step 9. An upstream change landed that the editor's buffer never started
	// from, so applying this save would silently destroy it.
	if current != pending.body_baseline {
		mark_conflicted(app, note_id);
		return true;
	}

	// Matches what `edit_note` will actually store, so the baseline this sets is
	// the store's own text rather than a version of it that only agrees by luck.
	let body = text.trim_end().to_string();
	if body.is_empty() {
		// The store refuses an empty body rather than treating it as a delete, and
		// a cleared buffer is far more likely to be an accident than an intent.
		return false;
	}
	if body == current {
		return accept_save(app, note_id, bytes, body);
	}

	match write_body(app, note_id, &body) {
		Ok(()) => accept_save(app, note_id, bytes, body),
		Err(message) => {
			diagnostics::log_error(&format!("[copper] editor save for {note_id} failed: {message}"));
			mark_conflicted(app, note_id);
			true
		}
	}
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
	// reason the cap refuses rather than evicts.
	if entries(&app).contains_key(&id) {
		apply_saved_file(&app, &id, None);
		remove(&app, &id);
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
		let _ = std::fs::remove_dir_all(&dir);
		return Err(err.to_string());
	}

	// The watch and the registry entry go in *before* the spawn: an already-open
	// editor can save within milliseconds of the file appearing.
	let watcher = match spawn_watch(&app, &id, &handoff_id, &dir, &file) {
		Ok(watcher) => watcher,
		Err(message) => {
			let _ = std::fs::remove_dir_all(&dir);
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
				return Ok(OpenOutcome::Opened);
			}
			Err(message) => failures.push(message),
		}
	}

	// Every candidate failed: roll the whole thing back rather than leaving a
	// watched temp file nothing will ever open.
	remove(&app, &id);
	diagnostics::log_error(&format!(
		"[copper] no editor could be launched: {}",
		failures.join("; ")
	));
	Ok(OpenOutcome::NoEditor)
}

#[tauri::command]
pub async fn editor_stop_handoff(id: String, app: AppHandle) -> Result<(), String> {
	// A pending save is applied — or refused and reported — before the temp file
	// goes. Ending a handoff is not consent to discard unsaved work.
	apply_saved_file(&app, &id, None);
	remove(&app, &id);
	emit_state(&app);
	Ok(())
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
		let body = note_body(app, &id);
		match body {
			None => changed |= remove(app, &id),
			Some(body) => changed |= rewrite_temp_file(app, &id, &body),
		}
	}

	if changed {
		emit_state(app);
	}
}

fn rewrite_temp_file(app: &AppHandle, note_id: &str, body: &str) -> bool {
	let mut guard = entries(app);
	let Some(handoff) = guard.get_mut(note_id) else {
		return false;
	};
	if handoff.body_baseline == body {
		return false;
	}

	let contents = file_contents(body);
	if let Err(err) = std::fs::write(&handoff.file, contents.as_bytes()) {
		diagnostics::log_error(&format!(
			"[copper] could not refresh {}: {err}",
			handoff.file.display()
		));
		return false;
	}

	handoff.file_seen = contents.into_bytes();
	handoff.body_baseline = body.to_string();
	// The note caught up with what the editor is looking at, so an earlier refusal
	// no longer describes anything. The old flag is the return value: it is what
	// decides whether the card's state actually changed.
	std::mem::replace(&mut handoff.conflicted, false)
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
pub fn end_all(app: &AppHandle) {
	let ids: Vec<String> = entries(app).keys().cloned().collect();
	if ids.is_empty() {
		return;
	}
	for id in ids {
		apply_saved_file(app, &id, None);
		remove(app, &id);
	}
	emit_state(app);
}

/// Removes the whole `%TEMP%\Copper` tree.
///
/// Run on startup **before any handoff can be registered**, which is what makes
/// the cleanup guarantee true after a crash or a file an editor held open — an
/// exit hook alone cannot, because a crash runs no exit hook at all. A failed
/// delete is logged, never surfaced.
pub fn scavenge() {
	let root = temp_root();
	if let Err(err) = std::fs::remove_dir_all(&root) {
		if err.kind() != std::io::ErrorKind::NotFound {
			diagnostics::log_error(&format!(
				"[copper] could not clear {}: {err}",
				root.display()
			));
		}
	}
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
}
