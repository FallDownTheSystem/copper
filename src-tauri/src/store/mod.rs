//! What is left of the store on the app's side of the crate boundary: the Tauri
//! command wrappers, and the sink that emits over IPC.
//!
//! The store itself — the model, the format, the operations, undo, conflict
//! handling, the watch, settings and bootstrap — is [`copper_core::store`].
//! These two modules keep their original paths so that
//! `crate::store::commands::…` and `crate::store::events::AppSink` resolve
//! unchanged; everything else that used to be reachable through `crate::store::`
//! is reached through `copper_core::store::` instead.

pub mod commands;
pub mod events;
