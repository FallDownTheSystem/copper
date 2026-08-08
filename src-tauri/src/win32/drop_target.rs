//! The panel's own OLE drop targets, standing in for wry's until wry#1639 ships.
//!
//! **Why this file exists at all.** `dragDropEnabled: true` means wry disables
//! WebView2's external drop handling and registers its own `IDropTarget` on
//! every WebView2 child HWND — but it enumerates those children exactly once,
//! at webview creation, and a window created `visible: false` does not have
//! them yet. The panel is born hidden by design, so wry registers on nothing,
//! Chromium's own drop target (with external drops disabled) answers every drag
//! with `DROPEFFECT_NONE`, and a file dragged over the panel shows the refusal
//! cursor and delivers nowhere. The upstream fix (re-register after show,
//! tauri-apps/wry#1638) is unreleased, so [`reinstall`] does the same from app
//! code: revoke whatever target each descendant HWND carries and register ours.
//!
//! **The implementation mirrors wry's `webview2/drag_drop.rs` deliberately**,
//! down to the effect and validity bookkeeping — this is a stand-in that must
//! behave byte-for-byte like the thing it stands in for, because the day a
//! fixed wry re-registers its own targets over these, the frontend must not be
//! able to tell the difference. The one divergence is delivery: wry hands
//! events to tauri's runtime proxy, which this module cannot reach, so the
//! listener is a closure the caller builds (in practice `panel.rs`, emitting
//! the same `tauri://drag-*` events tauri itself would).
//!
//! **Main thread only, enforced by construction.** `RegisterDragDrop` requires
//! the calling thread's OLE apartment, which tao initialises for the event-loop
//! thread — and the registry below is a `thread_local`, so a call from any
//! other thread could only ever see an empty list and leak registrations. Every
//! caller is a reveal path, and reveals are main-thread by the same rule every
//! window operation follows.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;
use std::rc::Rc;

use windows::core::{implement, BOOL};
use windows::Win32::Foundation::{DRAGDROP_E_INVALIDHWND, HWND, LPARAM, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
	IDropTarget, IDropTarget_Impl, RegisterDragDrop, RevokeDragDrop, CF_HDROP, DROPEFFECT,
	DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::EnumChildWindows;

/// What a drag did, in the vocabulary the frontend already speaks.
///
/// Positions are physical client-area coordinates of the HWND the drag is over,
/// exactly as wry reports them. Nothing in the panel reads them today — the
/// drop treatment is panel-wide — but the payload shape is part of the
/// stand-in contract.
pub enum DropEvent {
	Enter { paths: Vec<PathBuf>, position: (i32, i32) },
	Over { position: (i32, i32) },
	Drop { paths: Vec<PathBuf>, position: (i32, i32) },
	Leave,
}

thread_local! {
	/// The registrations the last [`reinstall`] made, so the next one can revoke
	/// them before re-enumerating. `(HWND as isize, target)` pairs rather than
	/// bare targets, because revocation addresses the window, not the interface.
	static REGISTERED: RefCell<Vec<(isize, IDropTarget)>> = const { RefCell::new(Vec::new()) };
}

/// Registers a drop target forwarding to `listener` on every descendant of
/// `window`, replacing whatever was there — Chromium's refusing target, wry's
/// dead one, or ours from the previous call.
///
/// Idempotent and cheap (a handful of Win32 calls over a seven-window tree), so
/// it runs on **every** reveal rather than once: WebView2 is documented to be
/// able to recreate its child HWNDs, and the first reveal is exactly the moment
/// the children this must land on come into existence.
pub fn reinstall(window: HWND, listener: Rc<dyn Fn(DropEvent)>) {
	REGISTERED.with(|cell| {
		let mut registered = cell.borrow_mut();
		for (hwnd, _target) in registered.drain(..) {
			// A window that died since last time answers DRAGDROP_E_INVALIDHWND,
			// which is exactly the outcome revocation wanted.
			let _ = unsafe { RevokeDragDrop(HWND(hwnd as _)) };
		}

		for child in descendants(window) {
			let target: IDropTarget = PanelDropTarget::new(child, listener.clone()).into();
			// wry's own condition, kept verbatim: an invalid HWND is the one revoke
			// failure that means registering would be pointless.
			if unsafe { RevokeDragDrop(child) } != Err(DRAGDROP_E_INVALIDHWND.into())
				&& unsafe { RegisterDragDrop(child, &target) }.is_ok()
			{
				registered.push((child.0 as isize, target));
			}
		}
	});
}

/// Every descendant HWND, collected before any registration happens — unlike
/// wry's in-callback registration, so the enumeration cannot observe windows a
/// registration side effect might create.
fn descendants(window: HWND) -> Vec<HWND> {
	unsafe extern "system" fn push(hwnd: HWND, lparam: LPARAM) -> BOOL {
		// SAFETY: `lparam` is the pointer to the Vec below, alive for the whole
		// synchronous EnumChildWindows call.
		let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
		list.push(hwnd);
		true.into()
	}

	let mut list: Vec<HWND> = Vec::new();
	// SAFETY: the callback only touches the Vec passed through lparam, and
	// EnumChildWindows returns before this frame does.
	let _ = unsafe {
		EnumChildWindows(
			Some(window),
			Some(push),
			LPARAM(&mut list as *mut Vec<HWND> as isize),
		)
	};
	list
}

/// One registered target. State is `Cell` rather than wry's `UnsafeCell` — the
/// COM calls arrive single-threaded on the STA thread, so interior mutability
/// is all that is needed and `Cell` provides it without an unsafe block.
#[implement(IDropTarget)]
struct PanelDropTarget {
	hwnd: HWND,
	listener: Rc<dyn Fn(DropEvent)>,
	/// The effect DragEnter chose, replayed by every DragOver: the decision is
	/// made once per drag, not once per mouse move.
	cursor_effect: Cell<DROPEFFECT>,
	/// Whether the current hover carries files at all. A drag of plain text must
	/// produce no events and no Leave — wry's `enter_is_valid`, same name.
	enter_is_valid: Cell<bool>,
}

impl PanelDropTarget {
	fn new(hwnd: HWND, listener: Rc<dyn Fn(DropEvent)>) -> Self {
		Self {
			hwnd,
			listener,
			cursor_effect: Cell::new(DROPEFFECT_NONE),
			enter_is_valid: Cell::new(false),
		}
	}

	fn client_point(&self, pt: &POINTL) -> (i32, i32) {
		let mut point = POINT { x: pt.x, y: pt.y };
		// SAFETY: `hwnd` is one of the windows this target was registered on, and
		// the point is a plain out-parameter.
		let _ = unsafe { ScreenToClient(self.hwnd, &mut point) };
		(point.x, point.y)
	}

	/// The paths in a CF_HDROP payload, or `None` when the drag is not files.
	/// The returned HDROP is live only while the caller still holds the data
	/// object; `Drop` passes it to `DragFinish`, the hover paths do not.
	fn paths(data: windows::core::Ref<'_, IDataObject>) -> Option<(Vec<PathBuf>, HDROP)> {
		let format = FORMATETC {
			cfFormat: CF_HDROP.0,
			ptd: ptr::null_mut(),
			dwAspect: DVASPECT_CONTENT.0,
			lindex: -1,
			tymed: TYMED_HGLOBAL.0 as u32,
		};

		// SAFETY: the medium's hGlobal is an HDROP for a successful CF_HDROP
		// GetData, per the shell's clipboard-format contract; DragQueryFileW is
		// given buffers sized by its own count-then-fill protocol.
		unsafe {
			let medium = data.as_ref()?.GetData(&format).ok()?;
			let hdrop = HDROP(medium.u.hGlobal.0 as _);

			// 0xFFFFFFFF asks for the item count rather than an item.
			let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
			let mut paths = Vec::with_capacity(count as usize);
			for i in 0..count {
				// Sized per path rather than MAX_PATH: long-path-aware sources hand
				// out longer names, and truncating one silently corrupts the drop.
				let characters = DragQueryFileW(hdrop, i, None) as usize;
				let mut buffer = vec![0u16; characters + 1];
				DragQueryFileW(hdrop, i, Some(&mut buffer));
				paths.push(OsString::from_wide(&buffer[0..characters]).into());
			}
			Some((paths, hdrop))
		}
	}
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for PanelDropTarget_Impl {
	fn DragEnter(
		&self,
		pDataObj: windows::core::Ref<'_, IDataObject>,
		_grfKeyState: MODIFIERKEYS_FLAGS,
		pt: &POINTL,
		pdwEffect: *mut DROPEFFECT,
	) -> windows::core::Result<()> {
		let Some((paths, _hdrop)) = PanelDropTarget::paths(pDataObj) else {
			// Not files: no events, and — mirroring wry — the effect is left as the
			// caller initialised it, with `enter_is_valid` false silencing the rest
			// of the drag.
			return Ok(());
		};

		self.enter_is_valid.set(true);
		(self.listener)(DropEvent::Enter {
			paths,
			position: self.client_point(pt),
		});

		self.cursor_effect.set(DROPEFFECT_COPY);
		// SAFETY: `pdwEffect` is the out-parameter of a live COM call.
		unsafe { *pdwEffect = DROPEFFECT_COPY };
		Ok(())
	}

	fn DragOver(
		&self,
		_grfKeyState: MODIFIERKEYS_FLAGS,
		pt: &POINTL,
		pdwEffect: *mut DROPEFFECT,
	) -> windows::core::Result<()> {
		if self.enter_is_valid.get() {
			(self.listener)(DropEvent::Over {
				position: self.client_point(pt),
			});
		}
		// SAFETY: `pdwEffect` is the out-parameter of a live COM call.
		unsafe { *pdwEffect = self.cursor_effect.get() };
		Ok(())
	}

	fn DragLeave(&self) -> windows::core::Result<()> {
		if self.enter_is_valid.get() {
			(self.listener)(DropEvent::Leave);
		}
		Ok(())
	}

	fn Drop(
		&self,
		pDataObj: windows::core::Ref<'_, IDataObject>,
		_grfKeyState: MODIFIERKEYS_FLAGS,
		pt: &POINTL,
		_pdwEffect: *mut DROPEFFECT,
	) -> windows::core::Result<()> {
		if self.enter_is_valid.get() {
			if let Some((paths, hdrop)) = PanelDropTarget::paths(pDataObj) {
				(self.listener)(DropEvent::Drop {
					paths,
					position: self.client_point(pt),
				});
				// SAFETY: `hdrop` came out of this drop's own data object.
				unsafe { DragFinish(hdrop) };
			}
		}
		Ok(())
	}
}
