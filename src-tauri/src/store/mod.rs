//! The store: everything Copper writes to disk, and the in-memory document it
//! writes from.
//!
//! Plain Rust with a thin Tauri shell. Nothing in this module or its siblings
//! mentions a `tauri::` type except `events::AppSink` and `commands`, so the
//! whole of the interesting behaviour — the model, the format, the operations,
//! undo, conflict handling, watching — is testable with ordinary `cargo test`
//! and no mock runtime.
//!
//! # The one pipeline
//!
//! Every mutation goes through [`Store::mutate`]. The shape has three properties
//! that are load-bearing rather than incidental, and each of them is a defect
//! that a plausible implementation would have:
//!
//! - **`expected_text` is attempt-local**, seeded from `on_disk_text` and
//!   replaced by the freshly read text after each conflict. Comparing every
//!   attempt against the shared field instead would guarantee that all three
//!   attempts conflict whenever the file has moved on even once, turning every
//!   recoverable conflict into a hard failure.
//! - **The re-read document is normalised before the operation is re-applied.**
//!   An external writer is under no obligation to have written canonical bytes,
//!   and an index- or order-sensitive operation applied to a non-canonical
//!   document targets the wrong position.
//! - **The undo snapshot is the document the operation was finally applied to.**
//!   After a conflict that is the *external* document. Pushing the one held in
//!   memory when the mutation started means a later `Ctrl+Z` silently discards
//!   someone else's change — a git checkout reverted with no indication.
//!
//! # Two rules about the lock
//!
//! The compare, the write and the `on_disk_text` update happen under one lock
//! acquisition (spec 2.6). Without that the watcher thread could observe the
//! file already replaced while `on_disk_text` still held the old text, and
//! report the store's own write as an external change.
//!
//! And **no event is ever emitted while the guard is held** (spec 2.10). Every
//! state method returns its events to the caller, which drops the guard before
//! emitting. Tauri dispatches to Rust-side listeners synchronously on the
//! emitting thread, so a listener that touched store state would deadlock
//! against `std::sync::Mutex`, which is not reentrant.

pub mod atomic;
pub mod bootstrap;
pub mod commands;
pub mod error;
pub mod events;
pub mod format;
pub mod ids;
pub mod model;
pub mod ops;
pub mod settings;
pub mod undo;
pub mod watch;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use serde::Serialize;

use atomic::{Attempt, Prepared};
use error::{io_err, Result, StoreError};
use events::{ChangeReason, EventSink, SpaceChanged, StoreEvent};
use model::{Section, Space, DEFAULT_SECTION_NAME};
use settings::{Settings, SettingsPatch};
use undo::UndoStack;
use watch::SpaceWatcher;

/// Spec 2.3. Three re-applies is enough for a human-paced external editor and
/// short enough that a pathological one fails fast instead of livelocking.
const MAX_CONFLICT_ATTEMPTS: usize = 3;

/// The store as the app holds it.
///
/// `Arc` rather than a bare `Mutex` in Tauri's state because the watcher
/// callback needs a `Weak` back to it — which also keeps `watch.rs` free of any
/// Tauri type and testable on its own.
pub type SharedStore = Arc<Mutex<Store>>;

/// Acquires the store, tolerating a poisoned mutex.
///
/// A panic while holding the guard leaves the store consistent — every method
/// commits to disk before it commits in memory — so refusing to work afterwards
/// would only turn one failure into permanent unavailability.
pub fn lock(shared: &SharedStore) -> MutexGuard<'_, Store> {
	shared.lock().unwrap_or_else(|err| err.into_inner())
}

/// The currently open space and everything that is true only while it is open.
pub struct OpenSpace {
	path: PathBuf,
	doc: Space,
	/// The exact text last read from or written to the file.
	///
	/// Exact text rather than an mtime or a hash: documents are tens of
	/// kilobytes, and `Metadata::modified()` plus `len()` is not a sound change
	/// check on NTFS — two writes can land in the same timestamp tick at the same
	/// length. This one field is both the self-write suppressor and the conflict
	/// detector, which is why they can never disagree.
	on_disk_text: String,
	undo: UndoStack,
	/// Held here so it is not dropped early; dropping a debouncer silently stops
	/// the watch.
	watcher: Option<SpaceWatcher>,
	/// The document on disk is unreadable or unparseable. Blocks mutations.
	doc_error: Option<String>,
	/// The watch could not be registered. Does **not** block mutations.
	///
	/// Separate from `doc_error` deliberately (spec 3.7a): the two have opposite
	/// consequences, and one conflated field would make an unwatchable space
	/// read-only — precisely the outcome spec 3.7 exists to prevent.
	watch_error: Option<String>,
}

impl OpenSpace {
	fn load(path: &Path) -> Result<Self> {
		let text = std::fs::read_to_string(path).map_err(|err| io_err(path, "read", &err))?;
		let mut doc = format::from_json(&text)?;
		format::normalise(&mut doc);
		Ok(Self {
			path: path.to_path_buf(),
			doc,
			// The text as read, not a re-serialisation of the normalised document:
			// the compare baseline has to match what is actually on disk, and spec
			// 7.4 forbids rewriting a file merely because loading it tidied it.
			on_disk_text: text,
			undo: UndoStack::default(),
			watcher: None,
			doc_error: None,
			watch_error: None,
		})
	}
}

pub struct Store {
	settings: Settings,
	settings_path: PathBuf,
	spaces_dir: PathBuf,
	open: Option<OpenSpace>,
	startup_notice: Option<String>,
	sink: Arc<dyn EventSink>,
}

/// Persistent state that no `Space` payload carries (spec 8.1a).
///
/// `store-error` events are transient; "this space is unreadable" and "undo is
/// available" are states the panel has to be able to ask about.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoreStatus {
	pub path: Option<String>,
	pub errored: bool,
	pub watching: bool,
	pub can_undo: bool,
	pub can_redo: bool,
	pub startup_notice: Option<String>,
}

impl Store {
	// --- reads ---------------------------------------------------------------

	pub fn settings(&self) -> &Settings {
		&self.settings
	}

	/// Spec 6.8. For later phases that operate on paths from Rust, where a
	/// command round trip is the wrong tool.
	pub fn spaces_dir(&self) -> &Path {
		&self.spaces_dir
	}

	pub fn recents(&self) -> &[String] {
		&self.settings.recents
	}

	pub fn active_path(&self) -> Option<&Path> {
		self.open.as_ref().map(|open| open.path.as_path())
	}

	/// The bytes the store believes are on disk, for tests and diagnostics.
	pub fn on_disk_text(&self) -> Option<&str> {
		self.open.as_ref().map(|open| open.on_disk_text.as_str())
	}

	pub fn active_space(&self) -> Result<Space> {
		self.open
			.as_ref()
			.map(|open| open.doc.clone())
			.ok_or_else(no_space)
	}

	pub fn status(&self) -> StoreStatus {
		let open = self.open.as_ref();
		StoreStatus {
			path: open.map(|open| path_string(&open.path)),
			errored: open.is_some_and(|open| open.doc_error.is_some()),
			// Read from the live debouncer rather than from `watch_error.is_none()`
			// so the window between bootstrap and `attach_watcher` reports honestly.
			// The two agree everywhere else: a watcher is only ever installed when
			// registration succeeded.
			//
			// This reflects the *registration outcome* only. A watch broken later —
			// by deleting the watched directory, say — is not detectable, and spec
			// 3.9 accepts that rather than reporting a `watching: true` that lies.
			watching: open.is_some_and(|open| open.watcher.is_some()),
			can_undo: open.is_some_and(|open| open.undo.can_undo()),
			can_redo: open.is_some_and(|open| open.undo.can_redo()),
			startup_notice: self.startup_notice.clone(),
		}
	}

	pub fn startup_notice(&self) -> Option<&str> {
		self.startup_notice.as_deref()
	}

	/// Appends rather than overwrites (spec 6.6a).
	///
	/// More than one startup notice can legitimately occur in a launch — a corrupt
	/// `settings.json` and, from Phase 6, a file passed on the command line that
	/// failed to open. These are independent failures and the user needs both, so
	/// a last-writer-wins slot would silently drop a real one. The wire field
	/// stays a single string, which is what Phase 3 codes against.
	pub fn push_startup_notice(&mut self, reason: impl Into<String>) {
		let reason = reason.into();
		match &mut self.startup_notice {
			Some(existing) => {
				existing.push('\n');
				existing.push_str(&reason);
			}
			None => self.startup_notice = Some(reason),
		}
	}

	// --- the pipeline --------------------------------------------------------

	/// Applies `op`, writes the result, and records an undo snapshot.
	///
	/// `op` must be `Fn`, not `FnOnce`: an `FnOnce` is consumed by its first call
	/// and could not be re-applied to a freshly parsed document, which is the
	/// entire mechanism of the conflict path below.
	pub fn mutate<T>(&mut self, op: impl Fn(&mut Space) -> Result<T>) -> Result<(T, Space)> {
		self.mutate_with(op, true)
	}

	/// The same pipeline without the snapshot, for `edit_note` and
	/// `set_active_section` (spec 4.3) — text editing uses the browser's native
	/// undo, and changing the active section is navigational.
	pub fn mutate_no_snapshot<T>(
		&mut self,
		op: impl Fn(&mut Space) -> Result<T>,
	) -> Result<(T, Space)> {
		self.mutate_with(op, false)
	}

	fn mutate_with<T>(
		&mut self,
		op: impl Fn(&mut Space) -> Result<T>,
		snapshot: bool,
	) -> Result<(T, Space)> {
		let open = self.open.as_mut().ok_or_else(no_space)?;
		// A failed *watch* deliberately does not land here: the space stays fully
		// writable, it simply will not notice external edits.
		if let Some(reason) = &open.doc_error {
			return Err(StoreError::Unavailable(format!(
				"this space cannot be written while its file is unreadable: {reason}"
			)));
		}

		let path = open.path.clone();
		let dir = atomic::parent_dir(&path)?.to_path_buf();
		let mut base = open.doc.clone();
		let mut expected = open.on_disk_text.clone();

		for _ in 0..MAX_CONFLICT_ATTEMPTS {
			// The pre-operation state for undo, captured per attempt so that after a
			// conflict it is the external document rather than the stale local one.
			let pre_op = base.clone();
			let mut working = base.clone();
			// Validate-then-mutate: an `op` that fails has changed nothing but its
			// own scratch copy, so nothing reaches disk and no snapshot is pushed.
			let value = op(&mut working)?;
			format::normalise(&mut working);
			let text = format::to_git_json(&working)?;

			match commit_against(&path, &dir, &text, &expected)? {
				Commit::Done => {
					open.doc = working;
					open.on_disk_text = text;
					if snapshot {
						open.undo.push(pre_op);
					}
					return Ok((value, open.doc.clone()));
				}
				Commit::Conflicted { text, doc } => {
					base = doc;
					expected = text;
				}
			}
		}

		Err(StoreError::Conflict(format!(
			"{} kept changing underneath this edit; nothing was written",
			path.display()
		)))
	}

	/// Restores the previous document (spec 4.2), or `None` on an empty stack.
	pub fn undo(&mut self) -> Result<Option<Space>> {
		self.restore(true)
	}

	pub fn redo(&mut self) -> Result<Option<Space>> {
		self.restore(false)
	}

	/// Whole-document restore.
	///
	/// This is the one operation the conflict re-apply does not fit: re-applying
	/// a whole document against fresh disk state does not merge anything, it
	/// overwrites the external change wholesale — the exact outcome the snapshot
	/// rule above exists to prevent. So it **fails** on conflict instead (spec
	/// 4.8). Failing is recoverable, since the user retries once the reload has
	/// landed; silently clobbering someone else's change is not.
	///
	/// The stacks move only after the write has committed (spec 4.7), so a failed
	/// undo leaves both stacks, the document and the file exactly as they were.
	fn restore(&mut self, undoing: bool) -> Result<Option<Space>> {
		let open = self.open.as_mut().ok_or_else(no_space)?;
		if let Some(reason) = &open.doc_error {
			return Err(StoreError::Unavailable(format!(
				"this space cannot be written while its file is unreadable: {reason}"
			)));
		}

		let peeked = if undoing {
			open.undo.peek_undo()
		} else {
			open.undo.peek_redo()
		};
		// An empty stack returns null rather than erroring (spec 4.5).
		let Some(target) = peeked.cloned() else {
			return Ok(None);
		};

		let mut restored = target;
		format::normalise(&mut restored);
		let text = format::to_git_json(&restored)?;
		let path = open.path.clone();
		let dir = atomic::parent_dir(&path)?.to_path_buf();

		match commit_against(&path, &dir, &text, &open.on_disk_text)? {
			Commit::Done => {}
			Commit::Conflicted { .. } => {
				return Err(StoreError::Conflict(format!(
					"{} changed outside Copper; reload before undoing",
					path.display()
				)))
			}
		}

		let previous = std::mem::replace(&mut open.doc, restored);
		open.on_disk_text = text;
		if undoing {
			open.undo.commit_undo(previous);
		} else {
			open.undo.commit_redo(previous);
		}
		Ok(Some(open.doc.clone()))
	}

	// --- settings ------------------------------------------------------------

	/// Returns the updated settings, not a `Space` — this touches no document.
	pub fn update_settings(&mut self, patch: SettingsPatch) -> Result<Settings> {
		let mut next = self.settings.clone();
		next.apply_patch(patch);
		// Disk first, memory second, so a failed save leaves nothing half-applied.
		settings::save(&self.settings_path, &next)?;
		self.settings = next;
		Ok(self.settings.clone())
	}

	/// Forgets a recents entry (spec 6.7).
	///
	/// Bookkeeping, not a close: removing the currently open space's entry leaves
	/// it open. Returns its `settings-changed` payload rather than emitting, so
	/// the caller emits after dropping the guard.
	pub fn remove_recent(&mut self, path: &Path) -> Result<StoreEvent> {
		let target = canonical_or_raw(path);
		let mut next = self.settings.clone();
		next.forget_recent(&target);
		// Re-point at the still-open space if its path survived, clamp otherwise.
		let open_path = self.open.as_ref().map(|open| path_string(&open.path));
		next.point_at(open_path.as_deref());

		// Removing a path that was never there is a no-op rather than an error:
		// the desired end state already holds.
		if next != self.settings {
			settings::save(&self.settings_path, &next)?;
			self.settings = next;
		}
		Ok(StoreEvent::settings_changed())
	}

	// --- opening -------------------------------------------------------------

	/// Builds the new space completely before touching the current one, so any
	/// failure — bad path, unreadable, unparseable, or a settings save that will
	/// not go through — leaves the previous space open, watched and unchanged
	/// (spec 8.1b).
	fn open_space_locked(
		&mut self,
		path: &Path,
		weak: Weak<Mutex<Store>>,
	) -> Result<(Space, Vec<StoreEvent>)> {
		let path = canonical(path)?;
		if path.is_dir() {
			return Err(StoreError::Invalid(format!(
				"{} is a folder, not a space",
				path.display()
			)));
		}
		let opened = OpenSpace::load(&path)?;

		let mut next = self.settings.clone();
		next.touch_recent(&path_string(&path));
		settings::save(&self.settings_path, &next)?;

		self.settings = next;
		// Dropping the previous `OpenSpace` drops its debouncer, which unwatches
		// the old directory (spec 3.8).
		self.open = Some(opened);
		let watch_event = self.attach_watcher_locked(weak);

		let doc = self.open.as_ref().expect("just assigned").doc.clone();
		let mut produced = vec![StoreEvent::settings_changed()];
		produced.extend(watch_event);
		Ok((doc, produced))
	}

	fn create_space_locked(
		&mut self,
		path: &Path,
		name: &str,
		weak: Weak<Mutex<Store>>,
	) -> Result<(Space, Vec<StoreEvent>)> {
		let name = name.trim();
		if name.is_empty() {
			return Err(StoreError::Invalid("a space needs a name".into()));
		}
		let dir = atomic::parent_dir(path)?;
		std::fs::create_dir_all(dir).map_err(|err| io_err(dir, "create", &err))?;

		let text = format::to_git_json(&new_space(name))?;
		// No-clobber create rather than an `exists()` check followed by a replacing
		// persist: the refusal comes from the filesystem, so a file appearing
		// between the two has no window to be destroyed in (A9.30).
		atomic::prepare(dir, &text)?
			.commit_new(path)
			.map_err(|failure| {
				if failure.error.kind() == std::io::ErrorKind::AlreadyExists {
					StoreError::Invalid(format!("{} already exists", path.display()))
				} else {
					io_err(path, "create", &failure.error)
				}
			})?;

		// Opening it is what produces the single `settings-changed` (spec 8.4).
		self.open_space_locked(path, weak)
	}

	/// Registers the watch for the already-open space.
	///
	/// Returns a `store-error` payload on failure rather than emitting it; the
	/// startup caller discards it (nothing is listening yet, spec 8A.2) while
	/// `open_space` passes it on. The space stays open and fully writable either
	/// way (spec 3.7).
	fn attach_watcher_locked(&mut self, weak: Weak<Mutex<Store>>) -> Option<StoreEvent> {
		let open = self.open.as_mut()?;
		open.watcher = None;
		match watch::spawn_watcher(weak, &open.path) {
			Ok(watcher) => {
				open.watcher = Some(watcher);
				open.watch_error = None;
				None
			}
			Err(err) => {
				let event = StoreEvent::error(&err);
				open.watch_error = Some(err.message());
				Some(event)
			}
		}
	}

	/// Drops the open space and its watch.
	pub fn close_space(&mut self) {
		self.open = None;
	}

	// --- watcher support -----------------------------------------------------

	/// The decision table for an external-change candidate (spec 3.3 – 3.6a).
	///
	/// Returns the events to emit once the caller has dropped the guard.
	fn reload_from_disk(&mut self) -> Vec<StoreEvent> {
		let Some(open) = self.open.as_mut() else {
			return Vec::new();
		};
		let path = open.path.clone();

		let text = match std::fs::read_to_string(&path) {
			Ok(text) => text,
			Err(err) => return open.mark_errored(io_err(&path, "read", &err)),
		};

		// Our own write, so nothing to do — unless the space is errored, in which
		// case the short-circuit has to be skipped (spec 3.3a). Otherwise recovery
		// is unreachable by its most likely route: restoring a byte-identical good
		// file would match `on_disk_text`, be dismissed as a self-write, and leave
		// the space errored forever with a perfectly good document on disk.
		if text == open.on_disk_text && open.doc_error.is_none() {
			return Vec::new();
		}

		let doc = match format::parse_normalised(&text) {
			Ok(doc) => doc,
			Err(err) => return open.mark_errored(err),
		};

		let was_errored = open.doc_error.take().is_some();
		open.on_disk_text = text;

		// Normalising before comparing is required: the in-memory document is
		// always canonical, an externally written one need not be, and comparing
		// raw parsed structures would report a canonically identical document as a
		// change (spec 3.4).
		if doc == open.doc {
			// A semantic no-op earns no UI churn — but `errored` clearing *is*
			// observable, and the panel has no other way to learn about it.
			return if was_errored {
				vec![open.changed(ChangeReason::Reload)]
			} else {
				Vec::new()
			};
		}

		open.doc = doc;
		// The stacks describe a document that is no longer on disk (spec 4.6).
		open.undo.clear();
		let reason = if was_errored {
			ChangeReason::Reload
		} else {
			ChangeReason::External
		};
		vec![open.changed(reason)]
	}
}

impl OpenSpace {
	/// The in-memory document is **kept**, not discarded: a git checkout in
	/// progress or a failed merge must not cost the user what is on screen. The
	/// watch stays registered, so a later fix recovers automatically.
	fn mark_errored(&mut self, err: StoreError) -> Vec<StoreEvent> {
		self.doc_error = Some(err.message());
		vec![StoreEvent::error(&err)]
	}

	fn changed(&self, reason: ChangeReason) -> StoreEvent {
		StoreEvent::SpaceChanged(SpaceChanged {
			id: self.doc.id.clone(),
			path: path_string(&self.path),
			reason,
		})
	}
}

// --- entry points that need the shared handle --------------------------------

/// Builds the store. The first of the two startup stages (spec 7.5).
pub fn bootstrap_store(config_dir: &Path, sink: Arc<dyn EventSink>) -> Result<Store> {
	let built = bootstrap::init(config_dir)?;
	Ok(Store {
		settings: built.settings,
		settings_path: built.settings_path,
		spaces_dir: built.spaces_dir,
		open: built.open,
		startup_notice: built.startup_notice,
		sink,
	})
}

/// The second startup stage, run after `app.manage` (spec 7.5).
///
/// It cannot happen inside `bootstrap::init`, because the watcher callback
/// resolves the store through the handle that `init` is still in the middle of
/// producing — a watch registered there would have a live callback with nothing
/// to resolve.
pub fn attach_watcher(shared: &SharedStore) -> Option<StoreEvent> {
	let weak = Arc::downgrade(shared);
	lock(shared).attach_watcher_locked(weak)
}

pub fn open_space(shared: &SharedStore, path: &Path) -> Result<Space> {
	let weak = Arc::downgrade(shared);
	let mut guard = lock(shared);
	let (doc, produced) = guard.open_space_locked(path, weak)?;
	emit_after(guard, produced);
	Ok(doc)
}

pub fn create_space(shared: &SharedStore, path: &Path, name: &str) -> Result<Space> {
	let weak = Arc::downgrade(shared);
	let mut guard = lock(shared);
	let (doc, produced) = guard.create_space_locked(path, name, weak)?;
	emit_after(guard, produced);
	Ok(doc)
}

/// Phase 4's entry point: append a captured note without touching store
/// internals. Contains no capture logic.
///
/// **The write is the commit point.** Once it has succeeded this returns `Ok`
/// whatever the emit does (spec 8.5a) — the note is on disk, and reporting a
/// durable write as failed would send Phase 4's user-visible failure path into a
/// retry that duplicates the note.
pub fn append_capture(shared: &SharedStore, body: &str) -> Result<String> {
	let body = body.to_string();
	let mut guard = lock(shared);
	let (id, doc) = guard.mutate(|space| ops::add_note(space, &body, None))?;
	let path = guard.active_path().map(path_string).unwrap_or_default();
	let produced = vec![StoreEvent::SpaceChanged(SpaceChanged {
		id: doc.id,
		path,
		reason: ChangeReason::Capture,
	})];
	emit_after(guard, produced);
	Ok(id)
}

/// Drops the guard, then emits. The order is the whole point (spec 2.10).
fn emit_after(guard: MutexGuard<'_, Store>, produced: Vec<StoreEvent>) {
	let sink = Arc::clone(&guard.sink);
	drop(guard);
	for event in &produced {
		sink.emit(event);
	}
}

// --- the write step ----------------------------------------------------------

enum Commit {
	Done,
	Conflicted { text: String, doc: Space },
}

/// Prepare, compare, commit — in that order, with the retry wrapping all three.
///
/// The ordering is what makes spec 2.7's accepted race narrow: because the temp
/// file is written and synced *before* the comparison, the window between
/// "checked the file" and "replaced the file" is the rename call rather than the
/// whole serialise-and-fsync sequence. It cannot be closed entirely, and the
/// asymmetry is deliberate — the loser in that window is an external writer such
/// as git, whose content is recoverable from the repository, whereas a dropped
/// capture is not recoverable at all.
///
/// The retry wraps the **comparison as well as the commit** (spec 2.2a). A bare
/// persist-retry loop would be unsafe here: the backoff between attempts is
/// hundreds of milliseconds, ample for an external writer to land, and a blind
/// retry would then overwrite it.
///
/// A read or parse failure during the comparison is transient, not fatal, until
/// the backoff is exhausted (spec 2.3b). Git's checkout is not an atomic rename —
/// it unlinks the working-tree entry and writes the replacement in place — so a
/// space file being checked out is legitimately absent or partial for a short
/// window, and failing a capture on first sight of that would lose captures
/// during ordinary git operations. A genuinely unparseable file, such as one
/// holding conflict markers, survives every attempt and is still refused.
fn commit_against(path: &Path, dir: &Path, text: &str, expected: &str) -> Result<Commit> {
	let mut held: Option<Prepared> = None;
	atomic::with_backoff(|| {
		let prepared = match held.take() {
			Some(prepared) => prepared,
			None => match atomic::prepare(dir, text) {
				Ok(prepared) => prepared,
				Err(err) => return Attempt::Failed(err),
			},
		};

		let current = match std::fs::read_to_string(path) {
			Ok(current) => current,
			Err(err) => {
				held = Some(prepared);
				return Attempt::Transient(io_err(path, "read", &err));
			}
		};

		if current != *expected {
			return match format::parse_normalised(&current) {
				Ok(doc) => Attempt::Done(Commit::Conflicted {
					text: current,
					doc,
				}),
				Err(err) => {
					held = Some(prepared);
					Attempt::Transient(err)
				}
			};
		}

		match prepared.commit(path) {
			Ok(()) => Attempt::Done(Commit::Done),
			Err(failure) => {
				let transient = atomic::is_transient_commit_failure(&failure.error);
				let err = io_err(path, "write", &failure.error);
				if transient {
					held = Some(failure.prepared);
					Attempt::Transient(err)
				} else {
					Attempt::Failed(err)
				}
			}
		}
	})
}

// --- paths -------------------------------------------------------------------

/// Resolves a path and strips the verbatim prefix.
///
/// `std::fs::canonicalize` emits `\\?\C:\...` on Windows. That prefix would leak
/// into the switcher UI and break comparison against user-supplied paths, so no
/// path ever reaches `recents` with it attached (spec 6.4, A9.17).
pub fn canonical(path: &Path) -> Result<PathBuf> {
	let resolved = std::fs::canonicalize(path).map_err(|err| io_err(path, "resolve", &err))?;
	Ok(strip_verbatim(resolved))
}

/// Canonicalises when possible, falling back to the path as given.
///
/// A recents entry can name a file that no longer exists — a repo that is not
/// checked out right now — and `remove_recent` still has to be able to match it.
fn canonical_or_raw(path: &Path) -> String {
	canonical(path)
		.map(|resolved| path_string(&resolved))
		.unwrap_or_else(|_| path_string(path))
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
	let text = path.to_string_lossy().into_owned();
	if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
		return PathBuf::from(format!(r"\\{rest}"));
	}
	if let Some(rest) = text.strip_prefix(r"\\?\") {
		return PathBuf::from(rest);
	}
	path
}

pub fn path_string(path: &Path) -> String {
	path.to_string_lossy().into_owned()
}

fn no_space() -> StoreError {
	StoreError::Unavailable("no space is open".into())
}

/// A brand-new document: one section, named per the design, already active.
fn new_space(name: &str) -> Space {
	let section = Section {
		id: ids::new_id(ids::SECTION),
		name: DEFAULT_SECTION_NAME.to_string(),
		order: 0,
	};
	Space {
		id: ids::new_id(ids::SPACE),
		name: name.to_string(),
		active_section: section.id.clone(),
		sections: vec![section],
		notes: Vec::new(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn verbatim_prefixes_are_stripped() {
		assert_eq!(
			strip_verbatim(PathBuf::from(r"\\?\C:\Projects\notes.copper")),
			PathBuf::from(r"C:\Projects\notes.copper")
		);
		assert_eq!(
			strip_verbatim(PathBuf::from(r"\\?\UNC\server\share\notes.copper")),
			PathBuf::from(r"\\server\share\notes.copper")
		);
		assert_eq!(
			strip_verbatim(PathBuf::from(r"C:\Projects\notes.copper")),
			PathBuf::from(r"C:\Projects\notes.copper")
		);
	}

	#[test]
	fn a_new_space_has_one_active_section_and_no_notes() {
		let space = new_space("personal");
		assert_eq!(space.name, "personal");
		assert_eq!(space.sections.len(), 1);
		assert_eq!(space.sections[0].name, DEFAULT_SECTION_NAME);
		assert_eq!(space.active_section, space.sections[0].id);
		assert!(space.notes.is_empty());
		assert!(space.id.starts_with("spc_"));
	}
}
