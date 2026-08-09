//! The one Tauri-coupled sink.
//!
//! Everything else that was in this file — `StoreEvent` and its payloads, the
//! [`EventSink`] trait itself, `RecordingSink` and their tests — is in
//! [`copper_core::store::events`], because none of it needs a window. What is
//! left is the single implementation that does: the one that emits over IPC.
//!
//! The file keeps its old path deliberately. `crate::store::events::AppSink`
//! resolves in `lib.rs`, `spaces/mod.rs` and `editor.rs` exactly as it did
//! before the extraction, so no call site had to learn that the type moved
//! house while its neighbours left.

use copper_core::store::events::{EventSink, StoreEvent};

/// The real sink.
pub struct AppSink {
	app: tauri::AppHandle,
}

impl AppSink {
	pub fn new(app: tauri::AppHandle) -> Self {
		Self { app }
	}
}

impl EventSink for AppSink {
	fn emit(&self, event: &StoreEvent) {
		use tauri::Emitter;

		// Logged, never propagated. Every emit in this store happens *after* the
		// change it announces is already durable, so failing here would report a
		// completed write as failed — and Phase 4's capture failure path is
		// user-visible and may retry, which would duplicate the note (spec 8.5a).
		// The frontend recovers on its next pull regardless.
		if let Err(err) = self.app.emit(event.name(), event.payload()) {
			crate::diagnostics::log_error(&format!(
				"[copper] could not emit {}: {err}",
				event.name()
			));
		}
	}
}
