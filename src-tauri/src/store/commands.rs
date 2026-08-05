//! The twenty commands Phase 3 codes against (spec 8.1), and nothing else.
//!
//! Registration lives in the crate's own `commands.rs`: Tauri accepts exactly
//! one `invoke_handler`, and the closure `generate_handler!` builds consumes the
//! `Invoke` it is handed, so a per-module handler cannot be chained with
//! another. The wrappers stay here, next to the module they serve.
//!
//! Deliberately thin. Each one locks the store inside a synchronous block, calls
//! into the core, and lets the guard drop before returning — never held across
//! an `.await` (there are none) and never held while emitting.
//!
//! Two conventions are contracts rather than style:
//!
//! - **Every parameter name is a single word** — `patch`, `path`, `name`,
//!   `body`, `section`, `id`, `ids`, `done`, `index`. Tauri converts snake_case
//!   Rust argument names to camelCase on the JS side, so a multi-word parameter
//!   would have two spellings and the contract would depend on which one the
//!   caller guessed. If a wrapper ever needs one, its JS-side spelling must be
//!   documented in `doc-store-api.md` alongside the command (spec 8.1c).
//! - **Mutating commands emit nothing** (spec 8.4). Their return value already
//!   describes the change, and an event would only duplicate it. The exceptions
//!   are `open_space` and `create_space`, which each emit exactly one
//!   `settings-changed` because both mutate `recents`.
//!
//! What the return value does *not* describe is store status: `canUndo`,
//! `canRedo`, `errored` and `watching` are not part of a `Space`. The frontend
//! re-pulls `get_status` after `undo`/`redo` and on every `space-changed` or
//! `store-error` event; after an ordinary structural mutation it need not,
//! because the effect is deterministic — `canUndo: true`, `canRedo: false`.

use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use super::error::StoreError;
use super::model::Space;
use super::settings::{Settings, SettingsPatch};
use super::{lock, ops, SharedStore, StoreStatus};

type Reply<T> = std::result::Result<T, StoreError>;

/// `add_note` is the one mutation whose caller needs more than the document:
/// the new note's id, to focus it.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AddNoteResult {
	pub space: Space,
	pub note_id: String,
}

// --- settings and status -----------------------------------------------------

#[tauri::command]
pub async fn get_settings(state: State<'_, SharedStore>) -> Reply<Settings> {
	Ok(lock(&state).settings().clone())
}

#[tauri::command]
pub async fn update_settings(patch: SettingsPatch, state: State<'_, SharedStore>) -> Reply<Settings> {
	lock(&state).update_settings(patch)
}

#[tauri::command]
pub async fn get_status(state: State<'_, SharedStore>) -> Reply<StoreStatus> {
	Ok(lock(&state).status())
}

#[tauri::command]
pub async fn get_active_space(state: State<'_, SharedStore>) -> Reply<Space> {
	lock(&state).active_space()
}

// --- spaces ------------------------------------------------------------------

#[tauri::command]
pub async fn open_space(path: String, state: State<'_, SharedStore>) -> Reply<Space> {
	super::open_space(&state, Path::new(&path))
}

#[tauri::command]
pub async fn create_space(path: String, name: String, state: State<'_, SharedStore>) -> Reply<Space> {
	super::create_space(&state, Path::new(&path), &name)
}

// --- notes -------------------------------------------------------------------

#[tauri::command]
pub async fn add_note(
	body: String,
	section: Option<String>,
	state: State<'_, SharedStore>,
) -> Reply<AddNoteResult> {
	let (note_id, space) = lock(&state).mutate(|doc| ops::add_note(doc, &body, section.as_deref()))?;
	Ok(AddNoteResult { space, note_id })
}

#[tauri::command]
pub async fn edit_note(id: String, body: String, state: State<'_, SharedStore>) -> Reply<Space> {
	// No snapshot: text editing uses the browser's native undo (spec 4.3).
	let (_, space) = lock(&state).mutate_no_snapshot(|doc| ops::edit_note(doc, &id, &body))?;
	Ok(space)
}

#[tauri::command]
pub async fn set_notes_done(
	ids: Vec<String>,
	done: bool,
	state: State<'_, SharedStore>,
) -> Reply<Space> {
	let (_, space) = lock(&state).mutate(|doc| ops::set_notes_done(doc, &ids, done))?;
	Ok(space)
}

#[tauri::command]
pub async fn delete_notes(ids: Vec<String>, state: State<'_, SharedStore>) -> Reply<Space> {
	let (_, space) = lock(&state).mutate(|doc| ops::delete_notes(doc, &ids))?;
	Ok(space)
}

#[tauri::command]
pub async fn reorder_note(
	id: String,
	section: String,
	index: i64,
	state: State<'_, SharedStore>,
) -> Reply<Space> {
	let (_, space) = lock(&state).mutate(|doc| ops::reorder_note(doc, &id, &section, index))?;
	Ok(space)
}

#[tauri::command]
pub async fn move_notes(
	ids: Vec<String>,
	section: String,
	state: State<'_, SharedStore>,
) -> Reply<Space> {
	let (_, space) = lock(&state).mutate(|doc| ops::move_notes(doc, &ids, &section))?;
	Ok(space)
}

#[tauri::command]
pub async fn merge_notes(ids: Vec<String>, state: State<'_, SharedStore>) -> Reply<Space> {
	let (_, space) = lock(&state).mutate(|doc| ops::merge_notes(doc, &ids))?;
	Ok(space)
}

// --- sections ----------------------------------------------------------------

#[tauri::command]
pub async fn add_section(name: String, state: State<'_, SharedStore>) -> Reply<Space> {
	let (_, space) = lock(&state).mutate(|doc| ops::add_section(doc, &name))?;
	Ok(space)
}

#[tauri::command]
pub async fn rename_section(id: String, name: String, state: State<'_, SharedStore>) -> Reply<Space> {
	let (_, space) = lock(&state).mutate(|doc| ops::rename_section(doc, &id, &name))?;
	Ok(space)
}

#[tauri::command]
pub async fn delete_section(id: String, state: State<'_, SharedStore>) -> Reply<Space> {
	let (_, space) = lock(&state).mutate(|doc| ops::delete_section(doc, &id))?;
	Ok(space)
}

#[tauri::command]
pub async fn reorder_section(id: String, index: i64, state: State<'_, SharedStore>) -> Reply<Space> {
	let (_, space) = lock(&state).mutate(|doc| ops::reorder_section(doc, &id, index))?;
	Ok(space)
}

#[tauri::command]
pub async fn set_active_section(id: String, state: State<'_, SharedStore>) -> Reply<Space> {
	// Navigational rather than structural, so no snapshot (spec 4.3).
	let (_, space) = lock(&state).mutate_no_snapshot(|doc| ops::set_active_section(doc, &id))?;
	Ok(space)
}

// --- undo --------------------------------------------------------------------

#[tauri::command]
pub async fn undo(state: State<'_, SharedStore>) -> Reply<Option<Space>> {
	lock(&state).undo()
}

#[tauri::command]
pub async fn redo(state: State<'_, SharedStore>) -> Reply<Option<Space>> {
	lock(&state).redo()
}

// --- Rust-side entry points --------------------------------------------------

/// Phase 4's capture hook (spec 8.5), taking the handle it already has.
pub fn append_capture(app: &AppHandle, body: &str) -> Reply<String> {
	let state = app.state::<SharedStore>();
	super::append_capture(&state, body)
}
