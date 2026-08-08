//! The twenty commands Phase 3 codes against (spec 8.1), plus the one task-010
//! added — and nothing else.
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

use crate::entry::{classify, Entry};

use super::error::StoreError;
use super::model::{Attachment, Space};
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

/// Which of the three things a composer submission turned out to be.
///
/// Kebab-case on the wire, matching how `StoreError::kind` and `ShellError::kind`
/// already spell a discriminant the frontend branches on.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SubmitOutcome {
	Note,
	SectionCreated,
	SectionActivated,
}

/// What `submit_entry` gives back.
///
/// Richer than `AddNoteResult` because the caller cannot tell from the document
/// alone what it just did: the panel puts the roving focus on a new note, and
/// must **not** move it when the submission created or switched a section
/// instead. `noteId` is null on both section outcomes; `sectionId` always names
/// the section the submission concerned, so no follow-up round trip is needed.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SubmitResult {
	pub space: Space,
	pub outcome: SubmitOutcome,
	pub note_id: Option<String>,
	pub section_id: String,
}

// --- settings and status -----------------------------------------------------

#[tauri::command]
pub async fn get_settings(state: State<'_, SharedStore>) -> Reply<Settings> {
	Ok(lock(&state).settings().clone())
}

/// Delegates to the module's Rust-callable seam rather than locking here, so the
/// frontend's writer and Phase 7's are literally the same call.
#[tauri::command]
pub async fn update_settings(patch: SettingsPatch, state: State<'_, SharedStore>) -> Reply<Settings> {
	super::patch_settings(&state, patch)
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
	add(&state, &body, section.as_deref())
}

/// The body of [`add_note`], as a plain function over the shared store.
///
/// Split out for the same reason [`submit`] is: `cargo test` has no Tauri runtime
/// and so cannot construct a `State`, and asserting the insertion-point behaviour
/// by re-implementing the command in the test would prove only that the test
/// agrees with itself.
pub fn add(shared: &SharedStore, body: &str, section: Option<&str>) -> Reply<AddNoteResult> {
	let mut guard = lock(shared);
	// Read here rather than taken as a parameter: `insertAt` would be the store's
	// first multi-word parameter name, and the capture path — which has no frontend
	// caller to pass one — has to agree with this one anyway.
	let at = guard.settings().insertion();
	let (note_id, space) = guard.mutate(|doc| ops::add_note(doc, body, section, &[], at))?;
	Ok(AddNoteResult { space, note_id })
}

/// The composer's submit, and the only entry point that reads a body as
/// anything but opaque text.
///
/// `add_note` stays exactly as it was, and is what the capture path still
/// reaches: a captured selection whose whole body is `# Name` is an ordinary
/// note (Open Question 1, answered 2026-08-05). Inline section creation is a
/// composer affordance, so it lives on the composer's command.
///
/// `attachments` is the one parameter this task adds, and it is optional so that
/// every existing caller — and every future one that has nothing to attach —
/// keeps working unchanged. Single word, per spec 8.1c, so Tauri's snake↔camel
/// conversion stays a no-op and the contract has one spelling.
///
/// The blobs are already on disk by the time this runs: ingestion happens at
/// paste, drop and pick time so the composer tray can show real metadata
/// immediately, which makes submit a metadata-only document write.
#[tauri::command]
pub async fn submit_entry(
	body: String,
	attachments: Option<Vec<Attachment>>,
	state: State<'_, SharedStore>,
) -> Reply<SubmitResult> {
	submit(&state, &body, &attachments.unwrap_or_default())
}

/// The body of [`submit_entry`], as a plain function over the shared store.
///
/// Split out for the same reason [`super::append_capture`] is a module-level
/// seam rather than command-only code: `cargo test` has no Tauri runtime and so
/// cannot construct a `State`, and asserting the snapshot behaviour by
/// re-implementing the command in the test would prove only that the test agrees
/// with itself.
pub fn submit(shared: &SharedStore, body: &str, attachments: &[Attachment]) -> Reply<SubmitResult> {
	let mut guard = lock(shared);

	// The blobs have to be in **this** space's assets directory. Checked here
	// rather than in `ops`, which is deliberately pure over the document — every
	// op has to be re-appliable against a re-read document after a write
	// conflict, and an op that touched the filesystem could not be.
	//
	// The case is a space switch with a loaded tray: bytes ingested against A
	// live in `A.copper.assets\`, and writing them into B's document would leave
	// references to files that will never exist there. The frontend clears the
	// tray on a switch; this is the half that does not depend on it having done so.
	if !attachments.is_empty() {
		let space = guard.require_active_path()?;
		crate::attachments::commands::require_present(&space, attachments)?;
	}

	let at = guard.settings().insertion();
	let name = match classify(body) {
		Entry::Note { body } => {
			let (note_id, space) =
				guard.mutate(|doc| ops::add_note(doc, body, None, attachments, at))?;
			// Read back off the document rather than tracked through the op: the
			// store defaults an unaddressed note to `activeSection`, and re-deriving
			// that here would be a second copy of a rule that can change.
			let section_id = space
				.note(&note_id)
				.map(|note| note.section.clone())
				.unwrap_or_default();
			return Ok(SubmitResult {
				space,
				outcome: SubmitOutcome::Note,
				note_id: Some(note_id),
				section_id,
			});
		}
		Entry::Section { name } => name,
	};

	// A `# Name` line creates a section, and a section holds no files — so a
	// submission that is both is refused rather than resolved one way or the
	// other. Silently dropping the attachments would destroy work the tray still
	// shows; silently making it a note would create a note the user did not ask
	// for and hide the section they did. A refusal clears nothing, so the pending
	// tray survives and either fix is one keystroke away.
	if !attachments.is_empty() {
		return Err(StoreError::Invalid(
			"a section heading cannot carry attachments. Remove them, or add the files to a note \
			 instead"
				.into(),
		));
	}

	// Creating a section is structural and undoable; resolving a duplicate name to
	// one that already exists only moves `activeSection`, and must push nothing —
	// matching `set_active_section`'s exclusion (spec 4.3).
	//
	// The decision goes *through* `mutate_if` rather than being taken here,
	// because here is too early: a write conflict rebases the operation onto the
	// external document, where the section may have appeared or vanished, and a
	// decision made against the local view would then describe a mutation that did
	// not happen. The predicate is asked again for each attempt, against the
	// document the op is actually applied to.
	let ((section_id, created), space) = guard.mutate_if(
		|doc| ops::add_section_and_activate(doc, &name),
		|doc| ops::section_by_name(doc, &name).is_none(),
	)?;

	Ok(SubmitResult {
		space,
		outcome: if created {
			SubmitOutcome::SectionCreated
		} else {
			SubmitOutcome::SectionActivated
		},
		note_id: None,
		section_id,
	})
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
pub fn append_capture(app: &AppHandle, body: &str) -> Reply<super::Landed> {
	let state = app.state::<SharedStore>();
	super::append_capture(&state, body)
}

/// Phase 7's settings writer, taking the handle it already has.
pub fn patch_settings(app: &AppHandle, patch: SettingsPatch) -> Reply<Settings> {
	let state = app.state::<SharedStore>();
	super::patch_settings(&state, patch)
}

/// The settings as Phase 7's startup steps read them, without a command round
/// trip.
pub fn settings(app: &AppHandle) -> Settings {
	let state = app.state::<SharedStore>();
	let settings = lock(&state).settings().clone();
	settings
}
