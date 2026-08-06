//! The five commands the panel reaches attachments through.
//!
//! They follow the store's two conventions unchanged: **every parameter name is
//! a single word**, so Tauri's snake↔camel conversion stays a no-op and the
//! contract has one spelling, and none of them needs a capability entry —
//! app-defined commands are allowed for every window by default, no plugin is
//! added, the file dialog is driven from Rust, and thumbnails travel as bytes
//! rather than through the asset protocol.
//!
//! `file` rather than an attachment id is what [`attachment_thumb`] and
//! [`attachment_open`] take, and the choice is deliberate. The content-addressed
//! name is the one identifier a *pending* attachment and a committed one share —
//! the tray's items are not in the document yet — so an id-keyed command would
//! need a second path for the tray. It is also exactly the value
//! [`super::resolve`] validates, which puts the security check on the argument
//! the caller controls rather than one indirection away from it.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, State};

use crate::spaces;
use crate::store::error::{Result, StoreError};
use crate::store::model::Attachment;
use crate::store::{self, SharedStore};
use crate::win32::clipboard::{self, ClipboardAttachment};

use super::{ingest, read_blob, resolve, thumb, ATTACHMENT_MAX_BYTES};

type Reply<T> = std::result::Result<T, StoreError>;

/// The open space's path, or the reason there is nothing to attach to.
///
/// Every command here starts with this and drops the guard immediately: the
/// path is all they need, and holding the store lock across a file dialog, a
/// clipboard session or an image decode would stall every capture for as long
/// as the user left the dialog open.
fn space_path(state: &SharedStore) -> Result<PathBuf> {
	store::lock(state)
		.active_path()
		.map(Path::to_path_buf)
		.ok_or_else(|| StoreError::Unavailable("no space is open".into()))
}

/// Ingests each path, keeping the successes and the failures apart.
///
/// A multi-file drop with one oversized file in it attaches the rest — the user
/// dropped five things and getting four is better than getting none — so the
/// per-file error is collected rather than returned. The refusals come back as
/// one message the caller shows beside the tray.
fn ingest_paths(space: &Path, paths: &[PathBuf]) -> Result<Vec<Attachment>> {
	let mut attached = Vec::new();
	let mut refused: Vec<String> = Vec::new();

	for path in paths {
		let name = path
			.file_name()
			.map(|name| name.to_string_lossy().into_owned())
			.unwrap_or_else(|| path.display().to_string());

		// A directory is refused rather than traversed. Walking one would turn a
		// mis-aimed drop of a project folder into hundreds of files, and there is no
		// sensible answer to how deep it should go.
		match std::fs::metadata(path) {
			Ok(metadata) if metadata.is_dir() => {
				refused.push(format!("{name} is a folder"));
				continue;
			}
			// Measured before the read, so an enormous file costs a `stat` rather
			// than a full load into memory that is then thrown away.
			Ok(metadata) if metadata.len() > ATTACHMENT_MAX_BYTES => {
				refused.push(format!("{name} is too large"));
				continue;
			}
			Ok(_) => {}
			Err(err) => {
				refused.push(format!("{name} could not be read: {err}"));
				continue;
			}
		}

		match std::fs::read(path).map_err(|err| StoreError::Io(err.to_string())) {
			Ok(bytes) => match ingest(space, &bytes, &name) {
				Ok(attachment) => attached.push(attachment),
				Err(err) => refused.push(err.message()),
			},
			Err(err) => refused.push(format!("{name}: {}", err.message())),
		}
	}

	// Nothing attached and something refused is a plain failure; a partial success
	// is reported as one, because the tray is about to show what did land.
	if attached.is_empty() && !refused.is_empty() {
		return Err(StoreError::Invalid(refused.join("; ")));
	}
	Ok(attached)
}

/// `Ctrl+V` in the composer.
///
/// Returns an empty list — not an error — when the clipboard holds text or
/// nothing attachable. That is the signal the frontend falls through to the
/// native text paste on, and it has to be an ordinary outcome because it is the
/// overwhelmingly common one.
#[tauri::command]
pub async fn attach_paste(state: State<'_, SharedStore>) -> Reply<Vec<Attachment>> {
	let space = space_path(&state)?;

	let found = clipboard::read_attachment()
		.map_err(|err| StoreError::Io(format!("the clipboard could not be read: {err}")))?;

	match found {
		None => Ok(Vec::new()),
		Some(ClipboardAttachment::Files(paths)) => ingest_paths(&space, &paths),
		Some(ClipboardAttachment::Dib(dib)) => {
			// Encoded before storage: a DIB is not a portable file format and must
			// never be written to disk as one.
			let png = thumb::dib_to_png(&dib)?;
			Ok(vec![ingest(&space, &png, "Pasted image.png")?])
		}
	}
}

/// The paperclip button.
///
/// `tauri-plugin-dialog` from Rust inside `spawn_blocking` with `set_parent`,
/// exactly as task-007's `pick_and_open_space` does — and `set_parent` is not
/// optional against an always-on-top panel, which a parentless dialog opens
/// *behind*, reading as a hang.
#[tauri::command]
pub async fn attach_pick(app: AppHandle, state: State<'_, SharedStore>) -> Reply<Vec<Attachment>> {
	let space = space_path(&state)?;
	let picked = spaces::pick_attachment_files(&app).await?;
	if picked.is_empty() {
		// Cancelling is a success with no state change.
		return Ok(Vec::new());
	}
	ingest_paths(&space, &picked)
}

/// The drop path. `paths` comes straight from `tauri://drag-drop`.
#[tauri::command]
pub async fn attach_paths(paths: Vec<String>, state: State<'_, SharedStore>) -> Reply<Vec<Attachment>> {
	let space = space_path(&state)?;
	let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
	ingest_paths(&space, &paths)
}

/// A PNG thumbnail, or **an empty response** when the file is there but has no
/// preview.
///
/// The empty case is the whole reason this is one command rather than two. Every
/// attachment card needs to know three things — is the blob there, can it be
/// previewed, and if so what does it look like — and answering all three in one
/// round trip means a `.pdf` chip proves its file exists by the same call an
/// image proves it with. An *error* therefore means exactly one thing: this
/// attachment is unavailable, and the message says why.
///
/// The bytes go back through `tauri::ipc::Response`, which sends them raw. A
/// plain `Vec<u8>` return would be serialised as a JSON array of numbers and
/// quadruple a thumbnail on the wire for nothing.
#[tauri::command]
pub async fn attachment_thumb(
	file: String,
	state: State<'_, SharedStore>,
) -> Reply<tauri::ipc::Response> {
	let space = space_path(&state)?;
	let bytes = read_blob(&space, &file)?;

	// Sniffed from the bytes on disk, never from the `mime` in the document: that
	// field is hand-editable, and a `.png` that is really an executable must not
	// be handed to a decoder because a JSON key said so (AC22).
	let mime = infer::get(&bytes).map_or("application/octet-stream", |kind| kind.mime_type());
	if !thumb::is_thumbnailable(mime) {
		return Ok(tauri::ipc::Response::new(Vec::new()));
	}
	Ok(tauri::ipc::Response::new(thumb::thumbnail(&bytes, mime)?))
}

/// Images open in the OS viewer; **everything else is revealed in Explorer**.
///
/// Handing an arbitrary file to the shell is an execution surface, and a
/// `.copper` space can arrive from a git remote — a note carrying a `.lnk`,
/// `.hta`, `.reg` or `.ps1` would otherwise be one double-click from running it.
/// Reveal costs the user one more click and removes the surface entirely. The
/// asymmetry is justified because images are the overwhelmingly common case and
/// an image viewer is not an execution vector in that way.
///
/// The path is **reconstructed** through [`super::resolve`] rather than taken
/// from the document, and the type is re-sniffed from the bytes rather than
/// read from `mime` — so neither half of the decision can be steered by editing
/// the JSON.
#[tauri::command]
pub async fn attachment_open(
	file: String,
	app: AppHandle,
	state: State<'_, SharedStore>,
) -> Reply<()> {
	use tauri_plugin_opener::OpenerExt;

	let space = space_path(&state)?;
	let path = resolve(&space, &file)?;
	if !path.is_file() {
		return Err(StoreError::NotFound(format!(
			"{file} is missing from this space's attachments"
		)));
	}

	let is_image = std::fs::read(&path)
		.ok()
		.and_then(|bytes| infer::get(&bytes).map(|kind| kind.mime_type().to_string()))
		.is_some_and(|mime| thumb::is_thumbnailable(&mime));

	let target = path.to_string_lossy().to_string();
	let opened = if is_image {
		app.opener().open_path(target, None::<&str>)
	} else {
		app.opener().reveal_item_in_dir(&path)
	};
	opened.map_err(|err| StoreError::Io(format!("could not open {}: {err}", path.display())))
}

/// Collects the open space's unreferenced blobs, with **no store lock held**.
///
/// The path and the document are cloned out under the guard and the guard is
/// dropped before the directory is touched, following task-006's two-lock
/// discipline: a sweep holding the store mutex would stall every capture for the
/// length of a directory walk, and on a slow network share that is not a short
/// time.
///
/// Called at space close and at startup only — never during a session. The undo
/// stack is session-scoped, so a mid-session sweep would silently turn a
/// restorable `Ctrl+Z` into a note whose attachments no longer exist.
pub fn sweep_active_space(app: &AppHandle) {
	let state = app.state::<SharedStore>();
	let held = {
		let guard = store::lock(&state);
		guard
			.active_path()
			.map(Path::to_path_buf)
			.zip(guard.active_space().ok())
	};
	if let Some((path, doc)) = held {
		super::sweep(&path, &doc);
	}
}
