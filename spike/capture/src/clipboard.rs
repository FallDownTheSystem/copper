//! Clipboard snapshot, injected `Ctrl+C`, sequence-number polling, restore, and
//! the Windows clipboard-history / cloud-sync exclusion formats.
//!
//! Two things in here are load-bearing and easy to get catastrophically wrong:
//!
//! 1. **Writing requires an owner window.** `SetClipboardData` documents that
//!    "if an application calls OpenClipboard with hwnd set to NULL,
//!    EmptyClipboard sets the clipboard owner to NULL; this causes
//!    SetClipboardData to fail." A write path opened with `OpenClipboard(NULL)`
//!    therefore empties the user's clipboard and then cannot repopulate it.
//!    Hence [`OwnerWindow`], and hence `open_write` refusing to exist without one.
//! 2. **Every successful `OpenClipboard` must be paired with `CloseClipboard`**,
//!    or every process on the system fails to open the clipboard until we do.
//!    Hence [`ClipboardGuard`]; nothing here opens the clipboard any other way.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardOwner, GetClipboardSequenceNumber, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW, RegisterClassW,
    TranslateMessage, HWND_MESSAGE, MSG, PM_REMOVE, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
};

use crate::hook::{VK_CONTROL, VK_MENU, VK_SHIFT};

/// `CF_UNICODETEXT`. Defined here rather than imported because windows-rs puts
/// the `CF_*` constants behind `Win32_System_Ole`, and pulling the whole OLE
/// surface in for one ABI-fixed integer is a poor trade. The value is fixed by
/// `winuser.h` and mirrored in [`builtin_format_name`].
const CF_UNICODETEXT: u32 = 13;

/// Tag written into `KEYBDINPUT.dwExtraInfo` on every event we synthesize, and
/// matched by the hook so our own `Ctrl+C` cannot feed back into the state
/// machine. `dwExtraInfo` is the documented field for exactly this.
pub const COPPER_INJECTED_TAG: usize = 0x0C0F_FEE0;

const VK_C: u32 = 0x43;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ClipboardError {
    /// `OpenClipboard` kept failing for the whole retry budget.
    Busy { attempts: u32, elapsed_ms: u64 },
    /// Somebody else wrote the clipboard between our read and our write, so the
    /// write was abandoned rather than performed. Not a failure of ours — it is
    /// the guard that stops a restore from clobbering a fresh user copy.
    Superseded { expected: u32, actual: u32 },
    Win32 {
        op: &'static str,
        code: i32,
        message: String,
    },
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClipboardError::Busy {
                attempts,
                elapsed_ms,
            } => write!(
                f,
                "clipboard stayed locked by another process ({attempts} attempts over {elapsed_ms} ms)"
            ),
            ClipboardError::Superseded { expected, actual } => write!(
                f,
                "clipboard changed under us before the write (expected sequence {expected}, \
                 found {actual}); write abandoned"
            ),
            ClipboardError::Win32 { op, code, message } => {
                write!(f, "{op} failed: 0x{code:08X} {message}")
            }
        }
    }
}

impl std::error::Error for ClipboardError {}

fn win32(op: &'static str, e: windows::core::Error) -> ClipboardError {
    ClipboardError::Win32 {
        op,
        code: e.code().0,
        message: e.message(),
    }
}

// ---------------------------------------------------------------------------
// Owner window
// ---------------------------------------------------------------------------

unsafe extern "system" fn owner_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// A hidden message-only window that exists solely to be the clipboard owner.
///
/// Not `Send`: it belongs to the thread that created it, which must also pump
/// its queue. A window whose thread never pumps will hang any process that
/// sends it a message.
pub struct OwnerWindow {
    hwnd: HWND,
}

impl OwnerWindow {
    pub fn create() -> Result<Self, ClipboardError> {
        static CLASS: OnceLock<Result<(), String>> = OnceLock::new();

        let hinstance = unsafe { GetModuleHandleW(None) }
            .map_err(|e| win32("GetModuleHandleW", e))?;
        let class_name = w!("CopperSpikeClipboardOwner");

        let registered = CLASS.get_or_init(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(owner_wndproc),
                hInstance: hinstance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            if unsafe { RegisterClassW(&wc) } == 0 {
                Err(windows::core::Error::from_win32().message())
            } else {
                Ok(())
            }
        });
        if let Err(message) = registered {
            return Err(ClipboardError::Win32 {
                op: "RegisterClassW",
                code: 0,
                message: message.clone(),
            });
        }

        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("copper-spike-clipboard-owner"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinstance.into()),
                None,
            )
        }
        .map_err(|e| win32("CreateWindowExW", e))?;

        Ok(Self { hwnd })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Drain the queue. Called around clipboard sessions so this is never an
    /// unpumped window that other processes can block on.
    pub fn pump(&self) {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, Some(self.hwnd), 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

impl Drop for OwnerWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

// ---------------------------------------------------------------------------
// RAII clipboard session
// ---------------------------------------------------------------------------

/// An open clipboard session. Dropping it calls `CloseClipboard`.
pub struct ClipboardGuard {
    _private: (),
}

impl ClipboardGuard {
    /// Read-only session. May pass NULL as the owner because nothing in a read
    /// path calls `EmptyClipboard` or `SetClipboardData`.
    pub fn open_read() -> Result<Self, ClipboardError> {
        Self::open(None)
    }

    /// Write session. Requires a real window owned by this process: a session
    /// opened with NULL that calls `EmptyClipboard` sets the owner to NULL and
    /// makes every subsequent `SetClipboardData` fail.
    pub fn open_write(owner: &OwnerWindow) -> Result<Self, ClipboardError> {
        Self::open(Some(owner.hwnd()))
    }

    fn open(owner: Option<HWND>) -> Result<Self, ClipboardError> {
        let started = Instant::now();
        let mut backoff = Duration::from_millis(10);
        let mut attempts = 0u32;

        loop {
            attempts += 1;
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Ok(Self { _private: () });
            }
            if started.elapsed() >= Duration::from_millis(1000) {
                return Err(ClipboardError::Busy {
                    attempts,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                });
            }
            thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_millis(200));
        }
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

// ---------------------------------------------------------------------------
// Registered formats
// ---------------------------------------------------------------------------

struct Formats {
    html: u32,
    exclude_monitor: u32,
    can_include_history: u32,
    can_upload_cloud: u32,
}

fn formats() -> &'static Formats {
    static FORMATS: OnceLock<Formats> = OnceLock::new();
    FORMATS.get_or_init(|| Formats {
        html: register(w!("HTML Format")),
        // All three are officially documented on Microsoft Learn's "Clipboard
        // Formats" page. They are not folklore, and they are why this spike
        // uses raw Win32 rather than WinRT SetContentWithOptions.
        exclude_monitor: register(w!("ExcludeClipboardContentFromMonitorProcessing")),
        can_include_history: register(w!("CanIncludeInClipboardHistory")),
        can_upload_cloud: register(w!("CanUploadToCloudClipboard")),
    })
}

fn register(name: PCWSTR) -> u32 {
    unsafe { RegisterClipboardFormatW(name) }
}

pub fn html_format_id() -> u32 {
    formats().html
}

pub fn unicode_text_format_id() -> u32 {
    CF_UNICODETEXT
}

/// Resolve a format id to a readable name for the log.
pub fn format_name(id: u32) -> String {
    if let Some(known) = builtin_format_name(id) {
        return known.to_owned();
    }
    let mut buf = [0u16; 128];
    let len = unsafe { GetClipboardFormatNameW(id, &mut buf) };
    if len > 0 {
        String::from_utf16_lossy(&buf[..len as usize])
    } else {
        format!("0x{id:04X}")
    }
}

fn builtin_format_name(id: u32) -> Option<&'static str> {
    Some(match id {
        1 => "CF_TEXT",
        2 => "CF_BITMAP",
        3 => "CF_METAFILEPICT",
        4 => "CF_SYLK",
        5 => "CF_DIF",
        6 => "CF_TIFF",
        7 => "CF_OEMTEXT",
        8 => "CF_DIB",
        9 => "CF_PALETTE",
        13 => "CF_UNICODETEXT",
        14 => "CF_ENHMETAFILE",
        15 => "CF_HDROP",
        16 => "CF_LOCALE",
        17 => "CF_DIBV5",
        0x0080 => "CF_OWNERDISPLAY",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Snapshot / restore
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct FormatInfo {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Raw `CF_UNICODETEXT` payload bytes, exactly as they were on the clipboard.
    pub unicode_text: Option<Vec<u8>>,
    /// Raw `"HTML Format"` payload bytes.
    pub html: Option<Vec<u8>>,
    /// Every format that was present, in enumeration order.
    pub present: Vec<FormatInfo>,
    /// Formats present that this spike does not attempt to restore.
    ///
    /// Note when reading these: `EnumClipboardFormats` also lists system
    /// *synthesized* formats — `CF_TEXT`, `CF_OEMTEXT` and `CF_LOCALE` appear
    /// alongside a `CF_UNICODETEXT` the owner actually set — so this list
    /// contains entries that are not real losses.
    pub unrestorable: Vec<FormatInfo>,
}

/// Read the current clipboard under one session.
///
/// Bitmaps, `CF_HDROP`, `CF_OWNERDISPLAY` and OLE-backed formats are recorded
/// by id only: restoring those faithfully is out of scope, and the point is to
/// know what we would be dropping.
pub fn snapshot() -> Result<Snapshot, ClipboardError> {
    let _guard = ClipboardGuard::open_read()?;
    let fmts = formats();

    // Enumerate every format id FIRST, and only then fetch payloads.
    // `GetClipboardData` can trigger delayed rendering in the owning
    // application (`WM_RENDERFORMAT`), which changes the clipboard *during* the
    // walk — so calling it inside the `EnumClipboardFormats` loop risks a
    // truncated or corrupted enumeration.
    let mut ids: Vec<u32> = Vec::new();
    let mut id = 0u32;
    loop {
        id = unsafe { EnumClipboardFormats(id) };
        if id == 0 {
            break;
        }
        ids.push(id);
        // A misbehaving clipboard owner should not be able to spin us forever.
        if ids.len() >= 256 {
            tracing::warn!("clipboard reported over 256 formats; truncating the enumeration");
            break;
        }
    }

    let mut snap = Snapshot::default();
    for id in ids {
        let info = FormatInfo {
            id,
            name: format_name(id),
        };
        // A format we intend to restore that we cannot READ is a hard failure,
        // not an absent format. Treating a failed read as `None` would let the
        // snapshot look successful, the injection proceed, and the restore
        // silently omit content the user had — destroying it. Fail here instead,
        // before anything is injected, so the cascade records
        // `ClipboardSnapshotFailed` and the clipboard is left untouched.
        if id == unicode_text_format_id() {
            snap.unicode_text = Some(read_format_bytes(id).ok_or_else(|| {
                ClipboardError::Win32 {
                    op: "GetClipboardData(CF_UNICODETEXT)",
                    code: 0,
                    message: "format was advertised but could not be read; \
                              refusing to continue and risk losing it".to_owned(),
                }
            })?);
        } else if id == fmts.html {
            snap.html = Some(read_format_bytes(id).ok_or_else(|| ClipboardError::Win32 {
                op: "GetClipboardData(HTML Format)",
                code: 0,
                message: "format was advertised but could not be read; \
                          refusing to continue and risk losing it"
                    .to_owned(),
            })?);
        } else {
            snap.unrestorable.push(info.clone());
        }
        snap.present.push(info);
    }

    Ok(snap)
}

/// Copy a format's payload out of the clipboard's own memory.
///
/// The returned handle belongs to the clipboard; it is locked and read, never
/// freed.
fn read_format_bytes(id: u32) -> Option<Vec<u8>> {
    let handle = unsafe { GetClipboardData(id) }.ok()?;
    if handle.is_invalid() {
        return None;
    }
    let hglobal = HGLOBAL(handle.0);
    let ptr = unsafe { GlobalLock(hglobal) };
    if ptr.is_null() {
        return None;
    }
    let size = unsafe { GlobalSize(hglobal) };
    let bytes = if size == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(ptr as *const u8, size) }.to_vec()
    };
    unsafe {
        let _ = GlobalUnlock(hglobal);
    }
    Some(bytes)
}

/// Write payloads to the clipboard with history and cloud-sync suppressed.
///
/// This is the only write path. It replaces the clipboard wholesale
/// (`EmptyClipboard` then each payload), and sets all three exclusion formats
/// **inside the same session** as the real data, which is what makes the
/// suppression apply.
///
/// `expected_seq` makes the "has anyone else written since we looked?" check
/// **atomic with the write**. Checking the sequence number before calling this
/// is not sufficient: acquiring the clipboard can take up to a second of
/// `OpenClipboard` retries, and a copy made during that window would be
/// destroyed by the `EmptyClipboard` below. Once the session is open no other
/// process can write, so re-checking here is the only check that means anything.
pub fn write_excluded(
    owner: &OwnerWindow,
    entries: &[(u32, &[u8])],
    expected_seq: Option<u32>,
) -> Result<(), ClipboardError> {
    let fmts = formats();
    let guard = ClipboardGuard::open_write(owner)?;

    if let Some(expected) = expected_seq {
        let actual = unsafe { GetClipboardSequenceNumber() };
        if actual != expected {
            drop(guard);
            return Err(ClipboardError::Superseded { expected, actual });
        }
    }

    unsafe { EmptyClipboard() }.map_err(|e| win32("EmptyClipboard", e))?;

    for (id, bytes) in entries {
        set_bytes(*id, bytes)?;
    }

    // "Any data" — but never a NULL handle, which would request delayed
    // rendering rather than suppression.
    set_bytes(fmts.exclude_monitor, &[0u8])?;
    // These two take a serialized DWORD.
    set_bytes(fmts.can_include_history, &0u32.to_ne_bytes())?;
    set_bytes(fmts.can_upload_cloud, &0u32.to_ne_bytes())?;

    drop(guard);
    Ok(())
}

/// Allocate one `GMEM_MOVEABLE` block, copy the payload in, and hand it over.
///
/// On success the system owns the handle and it must not be freed or written
/// afterwards. On failure we still own it and must free it.
fn set_bytes(id: u32, bytes: &[u8]) -> Result<(), ClipboardError> {
    let len = bytes.len().max(1);
    let hglobal =
        unsafe { GlobalAlloc(GMEM_MOVEABLE, len) }.map_err(|e| win32("GlobalAlloc", e))?;

    let ptr = unsafe { GlobalLock(hglobal) };
    if ptr.is_null() {
        let err = windows::core::Error::from_win32();
        unsafe {
            let _ = GlobalFree(Some(hglobal));
        }
        return Err(win32("GlobalLock", err));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
        let _ = GlobalUnlock(hglobal);
    }

    match unsafe { SetClipboardData(id, Some(HANDLE(hglobal.0))) } {
        Ok(_) => Ok(()),
        Err(e) => {
            // Still ours, so still our job to free.
            unsafe {
                let _ = GlobalFree(Some(hglobal));
            }
            Err(win32("SetClipboardData", e))
        }
    }
}

/// Put a snapshot back, itself excluded from clipboard history.
///
/// This is what makes the first half of acceptance criterion 8 achievable: the
/// restore is a second clipboard write and would otherwise produce its own
/// `Win+V` entry. It does nothing about the target application's own write.
/// `expected_seq` is checked inside the write session — see [`write_excluded`].
/// Pass the sequence value the caller last observed, so a copy made while we
/// were acquiring the clipboard aborts the restore instead of being destroyed
/// by it.
pub fn restore(
    owner: &OwnerWindow,
    snap: &Snapshot,
    expected_seq: Option<u32>,
) -> Result<(), ClipboardError> {
    let mut entries: Vec<(u32, &[u8])> = Vec::new();
    if let Some(text) = &snap.unicode_text {
        entries.push((unicode_text_format_id(), text.as_slice()));
    }
    if let Some(html) = &snap.html {
        entries.push((html_format_id(), html.as_slice()));
    }
    write_excluded(owner, &entries, expected_seq)
}

// ---------------------------------------------------------------------------
// Reading text
// ---------------------------------------------------------------------------

/// Read `CF_UNICODETEXT`.
///
/// `CF_TEXT` is deliberately not consulted: Windows synthesizes
/// `CF_UNICODETEXT` from it automatically, per the documented "Synthesized
/// Clipboard Formats" table.
pub fn read_text() -> Result<Option<String>, ClipboardError> {
    let _guard = ClipboardGuard::open_read()?;
    Ok(decode_text(read_format_bytes(unicode_text_format_id())))
}

fn decode_text(bytes: Option<Vec<u8>>) -> Option<String> {
    let bytes = bytes?;
    // GlobalSize is in bytes, and a trailing NUL is not guaranteed to be there.
    let units = bytes.len() / 2;
    if units == 0 {
        return Some(String::new());
    }
    let wide: Vec<u16> = (0..units)
        .map(|i| u16::from_ne_bytes([bytes[i * 2], bytes[i * 2 + 1]]))
        .collect();
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    Some(String::from_utf16_lossy(&wide[..end]))
}

// ---------------------------------------------------------------------------
// Sequence number and owner
// ---------------------------------------------------------------------------

/// `GetClipboardSequenceNumber`, which needs no `OpenClipboard`.
///
/// A return of 0 means this window station denies clipboard access; it is
/// logged once rather than looped on.
pub fn seq() -> u32 {
    let n = unsafe { GetClipboardSequenceNumber() };
    if n == 0 {
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "GetClipboardSequenceNumber returned 0 — no WINSTA_ACCESSCLIPBOARD on this window station; \
                 sequence polling cannot work"
            );
        }
    }
    n
}

/// The process that owns the current clipboard contents, if it has a window.
///
/// A soft signal only: applications can set the clipboard with no owner window
/// or through OLE, so `None` is common and must never discard a good capture.
pub fn owner_pid() -> Option<u32> {
    let hwnd = unsafe { GetClipboardOwner() }.ok()?;
    crate::foreground::pid_of_window(hwnd)
}

// ---------------------------------------------------------------------------
// Input injection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct SendInputShort {
    pub inserted: u32,
    pub error: u32,
}

fn key_input(vk: u32, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk as u16),
                wScan: scan,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: COPPER_INJECTED_TAG,
            },
        },
    }
}

/// Synthesize `Ctrl+C` into the foreground window.
///
/// `SendInput` reports how many events it actually inserted and can insert
/// fewer than requested — 0 with `ERROR_ACCESS_DENIED` against a
/// higher-integrity target is the expected case. A *partial* insert is the
/// dangerous one: Ctrl-down accepted and Ctrl-up dropped leaves Ctrl stuck down
/// system-wide, which the user experiences as their machine breaking rather
/// than as a failed capture. So any short return triggers recovery key-ups.
pub fn send_ctrl_c() -> Result<(), SendInputShort> {
    let inputs = [
        key_input(VK_CONTROL, false),
        key_input(VK_C, false),
        key_input(VK_C, true),
        key_input(VK_CONTROL, true),
    ];
    let inserted = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if inserted as usize == inputs.len() {
        return Ok(());
    }

    let error = unsafe { windows::Win32::Foundation::GetLastError() }.0;
    tracing::error!(
        inserted,
        error,
        "SendInput inserted fewer events than requested; sending recovery key-ups"
    );
    let recovery = [key_input(VK_C, true), key_input(VK_CONTROL, true)];
    unsafe { SendInput(&recovery, std::mem::size_of::<INPUT>() as i32) };

    Err(SendInputShort { inserted, error })
}

fn is_down(vk: u32) -> bool {
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

/// Which of the modifiers we care about are physically down right now.
pub fn modifiers_held() -> Vec<&'static str> {
    let mut held = Vec::new();
    for (vk, name) in [
        (VK_SHIFT, "Shift"),
        (VK_CONTROL, "Ctrl"),
        (VK_MENU, "Alt"),
        (VK_LWIN, "LWin"),
        (VK_RWIN, "RWin"),
    ] {
        if is_down(vk) {
            held.push(name);
        }
    }
    held
}

/// Wait up to `budget` for every modifier to come up.
///
/// This should rarely fire, since the trigger is on a key *release* and no
/// modifier is held by construction at that point. If it fires often during
/// testing, that is itself a finding.
pub fn wait_for_modifier_release(budget: Duration) -> Result<(), Vec<&'static str>> {
    let started = Instant::now();
    loop {
        let held = modifiers_held();
        if held.is_empty() {
            return Ok(());
        }
        if started.elapsed() >= budget {
            return Err(held);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// Tests — pure decoding only; everything else needs a real clipboard.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16_bytes(s: &str, nul_terminated: bool) -> Vec<u8> {
        let mut wide: Vec<u16> = s.encode_utf16().collect();
        if nul_terminated {
            wide.push(0);
        }
        wide.iter().flat_map(|c| c.to_ne_bytes()).collect()
    }

    #[test]
    fn decodes_a_nul_terminated_payload() {
        let bytes = utf16_bytes("hello", true);
        assert_eq!(decode_text(Some(bytes)).as_deref(), Some("hello"));
    }

    #[test]
    fn decodes_a_payload_with_no_terminator() {
        // GlobalSize is in bytes and a NUL is not guaranteed to be present.
        let bytes = utf16_bytes("hello", false);
        assert_eq!(decode_text(Some(bytes)).as_deref(), Some("hello"));
    }

    #[test]
    fn truncates_at_the_first_nul_rather_than_the_buffer_end() {
        // GlobalAlloc rounds up, so trailing slack after the NUL is normal.
        let mut bytes = utf16_bytes("hi", true);
        bytes.extend_from_slice(&utf16_bytes("junk", false));
        assert_eq!(decode_text(Some(bytes)).as_deref(), Some("hi"));
    }

    #[test]
    fn tolerates_an_odd_byte_count() {
        let mut bytes = utf16_bytes("ok", false);
        bytes.push(0xFF); // trailing half unit
        assert_eq!(decode_text(Some(bytes)).as_deref(), Some("ok"));
    }

    #[test]
    fn an_empty_payload_decodes_to_an_empty_string_not_none() {
        assert_eq!(decode_text(Some(Vec::new())).as_deref(), Some(""));
        assert_eq!(decode_text(None), None);
    }

    #[test]
    fn builtin_format_names_cover_the_ones_we_expect_to_see() {
        assert_eq!(builtin_format_name(13), Some("CF_UNICODETEXT"));
        assert_eq!(builtin_format_name(15), Some("CF_HDROP"));
        assert_eq!(builtin_format_name(0xC000), None);
    }
}
