//! Noticing that somebody else changed the file.
//!
//! The watch is registered against the space file's **containing directory,
//! non-recursively** — never against the file path itself. Git checkouts and
//! atomic-saving editors write-replace rather than modify in place, which
//! invalidates a single-file watch on Windows and leaves it silently dead.
//!
//! Self-write suppression is content-based, not timing-based: on any candidate
//! event the store re-reads the file and compares it to `on_disk_text`. There is
//! no "ignore the next event" flag, because there is no reliable way to know how
//! many events one write produces.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Weak};
use std::time::Duration;

use notify::{RecursiveMode, RecommendedWatcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};

use super::atomic;
use super::error::{Result, StoreError};
use super::events::StoreEvent;
use super::{SharedStore, Store};

pub type SpaceWatcher = Debouncer<RecommendedWatcher, RecommendedCache>;

/// Long enough to absorb the several events one atomic replace produces, and
/// long enough that a routine git checkout — unlink, create, write — is seen as
/// one settled change rather than as a file that briefly vanished.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Watches `path`'s directory and reloads the store when `path` changes.
///
/// Takes a `Weak` to the store rather than an `AppHandle`: the callback needs to
/// reach store state, and going through the shared handle directly keeps this
/// module free of Tauri types and testable without a runtime.
pub fn spawn_watcher(shared: Weak<Mutex<Store>>, path: &Path) -> Result<SpaceWatcher> {
	let dir = atomic::parent_dir(path)?.to_path_buf();
	let file_name = path
		.file_name()
		.ok_or_else(|| StoreError::Invalid(format!("{} names no file", path.display())))?
		.to_os_string();
	// Captured so a callback that outlives its space can prove it is stale.
	let watched: PathBuf = path.to_path_buf();

	let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
		let Some(store) = shared.upgrade() else {
			return;
		};
		match result {
			Ok(events) => {
				// Event *kind* is not used to decide whether the file changed:
				// Create, Modify and a rename's To half are all "look again", and a
				// coalesced rename carries both its source and destination paths,
				// which is why every entry of `paths` is checked rather than the
				// first. `need_rescan` means the backend lost track and forces an
				// unconditional re-read.
				let candidate = events.iter().any(|event| {
					event.need_rescan()
						|| event
							.paths
							.iter()
							.any(|path| path.file_name() == Some(file_name.as_os_str()))
				});
				if candidate {
					handle_external_change(&store, &watched);
				}
			}
			Err(errors) => report_errors(&store, &watched, &errors),
		}
	})
	.map_err(|err| StoreError::Io(format!("could not start watching {}: {err}", dir.display())))?;

	// Called directly on the debouncer. In 0.7.0 both `.watcher()` and `.cache()`
	// are deprecated and root bookkeeping is automatic, so the
	// `debouncer.watcher().watch(...)` pattern in most tutorials is wrong here.
	debouncer
		.watch(&dir, RecursiveMode::NonRecursive)
		.map_err(|err| StoreError::Io(format!("could not watch {}: {err}", dir.display())))?;

	Ok(debouncer)
}

/// Re-reads the space file and applies the decision table in `reload_from_disk`.
///
/// Public because the stale-callback behaviour it guards is worth testing
/// directly (A9.27) and there is no other way to deliver a queued callback on
/// demand.
pub fn handle_external_change(shared: &SharedStore, watched: &Path) {
	let mut guard = super::lock(shared);
	// Dropping a `notify-debouncer-full` debouncer signals its worker to stop but
	// does not join the thread, so a callback queued for a previous space can run
	// after a different space has been opened. Without this guard it would re-read
	// the *new* space's state while reasoning about the *old* space's file.
	if guard.active_path() != Some(watched) {
		return;
	}
	let produced = guard.reload_from_disk();
	super::emit_after(guard, produced);
}

/// One `store-error` for a batch of watcher failures.
fn report_errors(shared: &SharedStore, watched: &Path, errors: &[notify::Error]) {
	let guard = super::lock(shared);
	if guard.active_path() != Some(watched) {
		return;
	}
	let detail = errors
		.iter()
		.map(ToString::to_string)
		.collect::<Vec<_>>()
		.join("; ");
	let event = StoreEvent::error(&StoreError::Io(format!(
		"watching {} reported: {detail}",
		watched.display()
	)));
	super::emit_after(guard, vec![event]);
}
