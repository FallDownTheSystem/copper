//! Where Copper says something went wrong.
//!
//! A release build is a windows-subsystem process (see `main.rs`), so it has no
//! console attached: `println!` and `eprintln!` write to handles that go
//! nowhere. That matters more here than in most apps, because Copper starts
//! hidden by design — a failed launch and a successful one look identical from
//! the outside, and there is no window to carry the bad news.
//!
//! Two sinks, both chosen to need no dependency beyond the `windows` crate we
//! already pull in:
//!
//! - `OutputDebugStringW` for diagnostics. Visible in DebugView or any attached
//!   debugger, and a no-op with nothing listening.
//! - `MessageBoxW` for a panic. It is the only one of these that reaches a user
//!   who is not looking for it.
//!
//! Deliberately rejected: `AttachConsole`, which fails when launched from
//! Explorer; `AllocConsole`, whose console can vanish before it is read; and a
//! log file beside the executable, since the install directory may be
//! read-only. Durable logging under `app_log_dir()` is deferred to a later task.

/// Reports something noteworthy that is not a failure.
pub fn log(msg: &str) {
	#[cfg(debug_assertions)]
	println!("{msg}");
	#[cfg(not(debug_assertions))]
	emit_debug_string(msg);
}

/// Reports a failure that could not be propagated to a caller.
pub fn log_error(msg: &str) {
	#[cfg(debug_assertions)]
	eprintln!("{msg}");
	#[cfg(not(debug_assertions))]
	emit_debug_string(msg);
}

#[cfg(not(debug_assertions))]
fn emit_debug_string(msg: &str) {
	use windows::core::HSTRING;
	use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;

	// DebugView splits on newlines, so send one terminated line per call.
	let line = HSTRING::from(format!("{msg}\n"));
	// SAFETY: `line` is a NUL-terminated wide string that outlives the call.
	unsafe { OutputDebugStringW(&line) };
}

/// Makes a release-build panic visible instead of silent.
///
/// Returning an error from Tauri's `setup()` closure does not save us: Tauri
/// converts it into a panic (`tauri-2.11.5/src/app.rs:1424-1425, 1476-1477`), so
/// a failed `panel::apply_effects` or `tray::build` would otherwise terminate
/// the process with no window, no tray and nothing written anywhere — which for
/// an app that deliberately starts hidden is indistinguishable from a normal
/// launch. The hook runs before the abort, so it still fires under
/// `panic = "abort"`.
///
/// Debug builds keep the default hook, which prints a backtrace to the console
/// they actually have.
#[cfg(not(debug_assertions))]
pub fn install_panic_dialog() {
	std::panic::set_hook(Box::new(|info| {
		let details = info.to_string();
		emit_debug_string(&details);
		show_error_dialog(&format!(
			"Copper could not start.\n\n{details}\n\nThis is a bug. The panel and \
			 the tray icon are both unavailable for this session."
		));
	}));
}

#[cfg(debug_assertions)]
pub fn install_panic_dialog() {}

#[cfg(not(debug_assertions))]
fn show_error_dialog(msg: &str) {
	use windows::core::HSTRING;
	use windows::Win32::UI::WindowsAndMessaging::{
		MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TASKMODAL,
	};

	let text = HSTRING::from(msg);
	let caption = HSTRING::from("Copper");
	// SAFETY: both strings are NUL-terminated and outlive the call. MB_TASKMODAL
	// is used rather than an owner handle because the panel window may not exist
	// yet — a startup panic is the case this exists for.
	unsafe {
		MessageBoxW(
			None,
			&text,
			&caption,
			MB_OK | MB_ICONERROR | MB_TASKMODAL | MB_SETFOREGROUND,
		)
	};
}
