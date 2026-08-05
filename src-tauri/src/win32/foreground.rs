//! Who is in front, as an identity that can be compared later.
//!
//! The pair matters, not the handle: the capture pipeline revalidates its target
//! before each strategy, immediately before `SendInput`, and after reading but
//! before persisting (task-005 R21). A window handle alone is not enough for
//! that — handles are recycled, and the same handle under a new process is
//! exactly the mis-capture the revalidation exists to catch.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// A foreground window and the process that owns it.
///
/// `Copy` and comparable so revalidation is `target == Target::current()` rather
/// than a hand-written comparison at each of the three sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
	pub hwnd: HWND,
	pub pid: u32,
}

impl Target {
	/// Samples the current foreground window, or `None` when there is none or
	/// its process cannot be identified.
	pub fn current() -> Option<Self> {
		let hwnd = foreground_hwnd()?;
		let pid = pid_of_window(hwnd)?;
		Some(Self { hwnd, pid })
	}

	/// Whether the foreground is still this exact window of this exact process.
	pub fn still_current(&self) -> bool {
		Self::current().is_some_and(|now| now == *self)
	}
}

// SAFETY: an HWND is a plain kernel-side identifier, not a pointer into this
// process, and every use of it here is a call that Windows itself marshals. The
// capture pipeline samples the target on the worker thread and compares it
// there; no window operation is performed from it (task-005 R19 keeps those on
// the main thread).
unsafe impl Send for Target {}

fn foreground_hwnd() -> Option<HWND> {
	// SAFETY: no preconditions; returns null when there is no foreground window.
	let hwnd = unsafe { GetForegroundWindow() };
	(!hwnd.is_invalid()).then_some(hwnd)
}

/// Copper's own process id, for recognising that Copper itself is in front.
pub fn our_pid() -> u32 {
	// SAFETY: no preconditions.
	unsafe { GetCurrentProcessId() }
}

/// The process owning a window.
pub fn pid_of_window(hwnd: HWND) -> Option<u32> {
	if hwnd.is_invalid() {
		return None;
	}
	let mut pid = 0u32;
	// SAFETY: `hwnd` is non-null and `pid` is a live local for the call.
	unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
	(pid != 0).then_some(pid)
}
