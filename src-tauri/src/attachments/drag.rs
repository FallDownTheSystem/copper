//! A drag ticket owns a file list Rust resolved, never paths supplied by the
//! webview. The list names the stored files themselves, and the drag source
//! permits copy only, so a receiver can never move an attachment out of its
//! space.

use std::sync::Mutex;

use tauri::{AppHandle, Manager, State};

use copper_core::store::error::{Result, StoreError};
use copper_core::store::SharedStore;

use super::copy::{prepare_files, AttachmentTarget};
use crate::store::source::DocumentSource;
use crate::win32::drag_source;

struct PreparedDrag {
	id: String,
	source: DocumentSource,
	payload: Vec<u8>,
}

/// Only the most recent preparation can start; tickets are single-use and bounded.
#[derive(Default)]
pub struct DragExports(Mutex<Option<PreparedDrag>>);

impl DragExports {
	fn put(&self, source: DocumentSource, payload: Vec<u8>) -> String {
		let id = uuid::Uuid::new_v4().to_string();
		*self.0.lock().unwrap_or_else(|err| err.into_inner()) = Some(PreparedDrag {
			id: id.clone(),
			source,
			payload,
		});
		id
	}

	fn start(&self, id: &str, run: impl FnOnce(PreparedDrag) -> Result<bool>) -> Result<bool> {
		match self.take(id) {
			Some(prepared) => run(prepared),
			None => Ok(false),
		}
	}

	fn take(&self, id: &str) -> Option<PreparedDrag> {
		let mut pending = self.0.lock().unwrap_or_else(|err| err.into_inner());
		if pending.as_ref().is_some_and(|drag| drag.id == id) {
			pending.take()
		} else {
			None
		}
	}
}

#[tauri::command]
pub async fn attachment_prepare_drag(
	targets: Vec<AttachmentTarget>,
	source: DocumentSource,
	app: AppHandle,
	state: State<'_, SharedStore>,
) -> Result<String> {
	let (path, space) = source.snapshot(&state)?;
	let payload = tauri::async_runtime::spawn_blocking(move || {
		let paths = prepare_files(&path, &space, &targets)?;
		drag_source::file_payload(&paths)
	})
	.await
	.map_err(|err| StoreError::Io(format!("the drag files could not be prepared: {err}")))??;
	Ok(app.state::<DragExports>().put(source, payload))
}

#[tauri::command]
pub fn attachment_discard_drag(id: String, exports: State<'_, DragExports>) {
	exports.take(&id);
}

#[tauri::command]
pub async fn attachment_start_drag(
	id: String,
	app: AppHandle,
	state: State<'_, SharedStore>,
) -> Result<bool> {
	let state = state.inner().clone();
	let owner = app.clone();
	let (send, receive) = std::sync::mpsc::sync_channel(1);
	app.run_on_main_thread(move || {
		// A queued start has no ownership yet: cancellation can still discard its ticket.
		let result = owner.state::<DragExports>().start(&id, |prepared| {
			prepared
				.source
				.snapshot(&state)
				.and_then(|_| drag_source::start(prepared.payload))
		});
		let _ = send.send(result);
	})
	.map_err(|err| StoreError::Io(format!("the attachment drag could not start: {err}")))?;
	tauri::async_runtime::spawn_blocking(move || receive.recv())
		.await
		.map_err(|err| StoreError::Io(format!("the attachment drag stopped: {err}")))?
		.map_err(|err| StoreError::Io(format!("the attachment drag stopped: {err}")))?
}

#[cfg(test)]
mod tests {
	use super::*;

	fn source() -> DocumentSource {
		DocumentSource {
			path: "C:\\test.copper".into(),
			id: "document".into(),
		}
	}

	#[test]
	fn tickets_are_single_use_and_do_not_expose_arbitrary_paths() {
		let exports = DragExports::default();
		let id = exports.put(source(), vec![1, 2, 3]);
		assert!(exports.take("C:\\unrelated.txt").is_none());
		assert_eq!(exports.take(&id).unwrap().payload, [1, 2, 3]);
		assert!(exports.take(&id).is_none());
	}

	#[test]
	fn cancellation_before_main_thread_execution_prevents_native_start() {
		let exports = DragExports::default();
		let id = exports.put(source(), vec![1]);
		let queued = || exports.start(&id, |_| panic!("cancelled drag reached OLE"));
		exports.take(&id);
		assert!(!queued().unwrap());
	}

	#[test]
	fn native_start_consumes_the_ticket_without_holding_the_registry_lock() {
		let exports = DragExports::default();
		let id = exports.put(source(), vec![1]);
		assert!(exports
			.start(&id, |prepared| {
				assert_eq!(prepared.payload, [1]);
				assert!(exports.take(&id).is_none());
				Ok(true)
			})
			.unwrap());
		assert!(!exports.start(&id, |_| panic!("ticket replayed")).unwrap());
	}

	#[test]
	fn discarding_an_old_ticket_does_not_consume_the_new_one() {
		let exports = DragExports::default();
		let old = exports.put(source(), vec![1]);
		let new = exports.put(source(), vec![2]);
		assert!(exports.take(&old).is_none());
		assert_eq!(exports.take(&new).unwrap().payload, [2]);
	}
}
