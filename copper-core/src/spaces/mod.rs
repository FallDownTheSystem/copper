//! Space *identity*: what names a space, whether it can be opened right now, and
//! how a launch argument turns into a path.
//!
//! Two rules hold this layer together, and both are the reason it can live below
//! the app rather than inside it:
//!
//! - **Comparison never touches the filesystem.** [`paths::comparison_key`] is
//!   purely lexical, so two spellings of one path answer the same whether or not
//!   the drive is attached.
//! - **Availability is probed, never persisted.** A stale-looking entry is only
//!   ever a live probe result, which is what makes "it comes back when the
//!   branch is checked out again" work with no repair step.
//!
//! The policy layer above — which space is *open*, the switcher's commands, the
//! dialogs, the editor-handoff teardown — is `spaces` in the `copper` crate and
//! needs an `AppHandle` for all of it.

pub mod availability;
pub mod dispatch;
pub mod launch;
pub mod paths;

use std::sync::{Mutex, MutexGuard};

/// Locking for every mutex in this layer, poison-tolerant.
///
/// A panicking worker must not turn every later lock into a second failure: what
/// these mutexes hold is a queue and a cache, both still coherent after a panic
/// in whatever was holding them. Same rule as `store::lock`, and the reason it is
/// stated once rather than in each submodule.
///
/// `pub` rather than private to the module tree, because the app's own spaces
/// layer takes it for the activation guard that serialises a space switch — one
/// rule, one implementation, across the crate boundary.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
	mutex.lock().unwrap_or_else(|err| err.into_inner())
}
