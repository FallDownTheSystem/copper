//! Copy-only OLE file drags on the event-loop thread's existing STA apartment.

use std::cell::Cell;
use std::path::PathBuf;

use windows::core::{implement, BOOL, HRESULT};
use windows::Win32::Foundation::{
	DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK,
};
use windows::Win32::System::Ole::{
	DoDragDrop, IDropSource, IDropSource_Impl, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Input::KeyboardAndMouse::{
	GetAsyncKeyState, ReleaseCapture, VK_ESCAPE, VK_LBUTTON,
};

use copper_core::store::error::{Result, StoreError};

mod data;

thread_local! {
	static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

pub fn active() -> bool {
	ACTIVE.get()
}

struct DragSession;

impl Drop for DragSession {
	fn drop(&mut self) {
		ACTIVE.set(false);
	}
}

pub fn file_payload(paths: &[PathBuf]) -> Result<Vec<u8>> {
	super::clipboard::payload::files(paths)
		.map_err(|err| StoreError::Invalid(format!("the files could not be dragged: {err}")))
}

/// Called only through `run_on_main_thread`; no store or export locks span OLE's modal loop.
pub fn start(payload: Vec<u8>) -> Result<bool> {
	if active() {
		return Err(StoreError::Invalid(
			"an attachment drag is already active".into(),
		));
	}
	// Preparation happens off-thread. A released press must not become a later drop.
	if unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } >= 0
		|| unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) } < 0
	{
		return Ok(false);
	}
	let data = data::files(payload);
	let source: IDropSource = FileDropSource.into();
	ACTIVE.set(true);
	let _session = DragSession;
	let mut effect = DROPEFFECT_NONE;
	// Chromium can retain mouse capture after mousedown. OLE must own it instead.
	let _ = unsafe { ReleaseCapture() };
	let result = unsafe { DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect) };
	result
		.ok()
		.map_err(|err| StoreError::Io(format!("the attachment drag failed: {err}")))?;
	Ok(result == DRAGDROP_S_DROP && effect == DROPEFFECT_COPY)
}

#[implement(IDropSource)]
struct FileDropSource;

fn continue_drag(escape: bool, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
	if escape || keys.contains(MK_RBUTTON) {
		DRAGDROP_S_CANCEL
	} else if !keys.contains(MK_LBUTTON) {
		DRAGDROP_S_DROP
	} else {
		S_OK
	}
}

#[allow(non_snake_case)]
impl IDropSource_Impl for FileDropSource_Impl {
	fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
		continue_drag(escape.as_bool(), keys)
	}

	fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
		DRAGDROP_S_USEDEFAULTCURSORS
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn mouse_release_drops_but_escape_and_right_click_cancel() {
		assert_eq!(continue_drag(false, MK_LBUTTON), S_OK);
		assert_eq!(continue_drag(false, MODIFIERKEYS_FLAGS(0)), DRAGDROP_S_DROP);
		assert_eq!(continue_drag(true, MK_LBUTTON), DRAGDROP_S_CANCEL);
		assert_eq!(
			continue_drag(true, MODIFIERKEYS_FLAGS(0)),
			DRAGDROP_S_CANCEL
		);
		assert_eq!(
			continue_drag(false, MK_LBUTTON | MK_RBUTTON),
			DRAGDROP_S_CANCEL
		);
	}

	#[test]
	fn session_guard_restores_inbound_drop_availability() {
		assert!(!active());
		ACTIVE.set(true);
		let session = DragSession;
		assert!(active());
		drop(session);
		assert!(!active());
	}
}
