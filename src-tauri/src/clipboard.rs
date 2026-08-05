//! The one command that puts Copper's own text on the clipboard.
//!
//! Deliberately thin, and deliberately **not** inside `win32/`: task-005's
//! module-boundary assertions confine `HWND` and raw Win32 calls to `win32/` and
//! `capture/`, and a `#[tauri::command]` is neither. The write path itself,
//! including the three clipboard-privacy formats, is
//! [`win32::clipboard::write_text_private`] and is not reimplemented here.
//!
//! The visible consequence is intended, not a bug: text copied through this
//! never appears in `Win+V` history, because every write Copper makes carries
//! `ExcludeClipboardContentFromMonitorProcessing` and
//! `CanIncludeInClipboardHistory`.

use crate::win32::clipboard;

/// Errors cross as a plain string rather than the store's `{ kind, message }`:
/// there is nothing here for a caller to branch on — a failed copy is a failed
/// copy — and inventing a second error taxonomy for one command would be worse
/// than the asymmetry.
#[tauri::command]
pub async fn clipboard_write_text(text: String) -> Result<(), String> {
	clipboard::write_text_private(&text).map_err(|err| err.to_string())
}
