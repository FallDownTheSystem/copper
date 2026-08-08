//! What the store tells the frontend, and the seam that makes it testable.
//!
//! Payloads carry **identity only, never document contents** (spec 8.3). Tauri
//! event payloads are JSON evaluated in the webview and are not a bulk data
//! channel; more importantly, an identity-only payload means a dropped event can
//! never cost data, because the recovery is another `get_active_space` pull.
//!
//! Emission goes through `EventSink` rather than straight to `AppHandle` for two
//! reasons. `cargo test` can then count emissions exactly (A9.37) — reviewing
//! thin command wrappers by eye cannot, and "emits exactly one
//! `settings-changed`" is the kind of claim that rots silently. And the sink is
//! reachable only *after* the store guard is dropped, which keeps spec 2.10's
//! never-emit-under-the-lock rule structural: Tauri dispatches to Rust-side
//! listeners synchronously on the emitting thread, so the first internal
//! listener that touches store state would deadlock against a non-reentrant
//! `std::sync::Mutex`.

use std::sync::{Mutex, MutexGuard};

use serde::Serialize;

use super::error::StoreError;

pub const SPACE_CHANGED: &str = "space-changed";
pub const SETTINGS_CHANGED: &str = "settings-changed";
pub const STORE_ERROR: &str = "store-error";

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SpaceChanged {
	pub id: String,
	pub path: String,
	pub reason: ChangeReason,
}

/// Each variant has exactly one producer, so none of them is dead (spec 8.3).
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChangeReason {
	/// A watcher reload that found a genuinely different document (spec 3.5).
	External,
	/// `append_capture` (spec 8.5).
	Capture,
	/// Recovery from the errored state (spec 3.6a) — the panel is displaying
	/// "this space is unreadable" and has no other way to learn that it is not.
	Reload,
	/// An `$EDITOR` read-back (Phase 5). Like a capture, this is a Rust-side
	/// writer the frontend did not invoke, so its `Space` return value reaches
	/// nobody and the panel would otherwise keep rendering the pre-save body.
	Editor,
	/// A capture notification's re-route button (task-018) — the fourth Rust-side
	/// writer, and the reason it is not folded into [`Self::Capture`]: the panel
	/// treats `capture` as "a note just landed" and answers it with a sound and a
	/// scroll request, neither of which belongs to a note the user is merely
	/// filing from a toast.
	Reroute,
	/// `append_paths_note` — the note that stands in for files too large to
	/// attach (2026-08-09). The fifth Rust-side writer, and not [`Self::Capture`]
	/// for the same reason `Reroute` is not: the user is at the panel with their
	/// hands on the drop or the paste, so the capture sound and its scroll
	/// request would be answering an absence nobody experienced.
	Attach,
}

/// Braces, not a unit struct: `struct SettingsChanged;` and Rust's `()` both
/// serialise to `null`, and the contract in spec 8.3 promises `{}`.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SettingsChanged {}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct StoreErrorEvent {
	pub kind: String,
	pub message: String,
}

impl From<&StoreError> for StoreErrorEvent {
	fn from(err: &StoreError) -> Self {
		Self {
			kind: err.kind().to_string(),
			message: err.message(),
		}
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreEvent {
	SpaceChanged(SpaceChanged),
	SettingsChanged(SettingsChanged),
	StoreError(StoreErrorEvent),
}

impl StoreEvent {
	pub fn name(&self) -> &'static str {
		match self {
			Self::SpaceChanged(_) => SPACE_CHANGED,
			Self::SettingsChanged(_) => SETTINGS_CHANGED,
			Self::StoreError(_) => STORE_ERROR,
		}
	}

	pub fn settings_changed() -> Self {
		Self::SettingsChanged(SettingsChanged {})
	}

	pub fn error(err: &StoreError) -> Self {
		Self::StoreError(StoreErrorEvent::from(err))
	}

	/// The payload as it crosses the IPC boundary. Used by the `AppHandle` sink
	/// and by tests asserting the documented JSON shapes.
	pub fn payload(&self) -> serde_json::Value {
		match self {
			Self::SpaceChanged(payload) => serde_json::to_value(payload),
			Self::SettingsChanged(payload) => serde_json::to_value(payload),
			Self::StoreError(payload) => serde_json::to_value(payload),
		}
		.unwrap_or(serde_json::Value::Null)
	}
}

/// Where an event goes once the store guard has been released.
pub trait EventSink: Send + Sync {
	fn emit(&self, event: &StoreEvent);
}

/// Keeps every event for a test to inspect.
///
/// Public rather than `#[cfg(test)]`: the filesystem and watcher tests live in
/// `tests/store_fs.rs`, which links the library as an external crate.
#[derive(Default)]
pub struct RecordingSink {
	events: Mutex<Vec<StoreEvent>>,
}

impl RecordingSink {
	pub fn new() -> Self {
		Self::default()
	}

	/// Tolerates a poisoned mutex for the same reason `store::lock` does: one
	/// panicking test must not turn every later assertion into a second failure.
	fn recorded(&self) -> MutexGuard<'_, Vec<StoreEvent>> {
		self.events.lock().unwrap_or_else(|err| err.into_inner())
	}

	pub fn events(&self) -> Vec<StoreEvent> {
		self.recorded().clone()
	}

	pub fn take(&self) -> Vec<StoreEvent> {
		std::mem::take(&mut *self.recorded())
	}

	/// The names of the events recorded so far, in order — the form most emit
	/// assertions actually want. Read under the guard: cloning the events only
	/// to borrow their static names would copy every payload string.
	pub fn names(&self) -> Vec<&'static str> {
		self.recorded().iter().map(StoreEvent::name).collect()
	}

	/// `take` and `names` in one call. An emit assertion almost always wants both:
	/// the names one step produced, and a cleared recording for the next.
	pub fn take_names(&self) -> Vec<&'static str> {
		self.take().iter().map(StoreEvent::name).collect()
	}
}

impl EventSink for RecordingSink {
	fn emit(&self, event: &StoreEvent) {
		self.recorded().push(event.clone());
	}
}

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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn settings_changed_is_an_empty_object_not_null() {
		assert_eq!(StoreEvent::settings_changed().payload().to_string(), "{}");
	}

	#[test]
	fn space_changed_payload_is_camel_case_identity_only() {
		let event = StoreEvent::SpaceChanged(SpaceChanged {
			id: "spc_00000001".into(),
			path: "C:\\notes.copper".into(),
			reason: ChangeReason::External,
		});
		let payload = event.payload();

		assert_eq!(payload["id"], "spc_00000001");
		assert_eq!(payload["reason"], "external");
		assert_eq!(payload.as_object().unwrap().len(), 3, "payload carries extra data");
	}

	#[test]
	fn every_reason_serialises_to_its_documented_string() {
		for (reason, expected) in [
			(ChangeReason::External, "external"),
			(ChangeReason::Capture, "capture"),
			(ChangeReason::Reload, "reload"),
			(ChangeReason::Editor, "editor"),
			(ChangeReason::Reroute, "reroute"),
			(ChangeReason::Attach, "attach"),
		] {
			assert_eq!(serde_json::to_value(reason).unwrap(), expected);
		}
	}

	#[test]
	fn store_error_payload_matches_the_error_shape() {
		let event = StoreEvent::error(&StoreError::Parse("bad json".into()));
		assert_eq!(event.payload()["kind"], "parse");
		assert_eq!(event.payload()["message"], "bad json");
		assert_eq!(event.name(), STORE_ERROR);
	}

	#[test]
	fn the_recording_sink_keeps_order() {
		let sink = RecordingSink::new();
		sink.emit(&StoreEvent::settings_changed());
		sink.emit(&StoreEvent::error(&StoreError::Io("x".into())));

		assert_eq!(sink.names(), [SETTINGS_CHANGED, STORE_ERROR]);
		assert_eq!(sink.take().len(), 2);
		assert!(sink.events().is_empty());
	}
}
