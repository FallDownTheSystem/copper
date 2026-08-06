//! The only clipboard implementation in Copper (task-005 R13). Phase 5's Copy
//! and Copy as List call [`write_text_private`]; the capture fallback calls
//! everything else.
//!
//! Three things here are load-bearing and each is catastrophic done wrong:
//!
//! 1. **Writing needs an owner window.** `SetClipboardData` documents that a
//!    session opened with a NULL owner has its owner set to NULL by
//!    `EmptyClipboard`, "which causes SetClipboardData to fail" — so a write
//!    opened that way empties the user's clipboard and then cannot repopulate
//!    it. Hence [`OwnerWindow`], and hence [`Session::open_write`] refusing to
//!    exist without one. Reads have no such obligation, which is why the two are
//!    separate constructors rather than one with a flag: a write eventually made
//!    through the read path fails at `SetClipboardData` for reasons that look
//!    like nothing.
//! 2. **Every successful `OpenClipboard` must be paired with `CloseClipboard`**,
//!    or every process on the desktop fails to open the clipboard until we do.
//!    Hence [`Session`]; nothing here opens the clipboard any other way.
//! 3. **`SetClipboardData` takes ownership only on success.** A failed call
//!    leaves the caller holding an `HGLOBAL` it must free. Hence [`Moveable`].
//!
//! Every write Copper makes — including putting the user's own snapshot back —
//! carries the three clipboard-privacy formats, so Copper never seeds `Win+V`
//! history. The cost is stated rather than hidden: content that was eligible for
//! clipboard history before a fallback capture comes back excluded, because
//! Copper cannot preserve metadata it has no way to read.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
	GetLastError, GlobalFree, ERROR_SUCCESS, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::DataExchange::{
	CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardOwner,
	GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
	GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::UI::WindowsAndMessaging::{
	CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, HWND_MESSAGE, WINDOW_EX_STYLE,
	WINDOW_STYLE, WNDCLASSW,
};

use super::foreground::pid_of_window;

// --- tuning ------------------------------------------------------------------
// Kept here rather than in `capture/mod.rs` with the capture constants: this
// module serves Phase 5's copy commands too, and those are not captures. One
// named place each, never a literal at a call site, is the rule that matters.

/// How long `OpenClipboard` is retried before giving up. Ported from task-001,
/// which measured a worst case of roughly a second when another process holds
/// the clipboard. No retry protocol is documented by Microsoft.
const CLIPBOARD_OPEN_BUDGET: Duration = Duration::from_millis(1000);
/// First delay between `OpenClipboard` attempts; doubles up to the cap.
const CLIPBOARD_OPEN_RETRY_DELAY: Duration = Duration::from_millis(10);
const CLIPBOARD_OPEN_RETRY_DELAY_MAX: Duration = Duration::from_millis(200);
/// Formats larger than this are not snapshotted (task-005 R15). A snapshot that
/// skips a format is a lossy snapshot, which withholds the restore entirely.
const SNAPSHOT_FORMAT_SIZE_LIMIT: usize = 4 * 1024 * 1024;
/// A misbehaving clipboard owner must not be able to spin the enumeration
/// forever.
const MAX_ENUMERATED_FORMATS: usize = 256;
/// The ceiling on a clipboard payload [`read_attachment`] will copy out.
///
/// Deliberately larger than `attachments::ATTACHMENT_MAX_BYTES`, not equal to
/// it: a DIB is uncompressed, so a screenshot that lands well under the 10 MiB
/// attachment limit once it is PNG-encoded arrives here several times that
/// size. Matching the two would refuse ordinary screenshots for being large in
/// a representation the user never sees. This bound exists to stop an absurd
/// allocation, and the real limit is applied to the encoded bytes at ingest.
const ATTACHMENT_READ_LIMIT: usize = 128 * 1024 * 1024;

// --- format ids --------------------------------------------------------------
// Defined locally rather than imported: windows-rs puts the `CF_*` constants
// behind `Win32_System_Ole`, and pulling the whole OLE surface in for five
// ABI-fixed integers is a poor trade. The values are fixed by `winuser.h`.

const CF_TEXT: u32 = 1;
const CF_BITMAP: u32 = 2;
const CF_OEMTEXT: u32 = 7;
const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_LOCALE: u32 = 16;
const CF_DIBV5: u32 = 17;

// Named only so the tests can assert they are absent from both lists. Their
// whole significance here is that they are neither restorable nor exempt: a
// clipboard carrying one is a clipboard Copper must not restore over.
#[cfg(test)]
const CF_METAFILEPICT: u32 = 3;
#[cfg(test)]
const CF_PALETTE: u32 = 9;
#[cfg(test)]
const CF_ENHMETAFILE: u32 = 14;

/// The built-in formats the snapshot copies out and puts back.
///
/// `CF_BITMAP` is deliberately absent: it is a GDI handle, not `HGLOBAL`-backed
/// data, so a byte copy of it restores nothing. Windows synthesizes it from a
/// restored `CF_DIB`.
const BUILTIN_ALLOW_LIST: [u32; 5] = [CF_UNICODETEXT, CF_TEXT, CF_HDROP, CF_DIB, CF_DIBV5];

/// Formats whose presence is **not** evidence that the clipboard holds something
/// we cannot reproduce.
///
/// Windows synthesizes these from formats we do restore — `CF_TEXT`,
/// `CF_OEMTEXT` and `CF_LOCALE` appear alongside a `CF_UNICODETEXT` the owner
/// actually set, and `CF_BITMAP` alongside a `CF_DIB`. Task-001 found this the
/// hard way and warned that anyone reading its `unrestorable` list would
/// otherwise draw the wrong conclusion. Here the stake is higher than a
/// misleading log: treating a synthesized format as unreproducible would mark
/// almost every ordinary text clipboard lossy and suppress every restore.
///
/// The list is exactly three, and metafiles and `CF_PALETTE` are deliberately
/// **not** on it. Windows synthesizes neither from anything Copper restores, so
/// their presence means real content a restore would destroy — they must count
/// toward lossy and withhold it.
const SYNTHESIZED: [u32; 3] = [CF_OEMTEXT, CF_LOCALE, CF_BITMAP];

// --- errors ------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ClipboardError {
	/// `OpenClipboard` kept failing for the whole retry budget.
	Busy { attempts: u32, elapsed_ms: u64 },
	/// Somebody else wrote the clipboard between the caller's last look and this
	/// write, so the write was abandoned rather than performed. Not a failure —
	/// it is the guard that stops a restore from clobbering a fresh user copy.
	Superseded { expected: u32, actual: u32 },
	Win32 {
		op: &'static str,
		code: i32,
		message: String,
	},
}

impl std::fmt::Display for ClipboardError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			ClipboardError::Busy {
				attempts,
				elapsed_ms,
			} => write!(
				f,
				"the clipboard stayed locked by another process ({attempts} attempts over {elapsed_ms} ms)"
			),
			ClipboardError::Superseded { expected, actual } => write!(
				f,
				"the clipboard changed before the write (expected sequence {expected}, found \
				 {actual}); write abandoned"
			),
			ClipboardError::Win32 { op, code, message } => {
				write!(f, "{op} failed: 0x{code:08X} {message}")
			}
		}
	}
}

impl std::error::Error for ClipboardError {}

type Result<T> = std::result::Result<T, ClipboardError>;

fn win32(op: &'static str, err: windows::core::Error) -> ClipboardError {
	ClipboardError::Win32 {
		op,
		code: err.code().0,
		message: err.message(),
	}
}

// --- owner window ------------------------------------------------------------

unsafe extern "system" fn owner_wndproc(
	hwnd: HWND,
	msg: u32,
	wparam: WPARAM,
	lparam: LPARAM,
) -> LRESULT {
	// SAFETY: the default handler is always a valid response to any message.
	unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A hidden message-only window that exists solely to be the clipboard owner,
/// for exactly as long as one write session.
///
/// Short-lived on purpose. A long-lived owner window would have to be pumped for
/// the lifetime of the process — an unpumped window hangs any process that sends
/// it a message — and nothing here needs one to outlive its write: Copper sets
/// real data rather than delayed-rendering it, so it never receives
/// `WM_RENDERFORMAT`.
struct OwnerWindow {
	hwnd: HWND,
}

impl OwnerWindow {
	fn create() -> Result<Self> {
		static CLASS: OnceLock<std::result::Result<(), String>> = OnceLock::new();

		// SAFETY: no preconditions; failure is reported through the Result.
		let hinstance =
			unsafe { GetModuleHandleW(None) }.map_err(|err| win32("GetModuleHandleW", err))?;
		let class_name = w!("CopperClipboardOwner");

		let registered = CLASS.get_or_init(|| {
			let class = WNDCLASSW {
				lpfnWndProc: Some(owner_wndproc),
				hInstance: hinstance.into(),
				lpszClassName: class_name,
				..Default::default()
			};
			// SAFETY: `class` outlives the call and names a valid wndproc.
			if unsafe { RegisterClassW(&class) } == 0 {
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

		// SAFETY: the class is registered above and HWND_MESSAGE is the documented
		// parent for a message-only window.
		let hwnd = unsafe {
			CreateWindowExW(
				WINDOW_EX_STYLE(0),
				class_name,
				w!("copper-clipboard-owner"),
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
		.map_err(|err| win32("CreateWindowExW", err))?;

		Ok(Self { hwnd })
	}
}

impl Drop for OwnerWindow {
	fn drop(&mut self) {
		// SAFETY: created above and destroyed exactly once, here. Destroying it
		// after CloseClipboard does not take the data back — the handles were
		// transferred to the system.
		unsafe {
			let _ = DestroyWindow(self.hwnd);
		}
	}
}

// --- sessions ----------------------------------------------------------------

/// An open clipboard session. Dropping it calls `CloseClipboard`.
struct Session {
	_private: (),
}

impl Session {
	/// Read-only. May pass a NULL owner because no read path calls
	/// `EmptyClipboard` or `SetClipboardData`.
	fn open_read() -> Result<Self> {
		Self::open(None)
	}

	/// Write. Requires a real window owned by this process — see the module note.
	fn open_write(owner: &OwnerWindow) -> Result<Self> {
		Self::open(Some(owner.hwnd))
	}

	fn open(owner: Option<HWND>) -> Result<Self> {
		let started = Instant::now();
		let mut delay = CLIPBOARD_OPEN_RETRY_DELAY;
		let mut attempts = 0u32;

		loop {
			attempts += 1;
			// SAFETY: `owner` is either NULL or a live window of this process.
			if unsafe { OpenClipboard(owner) }.is_ok() {
				return Ok(Self { _private: () });
			}
			if started.elapsed() >= CLIPBOARD_OPEN_BUDGET {
				return Err(ClipboardError::Busy {
					attempts,
					elapsed_ms: started.elapsed().as_millis() as u64,
				});
			}
			thread::sleep(delay);
			delay = (delay * 2).min(CLIPBOARD_OPEN_RETRY_DELAY_MAX);
		}
	}
}

impl Drop for Session {
	fn drop(&mut self) {
		// SAFETY: paired with the successful OpenClipboard that produced `self`.
		unsafe {
			let _ = CloseClipboard();
		}
	}
}

// --- registered formats ------------------------------------------------------

struct Registered {
	html: u32,
	rtf: u32,
	exclude_monitor: u32,
	can_include_history: u32,
	can_upload_cloud: u32,
}

fn registered() -> &'static Registered {
	static FORMATS: OnceLock<Registered> = OnceLock::new();
	FORMATS.get_or_init(|| Registered {
		html: register(w!("HTML Format")),
		rtf: register(w!("Rich Text Format")),
		// All three are documented on Microsoft Learn's "Clipboard Formats" page.
		// `ExcludeClipboardContentFromMonitorProcessing` alone is documented to
		// cover both history and cloud sync, but the interaction between the three
		// is not documented precisely enough to rely on one, so all three are set.
		exclude_monitor: register(w!("ExcludeClipboardContentFromMonitorProcessing")),
		can_include_history: register(w!("CanIncludeInClipboardHistory")),
		can_upload_cloud: register(w!("CanUploadToCloudClipboard")),
	})
}

fn register(name: PCWSTR) -> u32 {
	// SAFETY: `name` is a static NUL-terminated wide string.
	unsafe { RegisterClipboardFormatW(name) }
}

// --- allocation --------------------------------------------------------------

/// A `GMEM_MOVEABLE` block that frees itself unless the clipboard takes it.
///
/// `SetClipboardData` takes ownership **only on success**; on failure the caller
/// is still responsible for the handle. Expressing that as a guard rather than as
/// a pair of manual `GlobalFree` calls is what keeps it true on every error path.
struct Moveable {
	hglobal: Option<HGLOBAL>,
}

impl Moveable {
	fn with_bytes(bytes: &[u8]) -> Result<Self> {
		// Never a zero-length allocation: a zero-size GMEM_MOVEABLE block yields a
		// discarded handle, which is needlessly ambiguous where the presence of the
		// format is the whole signal.
		let len = bytes.len().max(1);
		// SAFETY: no preconditions; failure is reported through the Result.
		let hglobal =
			unsafe { GlobalAlloc(GMEM_MOVEABLE, len) }.map_err(|err| win32("GlobalAlloc", err))?;
		let guard = Self {
			hglobal: Some(hglobal),
		};

		// SAFETY: `hglobal` was just allocated with at least `bytes.len()` bytes.
		let ptr = unsafe { GlobalLock(hglobal) };
		if ptr.is_null() {
			return Err(win32("GlobalLock", windows::core::Error::from_win32()));
		}
		// SAFETY: `ptr` is locked, writable, and at least `bytes.len()` bytes long;
		// the source and destination cannot overlap.
		unsafe {
			std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
			let _ = GlobalUnlock(hglobal);
		}

		Ok(guard)
	}

	/// Hands the block to the clipboard. On success the system owns it; on
	/// failure this guard still does and frees it on drop.
	fn set_as(mut self, format: u32) -> Result<()> {
		let Some(hglobal) = self.hglobal else {
			return Ok(());
		};
		// SAFETY: the clipboard is open for writing and `hglobal` is a live
		// GMEM_MOVEABLE block that is not locked.
		match unsafe { SetClipboardData(format, Some(HANDLE(hglobal.0))) } {
			Ok(_) => {
				self.hglobal = None;
				Ok(())
			}
			Err(err) => Err(win32("SetClipboardData", err)),
		}
	}
}

impl Drop for Moveable {
	fn drop(&mut self) {
		if let Some(hglobal) = self.hglobal.take() {
			// SAFETY: still ours — SetClipboardData either was never called or
			// failed, and a failed call does not take ownership.
			unsafe {
				let _ = GlobalFree(Some(hglobal));
			}
		}
	}
}

// --- reading -----------------------------------------------------------------

/// `GetClipboardSequenceNumber`, which needs no open session.
///
/// Returns 0 when this window station denies clipboard access, which callers
/// must treat as "polling unavailable" rather than as a baseline that will never
/// change. It also increments on `EmptyClipboard`, so a target answering an
/// injected `Ctrl+C` moves it by one or by two depending on whether it empties
/// first — which is why the fallback polls for *any* change and guards the
/// restore with the tokens [`Snapshot::sequence`] and [`read_text`] hand back
/// from inside their own sessions, rather than by predicting a value.
pub fn sequence_number() -> u32 {
	// SAFETY: no preconditions.
	unsafe { GetClipboardSequenceNumber() }
}

/// The process owning the current clipboard contents, if it has a window.
///
/// A soft signal: applications set the clipboard with no owner window or through
/// OLE, so `None` is common and must never by itself discard a good capture.
pub fn owner_pid() -> Option<u32> {
	// SAFETY: no preconditions; returns null when there is no owner window.
	let hwnd = unsafe { GetClipboardOwner() }.ok()?;
	pid_of_window(hwnd)
}

/// Reads `CF_UNICODETEXT` — `None` when the format is absent — along with the
/// sequence number as it was **inside the same session**.
///
/// The sequence comes back with the text because the caller needs a value it can
/// hand to [`restore`], and sampling one after this session closes would leave a
/// gap: a copy landing in that gap would raise the live sequence, the restore's
/// in-session check would compare equal, and the user's new content would be
/// destroyed by the very check meant to protect it.
///
/// `CF_TEXT` is deliberately not consulted: Windows synthesizes `CF_UNICODETEXT`
/// from it automatically.
pub fn read_text() -> Result<(Option<String>, u32)> {
	let _session = Session::open_read()?;
	// No size limit on a read the caller asked for by name; the capture path's
	// own `MAX_CAPTURE_CHARS` is what bounds that.
	let bytes = match read_format_bytes(CF_UNICODETEXT, usize::MAX) {
		FormatBytes::Bytes(bytes) => Some(bytes),
		FormatBytes::TooLarge | FormatBytes::Unreadable => None,
	};
	Ok((decode_text(bytes), sequence_number()))
}

/// What the clipboard holds that could become an attachment.
///
/// The two arms are genuinely different things and are kept apart rather than
/// normalised into one: a `CF_HDROP` is a *file paste* — the user copied
/// something in Explorer — and it keeps the original filenames, whereas a
/// bitmap has no name and no file behind it at all.
#[derive(Debug, Clone)]
pub enum ClipboardAttachment {
	/// Raw device-independent bitmap bytes, exactly as the clipboard holds them:
	/// a BMP body with no `BITMAPFILEHEADER`.
	///
	/// Deliberately **not** encoded here. This module is the Win32 boundary and
	/// nothing else; teaching it about PNG would give it an image-codec
	/// dependency and a second reason to change. `attachments::thumb` owns the
	/// conversion, and the rule that a DIB never reaches disk as one.
	Dib(Vec<u8>),
	/// Files copied in Explorer, which route to the same path a drop does.
	Files(Vec<PathBuf>),
}

/// The clipboard's attachable content, or `None` when there is none.
///
/// **Text always wins.** `CF_UNICODETEXT` being present returns `None` before
/// anything else is considered, so pasting a copied code snippet can never
/// silently become a screenshot — Windows puts a bitmap on the clipboard
/// alongside text more often than one would like, and the failure would be
/// silent and confusing in exactly the surface where text is the whole point.
///
/// Preference order after that is `CF_DIBV5`, `CF_DIB`, then `CF_HDROP`. V5
/// first because it carries an alpha channel that the older header cannot
/// describe.
///
/// One session for the whole decision, like [`snapshot`]: asking "is there
/// text?" in one session and "is there an image?" in another leaves a window in
/// which the answer changes between them, and the caller would act on a
/// clipboard that never existed. This is still the only module that opens the
/// clipboard, and still through [`Session::open_read`].
pub fn read_attachment() -> Result<Option<ClipboardAttachment>> {
	let _session = Session::open_read()?;

	if has_format(CF_UNICODETEXT) {
		return Ok(None);
	}

	for format in [CF_DIBV5, CF_DIB] {
		if let FormatBytes::Bytes(bytes) = read_format_bytes(format, ATTACHMENT_READ_LIMIT) {
			return Ok(Some(ClipboardAttachment::Dib(bytes)));
		}
	}

	if let FormatBytes::Bytes(bytes) = read_format_bytes(CF_HDROP, ATTACHMENT_READ_LIMIT) {
		let paths = parse_hdrop(&bytes);
		if !paths.is_empty() {
			return Ok(Some(ClipboardAttachment::Files(paths)));
		}
	}

	Ok(None)
}

/// Whether the clipboard advertises a format, without copying its payload.
///
/// `IsClipboardFormatAvailable` is not used: it works outside a session and
/// would therefore answer about a *different* moment than the reads beside it.
fn has_format(format: u32) -> bool {
	// SAFETY: called only with the clipboard open. A null or failed handle means
	// the format is not really there, which is the answer either way.
	unsafe { GetClipboardData(format) }.is_ok_and(|handle| !handle.is_invalid())
}

/// Decodes a `DROPFILES` block into paths.
///
/// Parsed by hand rather than through `DragQueryFileW`, which would pull the
/// whole `Win32_UI_Shell` surface in for one call over a structure that is four
/// fixed fields and a double-NUL-terminated string list. The offset is read from
/// `pFiles` rather than assumed to be 20, because that is what the field is for.
///
/// `fWide` is honoured: a producer may still hand over ANSI, and reading those
/// bytes as UTF-16 would yield paths made of nonsense rather than no paths at
/// all.
fn parse_hdrop(bytes: &[u8]) -> Vec<PathBuf> {
	let Some(offset) = bytes.get(0..4).map(|head| {
		u32::from_le_bytes(head.try_into().expect("a four-byte slice")) as usize
	}) else {
		return Vec::new();
	};
	let wide = bytes
		.get(16..20)
		.map(|flag| u32::from_le_bytes(flag.try_into().expect("a four-byte slice")) != 0)
		.unwrap_or(false);
	let Some(list) = bytes.get(offset..) else {
		return Vec::new();
	};

	if wide {
		let units: Vec<u16> = list
			.chunks_exact(2)
			.map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
			.collect();
		units
			.split(|&unit| unit == 0)
			.filter(|part| !part.is_empty())
			.map(|part| PathBuf::from(String::from_utf16_lossy(part)))
			.collect()
	} else {
		list.split(|&byte| byte == 0)
			.filter(|part| !part.is_empty())
			.map(|part| PathBuf::from(String::from_utf8_lossy(part).into_owned()))
			.collect()
	}
}

/// What one format's payload turned out to be.
enum FormatBytes {
	Bytes(Vec<u8>),
	/// Advertised, readable, and over the caller's limit. Measured under the
	/// lock and declined *before* the copy, so a 30 MB screenshot costs a
	/// `GlobalSize` rather than an allocation and a memcpy of bytes we drop.
	TooLarge,
	/// Advertised but not readable at all.
	Unreadable,
}

/// Copies a format's payload out of the clipboard's own memory, unless it is
/// larger than `limit`. The handle belongs to the clipboard: it is locked and
/// read, never freed.
fn read_format_bytes(format: u32, limit: usize) -> FormatBytes {
	// SAFETY: called only with the clipboard open.
	let handle = match unsafe { GetClipboardData(format) } {
		Ok(handle) => handle,
		Err(_) => return FormatBytes::Unreadable,
	};
	if handle.is_invalid() {
		return FormatBytes::Unreadable;
	}
	let hglobal = HGLOBAL(handle.0);
	// SAFETY: `hglobal` is an HGLOBAL-backed clipboard handle; unlocked below.
	let ptr = unsafe { GlobalLock(hglobal) };
	if ptr.is_null() {
		return FormatBytes::Unreadable;
	}
	// SAFETY: `hglobal` is locked, so its size is stable for this call.
	let size = unsafe { GlobalSize(hglobal) };
	// A zero size after a *successful* lock is `GlobalSize` failing, not a
	// legitimately empty payload — no application advertises a format and then
	// backs it with nothing. Treated as a failed read so the snapshot refuses
	// rather than recording an empty payload it would later restore over real
	// content.
	let outcome = if size == 0 {
		FormatBytes::Unreadable
	} else if size > limit {
		FormatBytes::TooLarge
	} else {
		// SAFETY: `ptr` is valid for `size` bytes while the lock is held.
		FormatBytes::Bytes(unsafe { std::slice::from_raw_parts(ptr as *const u8, size) }.to_vec())
	};
	// SAFETY: paired with the GlobalLock above.
	unsafe {
		let _ = GlobalUnlock(hglobal);
	}
	outcome
}

fn decode_text(bytes: Option<Vec<u8>>) -> Option<String> {
	let bytes = bytes?;
	// `GlobalSize` is in bytes and rounds up, and a trailing NUL is not
	// guaranteed to be present — so the length is taken from the buffer and the
	// string from the first NUL, whichever comes first. An odd trailing byte is a
	// malformed payload; the half unit is dropped rather than read past.
	let units = bytes.len() / 2;
	let wide: Vec<u16> = (0..units)
		.map(|i| u16::from_ne_bytes([bytes[i * 2], bytes[i * 2 + 1]]))
		.collect();
	let end = wide.iter().position(|&unit| unit == 0).unwrap_or(wide.len());
	Some(String::from_utf16_lossy(&wide[..end]))
}

// --- snapshot ----------------------------------------------------------------

/// What was on the clipboard before Copper touched it.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
	/// Allow-listed formats and their raw payload bytes, in enumeration order.
	entries: Vec<(u32, Vec<u8>)>,
	/// A format was present that this snapshot cannot faithfully reproduce —
	/// outside the allow-list, over `SNAPSHOT_FORMAT_SIZE_LIMIT`, or beyond the
	/// enumeration cap.
	lossy: bool,
	/// The sequence number as it was **inside** the session that took this
	/// snapshot. See [`Snapshot::sequence`].
	sequence: u32,
}

impl Snapshot {
	/// Whether restoring this snapshot would destroy something.
	///
	/// The restore is skipped when this is true (task-005 R15a, checkpoint-1
	/// ruling): leaving Copper's captured text on the clipboard is better than a
	/// lossy restore that silently drops the user's richer content.
	pub fn is_lossy(&self) -> bool {
		self.lossy
	}

	/// Whether there is anything to put back at all.
	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}

	/// The clipboard's sequence number at the moment these bytes were copied.
	///
	/// Sampled inside the snapshot's own session, so it describes *this* content
	/// and nothing else. A caller holding a snapshot whose token no longer matches
	/// the live sequence is holding a stale copy of a clipboard that has since
	/// moved on, and restoring it would destroy whatever replaced it.
	pub fn sequence(&self) -> u32 {
		self.sequence
	}
}

/// Copies the clipboard's restorable formats out under one session.
///
/// The single session is itself the atomicity guarantee: no other process can
/// write while it is held, so the bytes and the sequence number returned with
/// them describe the same clipboard. A sequence check *across* the copy would
/// add nothing and would actively harm — reading a delayed-rendered format makes
/// the owning application call `SetClipboardData`, which bumps the sequence, so
/// a strict check would refuse to snapshot exactly the applications worth
/// capturing from.
pub fn snapshot() -> Result<Snapshot> {
	let _session = Session::open_read()?;
	let formats = registered();

	// Enumerate every id first and only then fetch payloads: `GetClipboardData`
	// can trigger delayed rendering in the owning application, which changes the
	// clipboard *during* the walk and can truncate or corrupt the enumeration.
	let mut ids: Vec<u32> = Vec::new();
	let mut truncated = false;
	let mut id = 0u32;
	loop {
		// SAFETY: called only with the clipboard open.
		id = unsafe { EnumClipboardFormats(id) };
		if id == 0 {
			// Zero is both "no more formats" and "the call failed", and the only way
			// to tell them apart is the last error. A failed enumeration read as a
			// complete one is the dangerous direction: it would produce a snapshot
			// that looks whole, restore over the real clipboard, and drop whatever
			// the enumeration never reached.
			// SAFETY: no preconditions.
			let last = unsafe { GetLastError() };
			if last != ERROR_SUCCESS {
				return Err(ClipboardError::Win32 {
					op: "EnumClipboardFormats",
					code: last.0 as i32,
					message: "the format enumeration failed part-way; refusing to snapshot a \
					          clipboard it cannot see all of"
						.to_owned(),
				});
			}
			break;
		}
		if ids.len() >= MAX_ENUMERATED_FORMATS {
			// A misbehaving clipboard owner must not be able to spin us forever, but
			// stopping early means there is content this snapshot has not seen —
			// which is exactly what lossy means.
			truncated = true;
			break;
		}
		ids.push(id);
	}

	let mut snapshot = Snapshot {
		lossy: truncated,
		// Sampled inside this session, so no other process can have written between
		// the copy and the reading of it.
		sequence: sequence_number(),
		..Snapshot::default()
	};
	for id in ids {
		if !is_restorable(id, formats) {
			// Copper's own privacy markers are not a loss: they are what the last
			// Copper write left behind, and the restore sets them again anyway.
			if !is_ignorable(id, formats) {
				snapshot.lossy = true;
			}
			continue;
		}

		match read_format_bytes(id, SNAPSHOT_FORMAT_SIZE_LIMIT) {
			FormatBytes::Bytes(bytes) => snapshot.entries.push((id, bytes)),
			// Over the limit, so not preserved — which is what lossy means.
			FormatBytes::TooLarge => snapshot.lossy = true,
			// A format we intend to restore but cannot *read* is a hard failure,
			// not an absent format. Reporting it as absent would let the snapshot
			// look complete, the injection proceed, and the restore silently omit
			// content the user had — destroying it. Failing here leaves the
			// clipboard untouched, because nothing has been injected yet.
			FormatBytes::Unreadable => {
				return Err(ClipboardError::Win32 {
					op: "GetClipboardData",
					code: 0,
					message: format!(
						"format {id} was advertised but could not be read; refusing to continue \
						 and risk losing it"
					),
				})
			}
		}
	}

	Ok(snapshot)
}

fn is_restorable(id: u32, formats: &Registered) -> bool {
	BUILTIN_ALLOW_LIST.contains(&id) || id == formats.html || id == formats.rtf
}

fn is_ignorable(id: u32, formats: &Registered) -> bool {
	SYNTHESIZED.contains(&id)
		|| id == formats.exclude_monitor
		|| id == formats.can_include_history
		|| id == formats.can_upload_cloud
}

/// Puts a snapshot back, itself excluded from clipboard history.
///
/// `expected_seq` is re-checked **inside** the write session, where no other
/// process can interleave. Checking it beforehand is not sufficient: acquiring
/// the clipboard can take up to a second of retries, and anything the user
/// copied during that window would be destroyed by the `EmptyClipboard`.
pub fn restore(snapshot: &Snapshot, expected_seq: u32) -> Result<()> {
	write_excluded(&snapshot.entries, Some(expected_seq))
}

// --- writing -----------------------------------------------------------------

/// Puts text on the clipboard, excluded from clipboard history and cloud sync.
///
/// Phase 5's Copy and Copy as List are the callers this exists for; nothing in
/// Phase 4 puts Copper's own text on the clipboard. Built and tested a phase
/// early (task-005 R13) rather than left for the phase that first calls it — the
/// write path is where a clipboard implementation destroys data, and shipping it
/// untested for a phase is how that goes unnoticed. Phase 5 has since arrived,
/// and `clipboard::clipboard_write_text` is the caller.
pub fn write_text_private(text: &str) -> Result<()> {
	let mut wide: Vec<u16> = text.encode_utf16().collect();
	wide.push(0);
	let bytes: Vec<u8> = wide.iter().flat_map(|unit| unit.to_ne_bytes()).collect();
	write_excluded(&[(CF_UNICODETEXT, bytes)], None)
}

/// The one write path: an owner window, `EmptyClipboard`, the payloads, and the
/// three privacy formats, all inside one session.
fn write_excluded(entries: &[(u32, Vec<u8>)], expected_seq: Option<u32>) -> Result<()> {
	let formats = registered();
	let owner = OwnerWindow::create()?;
	let session = Session::open_write(&owner)?;

	if let Some(expected) = expected_seq {
		let actual = sequence_number();
		if actual != expected {
			return Err(ClipboardError::Superseded { expected, actual });
		}
	}

	// Every replacement block is allocated before `EmptyClipboard`, so a failed
	// allocation cannot leave the clipboard emptied and unrepopulated.
	let mut prepared: Vec<(u32, Moveable)> = Vec::with_capacity(entries.len() + 3);
	for (id, bytes) in entries {
		prepared.push((*id, Moveable::with_bytes(bytes)?));
	}
	// "Any data" for the marker format — but never a NULL handle, which would
	// request delayed rendering rather than suppression. The other two take a
	// serialized DWORD of zero.
	prepared.push((formats.exclude_monitor, Moveable::with_bytes(&[0u8])?));
	prepared.push((
		formats.can_include_history,
		Moveable::with_bytes(&0u32.to_ne_bytes())?,
	));
	prepared.push((
		formats.can_upload_cloud,
		Moveable::with_bytes(&0u32.to_ne_bytes())?,
	));

	// `RegisterClipboardFormatW` returns 0 on failure, and 0 is not a format id.
	// Caught here, before `EmptyClipboard`, because afterwards it is too late to
	// decline: the clipboard would already be empty and a `SetClipboardData(0, ..)`
	// would leave it that way. Failing now costs a capture and nothing else.
	if let Some((bad, _)) = prepared.iter().find(|(id, _)| *id == 0) {
		return Err(ClipboardError::Win32 {
			op: "RegisterClipboardFormatW",
			code: 0,
			message: format!(
				"format id {bad} is not valid; refusing to empty the clipboard for a write \
				 that cannot be completed"
			),
		});
	}

	// SAFETY: the clipboard is open for writing with a live owner window.
	unsafe { EmptyClipboard() }.map_err(|err| win32("EmptyClipboard", err))?;

	// Past `EmptyClipboard` the clipboard is already gone, so abandoning on the
	// first failure would leave it holding *less* than a complete failure would.
	// Every remaining format is attempted and the first error reported once they
	// have been — partial content beats none, and the caller still learns the
	// write did not fully succeed.
	let mut first_failure = None;
	for (id, block) in prepared {
		if let Err(err) = block.set_as(id) {
			first_failure.get_or_insert(err);
		}
	}

	drop(session);
	match first_failure {
		Some(err) => Err(err),
		None => Ok(()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn utf16_bytes(text: &str, nul_terminated: bool) -> Vec<u8> {
		let mut wide: Vec<u16> = text.encode_utf16().collect();
		if nul_terminated {
			wide.push(0);
		}
		wide.iter().flat_map(|unit| unit.to_ne_bytes()).collect()
	}

	#[test]
	fn decodes_a_nul_terminated_payload() {
		assert_eq!(
			decode_text(Some(utf16_bytes("hello", true))).as_deref(),
			Some("hello")
		);
	}

	#[test]
	fn decodes_a_payload_with_no_terminator() {
		// GlobalSize is in bytes and a NUL is not guaranteed to be present.
		assert_eq!(
			decode_text(Some(utf16_bytes("hello", false))).as_deref(),
			Some("hello")
		);
	}

	#[test]
	fn truncates_at_the_first_nul_rather_than_the_buffer_end() {
		// GlobalAlloc rounds up, so trailing slack after the NUL is normal.
		let mut bytes = utf16_bytes("hi", true);
		bytes.extend_from_slice(&utf16_bytes("junk", false));
		assert_eq!(decode_text(Some(bytes)).as_deref(), Some("hi"));
	}

	#[test]
	fn drops_an_odd_trailing_byte_rather_than_reading_past_the_end() {
		let mut bytes = utf16_bytes("ok", false);
		bytes.push(0xFF);
		assert_eq!(decode_text(Some(bytes)).as_deref(), Some("ok"));
	}

	#[test]
	fn an_empty_payload_decodes_to_an_empty_string_not_none() {
		assert_eq!(decode_text(Some(Vec::new())).as_deref(), Some(""));
		assert_eq!(decode_text(None), None);
	}

	#[test]
	fn synthesized_formats_do_not_make_a_snapshot_lossy() {
		// The case that would suppress every restore if it were wrong: a plain
		// text clipboard always carries CF_TEXT, CF_OEMTEXT and CF_LOCALE
		// alongside the CF_UNICODETEXT the owner actually set.
		let formats = registered();
		for id in [CF_OEMTEXT, CF_LOCALE, CF_BITMAP] {
			assert!(
				is_ignorable(id, formats),
				"format {id} is synthesized and must not count as a loss"
			);
		}
	}

	#[test]
	fn metafiles_and_palettes_count_toward_lossy() {
		// Windows synthesizes none of these from anything Copper restores, so their
		// presence is real content a restore would destroy. Exempting them would
		// let the restore run and drop them silently — the opposite of what the
		// lossy check is for.
		let formats = registered();
		for id in [CF_METAFILEPICT, CF_ENHMETAFILE, CF_PALETTE] {
			assert!(!is_ignorable(id, formats));
			assert!(!is_restorable(id, formats));
		}
	}

	#[test]
	fn copper_own_privacy_markers_are_not_a_loss() {
		let formats = registered();
		for id in [
			formats.exclude_monitor,
			formats.can_include_history,
			formats.can_upload_cloud,
		] {
			assert!(is_ignorable(id, formats));
		}
	}

	#[test]
	fn the_allow_list_is_restorable_and_an_unknown_format_is_not() {
		let formats = registered();
		for id in BUILTIN_ALLOW_LIST {
			assert!(is_restorable(id, formats));
		}
		assert!(is_restorable(formats.html, formats));
		assert!(is_restorable(formats.rtf, formats));

		// A private format an application registered for itself.
		let private = register(w!("Copper Test Private Format"));
		assert!(!is_restorable(private, formats));
		assert!(!is_ignorable(private, formats));
	}

	// --- CF_HDROP ---

	fn dropfiles(paths: &[&str], wide: bool) -> Vec<u8> {
		let mut bytes = Vec::new();
		bytes.extend_from_slice(&20u32.to_le_bytes());
		bytes.extend_from_slice(&0i32.to_le_bytes());
		bytes.extend_from_slice(&0i32.to_le_bytes());
		bytes.extend_from_slice(&0u32.to_le_bytes());
		bytes.extend_from_slice(&u32::from(wide).to_le_bytes());
		for path in paths {
			if wide {
				bytes.extend(utf16_bytes(path, true));
			} else {
				bytes.extend_from_slice(path.as_bytes());
				bytes.push(0);
			}
		}
		// The list's own terminator, on top of the last string's.
		if wide {
			bytes.extend_from_slice(&0u16.to_ne_bytes());
		} else {
			bytes.push(0);
		}
		bytes
	}

	#[test]
	fn a_wide_drop_list_decodes_to_every_path() {
		let paths = parse_hdrop(&dropfiles(&[r"C:\a\one.png", r"D:\two.pdf"], true));
		assert_eq!(
			paths,
			vec![PathBuf::from(r"C:\a\one.png"), PathBuf::from(r"D:\two.pdf")]
		);
	}

	/// Read as UTF-16 an ANSI list decodes to nonsense rather than to nothing,
	/// which is the failure worth having a test for.
	#[test]
	fn an_ansi_drop_list_is_decoded_as_ansi() {
		let paths = parse_hdrop(&dropfiles(&[r"C:\a\one.png"], false));
		assert_eq!(paths, vec![PathBuf::from(r"C:\a\one.png")]);
	}

	#[test]
	fn the_offset_is_read_from_the_structure_rather_than_assumed() {
		let mut bytes = dropfiles(&[r"C:\a.png"], true);
		// Pad the structure and re-point `pFiles`, exactly as a producer using a
		// larger header would.
		bytes.splice(20..20, std::iter::repeat_n(0u8, 8));
		bytes[0..4].copy_from_slice(&28u32.to_le_bytes());
		assert_eq!(parse_hdrop(&bytes), vec![PathBuf::from(r"C:\a.png")]);
	}

	#[test]
	fn a_truncated_or_empty_drop_list_yields_no_paths() {
		assert!(parse_hdrop(&[]).is_empty());
		assert!(parse_hdrop(&[0u8; 3]).is_empty());
		assert!(parse_hdrop(&dropfiles(&[], true)).is_empty());
		// An offset past the end of the block.
		let mut bytes = dropfiles(&[r"C:\a.png"], true);
		bytes[0..4].copy_from_slice(&9999u32.to_le_bytes());
		assert!(parse_hdrop(&bytes).is_empty());
	}

	#[test]
	fn cf_bitmap_is_not_in_the_allow_list() {
		// It is a GDI handle, not HGLOBAL-backed: a byte copy restores nothing.
		// Windows synthesizes it from a restored CF_DIB.
		assert!(!BUILTIN_ALLOW_LIST.contains(&CF_BITMAP));
	}

	// --- integration: these touch the real clipboard --------------------------
	// Ignored by default so `cargo test` stays hermetic — they replace whatever
	// the person running them had copied. Run with:
	//   cargo test -- --ignored --test-threads=1

	#[test]
	#[ignore = "touches the real clipboard"]
	fn write_then_read_round_trips() {
		let payload = "copper clipboard round trip";
		write_text_private(payload).expect("write");
		assert_eq!(read_text().expect("read").0.as_deref(), Some(payload));
	}

	#[test]
	#[ignore = "touches the real clipboard"]
	fn a_snapshot_carries_the_sequence_of_the_content_it_copied() {
		write_text_private("original").expect("seed");
		let snapshot = snapshot().expect("snapshot");
		assert_eq!(
			snapshot.sequence(),
			sequence_number(),
			"a fresh snapshot's token must match the live clipboard"
		);

		write_text_private("something the user copied afterwards").expect("overwrite");
		assert_ne!(
			snapshot.sequence(),
			sequence_number(),
			"the token must go stale the moment the clipboard moves on"
		);
	}

	#[test]
	#[ignore = "touches the real clipboard"]
	fn a_write_moves_the_sequence_number() {
		let before = sequence_number();
		write_text_private("copper sequence probe").expect("write");
		assert_ne!(before, sequence_number());
	}

	#[test]
	#[ignore = "touches the real clipboard"]
	fn an_oversized_format_is_declined_before_it_is_copied() {
		// UTF-16 doubles it, so the payload is over the limit either way.
		let payload = "x".repeat(SNAPSHOT_FORMAT_SIZE_LIMIT);
		write_text_private(&payload).expect("seed");
		let snapshot = snapshot().expect("snapshot");
		assert!(
			snapshot.is_lossy(),
			"an over-limit format must mark the snapshot lossy"
		);
		assert!(snapshot
			.entries
			.iter()
			.all(|(_, bytes)| bytes.len() <= SNAPSHOT_FORMAT_SIZE_LIMIT));
	}

	#[test]
	#[ignore = "touches the real clipboard"]
	fn snapshot_overwrite_restore_preserves_text() {
		write_text_private("original").expect("seed");
		let snapshot = snapshot().expect("snapshot");
		assert!(!snapshot.is_empty());

		write_text_private("replacement").expect("overwrite");
		assert_eq!(read_text().expect("read").0.as_deref(), Some("replacement"));

		restore(&snapshot, sequence_number()).expect("restore");
		assert_eq!(read_text().expect("read").0.as_deref(), Some("original"));
	}

	#[test]
	#[ignore = "touches the real clipboard"]
	fn a_superseded_restore_refuses_rather_than_clobbering() {
		write_text_private("original").expect("seed");
		let snapshot = snapshot().expect("snapshot");
		let stale = sequence_number();

		write_text_private("what the user copied afterwards").expect("overwrite");

		let err = restore(&snapshot, stale).expect_err("restore must refuse");
		assert!(matches!(err, ClipboardError::Superseded { .. }));
		assert_eq!(
			read_text().expect("read").0.as_deref(),
			Some("what the user copied afterwards"),
			"the newer content must survive"
		);
	}

	/// A minimal 24-bit `BI_RGB` DIB: 40-byte header, 2×2 pixels, no palette.
	fn sample_dib() -> Vec<u8> {
		let mut dib = Vec::new();
		dib.extend_from_slice(&40u32.to_le_bytes());
		dib.extend_from_slice(&2i32.to_le_bytes());
		dib.extend_from_slice(&2i32.to_le_bytes());
		dib.extend_from_slice(&1u16.to_le_bytes());
		dib.extend_from_slice(&24u16.to_le_bytes());
		for _ in 0..6 {
			dib.extend_from_slice(&0u32.to_le_bytes());
		}
		// Two rows of two BGR pixels, each padded to a four-byte boundary.
		dib.extend_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0, 0]);
		dib.extend_from_slice(&[0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0, 0]);
		dib
	}

	fn read_under_session(format: u32) -> Option<Vec<u8>> {
		let _session = Session::open_read().expect("open");
		match read_format_bytes(format, usize::MAX) {
			FormatBytes::Bytes(bytes) => Some(bytes),
			_ => None,
		}
	}

	/// The regression task-005's capture fallback depends on: adding an image
	/// *reader* must not change what the snapshot copies or what the restore puts
	/// back. An image clipboard has to survive the round trip byte for byte, or a
	/// `Ctrl+C` fallback capture silently destroys the user's screenshot.
	#[test]
	#[ignore = "touches the real clipboard"]
	fn a_capture_round_trip_restores_an_image_clipboard_byte_identically() {
		let dib = sample_dib();
		write_excluded(&[(CF_DIB, dib.clone())], None).expect("seed");
		let snapshot = snapshot().expect("snapshot");
		assert!(!snapshot.is_lossy(), "a plain DIB clipboard was reported as lossy");

		write_text_private("what a fallback capture leaves behind").expect("overwrite");
		restore(&snapshot, sequence_number()).expect("restore");

		let restored = read_under_session(CF_DIB).expect("CF_DIB is back");
		// GlobalAlloc rounds the block up, so the restored payload may carry slack
		// past the end; the bitmap itself has to match exactly.
		assert_eq!(&restored[..dib.len()], &dib[..]);
	}

	/// AC4 at the Win32 boundary: text wins, and it wins *before* any image
	/// format is even looked at.
	#[test]
	#[ignore = "touches the real clipboard"]
	fn read_attachment_declines_a_clipboard_that_also_carries_text() {
		let dib = sample_dib();
		let mut wide: Vec<u16> = "a copied code snippet".encode_utf16().collect();
		wide.push(0);
		let text: Vec<u8> = wide.iter().flat_map(|unit| unit.to_ne_bytes()).collect();

		write_excluded(&[(CF_UNICODETEXT, text), (CF_DIB, dib)], None).expect("seed");

		assert!(
			read_attachment().expect("read").is_none(),
			"an image was taken from a clipboard that also carried text"
		);
	}

	/// **Windows synthesizes `CF_DIBV5` from a `CF_DIB`**, so a clipboard seeded
	/// with the older format comes back through the reader as a 124-byte-header
	/// V5 bitmap rather than as the bytes that went in. That is the preference
	/// order working as intended, and it is why this asserts on the *image* the
	/// payload decodes to rather than on the payload itself.
	///
	/// It is also the only place the DIB→PNG conversion meets a header Windows
	/// wrote rather than one this file built, which is what makes it worth the
	/// round trip: a `bfOffBits` that double-counted a V5 header's channel masks
	/// would decode the masks as pixels and pass every hand-built fixture.
	#[test]
	#[ignore = "touches the real clipboard"]
	fn read_attachment_returns_a_decodable_bitmap_when_there_is_no_text() {
		write_excluded(&[(CF_DIB, sample_dib())], None).expect("seed");

		let Some(ClipboardAttachment::Dib(bytes)) = read_attachment().expect("read") else {
			panic!("expected a bitmap");
		};

		let png = crate::attachments::thumb::dib_to_png(&bytes).expect("the payload must decode");
		assert_eq!(
			crate::attachments::thumb::dimensions(&png, "image/png"),
			(Some(2), Some(2))
		);
	}

	/// Task-001 acceptance criterion 7.1 treats this as a hard pass/fail and it
	/// cannot be automated: run it, then press Win+V.
	#[test]
	#[ignore = "manual: run, then check Win+V shows neither entry"]
	fn win_v_shows_neither_a_copper_write_nor_a_copper_restore() {
		write_text_private("copper write — must not appear in Win+V").expect("write");
		let snapshot = snapshot().expect("snapshot");
		write_text_private("copper overwrite").expect("overwrite");
		restore(&snapshot, sequence_number()).expect("restore");
		println!("Press Win+V now. Neither Copper entry may appear in the history.");
	}
}
