//! Everything Copper does that is not a window.
//!
//! Plain Rust with no Tauri anywhere in it: the model, the on-disk format, the
//! operations, undo, conflict handling, the directory watch, settings, space
//! identity and availability, and the attachment path layer. The app's shell —
//! commands, the `AppHandle` event sink, dialogs, thumbnails, capture — lives in
//! the `copper` crate beside this one and depends on it.
//!
//! The split is not new architecture. `store/mod.rs` has described itself as
//! "plain Rust with a thin Tauri shell" since task-003, and
//! `src-tauri/tests/store_fs.rs` has been building a fully working store with no
//! Tauri runtime for just as long. This crate is that boundary made structural,
//! so a second front end — the CLI of task-021 — links the logic without linking
//! a window toolkit, and so the two can never drift into two implementations of
//! one format.
//!
//! The module hierarchy is preserved as it was inside the app crate:
//! `store::`, `spaces::`, `attachments::` and `entry::` mean here what they meant
//! there, so every `use crate::…` inside a moved file still resolves. Three
//! modules kept a namesake on the other side of the boundary rather than moving
//! whole — `store`, `store::events` and `attachments` — because each mixed pure
//! code with something that needs a window, an image decoder, or the app's log.

pub mod attachments;
pub mod entry;
pub mod spaces;
pub mod store;
