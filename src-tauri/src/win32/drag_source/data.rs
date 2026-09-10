//! Each GetData call transfers a fresh allocation to the receiver.

use std::ptr;

use windows::core::{implement, BOOL, HRESULT};
use windows::Win32::Foundation::{
	GlobalFree, DATA_S_SAMEFORMATETC, DV_E_FORMATETC, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER,
	OLE_E_ADVISENOTSUPPORTED, S_OK,
};
use windows::Win32::System::Com::{
	IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, DATADIR_GET,
	DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::UI::Shell::SHCreateStdEnumFmtEtc;

pub(super) fn files(payload: Vec<u8>) -> IDataObject {
	FileData { payload }.into()
}

fn format() -> FORMATETC {
	FORMATETC {
		cfFormat: CF_HDROP.0,
		ptd: ptr::null_mut(),
		dwAspect: DVASPECT_CONTENT.0,
		lindex: -1,
		tymed: TYMED_HGLOBAL.0 as u32,
	}
}

fn query(request: *const FORMATETC) -> HRESULT {
	// COM supplies either a live FORMATETC or a null pointer, which we refuse.
	let Some(request) = (unsafe { request.as_ref() }) else {
		return E_POINTER;
	};
	if request.cfFormat == CF_HDROP.0
		&& request.dwAspect == DVASPECT_CONTENT.0
		&& request.lindex == -1
		&& request.tymed & TYMED_HGLOBAL.0 as u32 != 0
		&& request.ptd.is_null()
	{
		S_OK
	} else {
		DV_E_FORMATETC
	}
}

#[implement(IDataObject)]
struct FileData {
	payload: Vec<u8>,
}

#[allow(non_snake_case)]
impl IDataObject_Impl for FileData_Impl {
	fn GetData(&self, request: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
		query(request).ok()?;
		// The allocation belongs to this call until STGMEDIUM transfers it. A target
		// may read, release, and request the same format repeatedly during one drag.
		unsafe {
			let memory = GlobalAlloc(GMEM_MOVEABLE, self.payload.len())?;
			let bytes = GlobalLock(memory);
			if bytes.is_null() {
				let _ = GlobalFree(Some(memory));
				return Err(E_OUTOFMEMORY.into());
			}
			ptr::copy_nonoverlapping(self.payload.as_ptr(), bytes.cast(), self.payload.len());
			let _ = GlobalUnlock(memory);
			Ok(STGMEDIUM {
				tymed: TYMED_HGLOBAL.0 as u32,
				u: STGMEDIUM_0 { hGlobal: memory },
				pUnkForRelease: Default::default(),
			})
		}
	}

	fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
		Err(E_NOTIMPL.into())
	}

	fn QueryGetData(&self, request: *const FORMATETC) -> HRESULT {
		query(request)
	}

	fn GetCanonicalFormatEtc(&self, input: *const FORMATETC, output: *mut FORMATETC) -> HRESULT {
		if input.is_null() || output.is_null() {
			return E_POINTER;
		}
		unsafe {
			*output = *input;
			(*output).ptd = ptr::null_mut();
		}
		DATA_S_SAMEFORMATETC
	}

	fn SetData(
		&self,
		_: *const FORMATETC,
		_: *const STGMEDIUM,
		_: BOOL,
	) -> windows::core::Result<()> {
		Err(E_NOTIMPL.into())
	}

	fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
		if direction != DATADIR_GET.0 as u32 {
			return Err(E_NOTIMPL.into());
		}
		unsafe { SHCreateStdEnumFmtEtc(&[format()]) }
	}

	fn DAdvise(
		&self,
		_: *const FORMATETC,
		_: u32,
		_: windows::core::Ref<'_, IAdviseSink>,
	) -> windows::core::Result<u32> {
		Err(OLE_E_ADVISENOTSUPPORTED.into())
	}

	fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
		Err(OLE_E_ADVISENOTSUPPORTED.into())
	}

	fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
		Err(OLE_E_ADVISENOTSUPPORTED.into())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use windows::Win32::System::Ole::{ReleaseStgMedium, CF_UNICODETEXT};
	use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

	#[test]
	fn only_file_lists_are_advertised_not_text_or_pixels() {
		let mut request = format();
		assert_eq!(query(&request), S_OK);
		request.cfFormat = CF_UNICODETEXT.0;
		assert_eq!(query(&request), DV_E_FORMATETC);
		request = format();
		request.lindex = 0;
		assert_eq!(query(&request), DV_E_FORMATETC);
		request = format();
		request.tymed = 0;
		assert_eq!(query(&request), DV_E_FORMATETC);
		assert_eq!(query(ptr::null()), E_POINTER);
	}

	#[test]
	fn repeated_native_reads_deliver_all_files_with_independent_ownership() {
		let paths = vec!["C:\\exports\\one.png".into(), "C:\\exports\\two.pdf".into()];
		let object = files(super::super::file_payload(&paths).unwrap());
		unsafe {
			let mut first = object.GetData(&format()).unwrap();
			let mut second = object.GetData(&format()).unwrap();
			assert_ne!(first.u.hGlobal, second.u.hGlobal);
			ReleaseStgMedium(&mut first);
			let hdrop = HDROP(second.u.hGlobal.0);
			assert_eq!(DragQueryFileW(hdrop, u32::MAX, None), 2);
			let mut buffer = [0u16; 256];
			let length = DragQueryFileW(hdrop, 1, Some(&mut buffer));
			assert_eq!(
				String::from_utf16(&buffer[..length as usize]).unwrap(),
				"C:\\exports\\two.pdf"
			);
			ReleaseStgMedium(&mut second);
		}
	}
}
