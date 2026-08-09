//! The app's half of the blob layer.
//!
//! The rules — what a valid `file` name is, where the assets directory sits, how
//! a blob is written and read, how long an orphan is left alone — are all in
//! [`copper_core::attachments`], and nothing here restates them. What is here is
//! the two operations that cannot be: `ingest`, which reads an image's
//! dimensions through the `image` crate, and `sweep`, which logs its failures
//! through this crate's `diagnostics`. Both are re-exported at this module's own
//! path, so `attachments::ingest(…)` and `attachments::sweep(…)` mean what they
//! always meant.

pub mod commands;
pub mod ingest;
pub mod sweep;
pub mod thumb;

// A module and a function may share a name — they live in different namespaces —
// and that is deliberate here: the two operations moved into files of their own
// without any caller having to learn a second path component.
pub use ingest::ingest;
pub use sweep::sweep;
