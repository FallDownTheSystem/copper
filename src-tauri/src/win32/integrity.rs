//! Why a capture that produced nothing produced nothing — the part of the answer
//! that lives in process tokens.
//!
//! Two separate questions, deliberately not collapsed into one boolean:
//!
//! - **Is the target above us?** [`target_integrity`], a tri-state.
//! - **Are we allowed above us anyway?** [`uiaccess_active`], our own token's
//!   UIAccess flag.
//!
//! Only both together justify saying "Copper can't read from apps running as
//! administrator". No shipped build carries `uiAccess="true"` — the flag lifts
//! the host's integrity above WebView2's browser process, whose `SetParent`
//! then fails and the app cannot boot (measured 2026-08-08; upstream
//! WebView2Feedback#4884, closed "not planned") — but the check stays paired
//! because the flag is a property of the token, not of our build plans, and a
//! target we could not read the token of proves nothing either, which
//! is why the tri-state has an `Unknown` arm instead of folding denial into
//! "elevated". Claiming a process is running as administrator when its token was
//! never read would be a confident lie on the one surface that ever speaks to
//! the user.
//!
//! Both are called **only after the cascade has already failed**, so the success
//! path pays nothing for either.

use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
	GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
	TokenUIAccess, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
	GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Medium integrity, from `winnt.h`. The assumed value for our own process when
/// even our own token will not read — every desktop app runs at least here, so
/// guessing it makes the comparison conservative rather than alarmist.
const SECURITY_MANDATORY_MEDIUM_RID: u32 = 0x2000;

/// How the target's integrity compares with ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetIntegrity {
	/// The target's RID was read and is above ours.
	Higher,
	/// The target's RID was read and is at or below ours.
	NotHigher,
	/// The comparison could not be made: `OpenProcess` or `OpenProcessToken` was
	/// denied. Task-001 measured this against 151 processes — `audiodg.exe` and
	/// three `Discord.exe` instances refuse the token read while running at
	/// *medium* integrity. They are hardened, not elevated, so this must never
	/// reach the administrator wording.
	#[default]
	Unknown,
}

/// A `HANDLE` that closes itself. Manual pairing is how handle leaks happen.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
	fn drop(&mut self) {
		if !self.0.is_invalid() {
			// SAFETY: the handle came from OpenProcess/OpenProcessToken and is
			// closed exactly once, here.
			unsafe {
				let _ = CloseHandle(self.0);
			}
		}
	}
}

/// Whether our own process token has UIAccess active.
///
/// True only for a binary that is Authenticode-signed, installed under a trusted
/// location, and manifested `uiAccess="true"` — which is a release-build
/// property. A dev build always answers false, which is correct rather than a
/// limitation: it genuinely cannot read elevated windows.
///
/// Computed once. The flag cannot change during a process's lifetime.
pub fn uiaccess_active() -> bool {
	static ACTIVE: OnceLock<bool> = OnceLock::new();
	*ACTIVE.get_or_init(|| read_uiaccess().unwrap_or(false))
}

fn read_uiaccess() -> Option<bool> {
	// SAFETY: GetCurrentProcess returns a pseudo-handle needing no close, and
	// `token` is written only on success.
	let mut token = HANDLE::default();
	unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
	let token = OwnedHandle(token);

	let mut ui_access = 0u32;
	let mut returned = 0u32;
	// SAFETY: TokenUIAccess returns a DWORD, and the buffer passed is exactly one.
	unsafe {
		GetTokenInformation(
			token.0,
			TokenUIAccess,
			Some(std::ptr::addr_of_mut!(ui_access).cast()),
			std::mem::size_of::<u32>() as u32,
			&mut returned,
		)
	}
	.ok()?;

	Some(ui_access != 0)
}

/// How a process's integrity compares with ours.
pub fn target_integrity(pid: u32) -> TargetIntegrity {
	// PROCESS_QUERY_LIMITED_INFORMATION rather than PROCESS_QUERY_INFORMATION:
	// task-001 measured that the fuller right helps in exactly none of the
	// failing cases, because OpenProcess itself is what gets denied for those.
	// SAFETY: no preconditions; failure is reported through the Result.
	let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
		Ok(handle) => OwnedHandle(handle),
		Err(_) => return TargetIntegrity::Unknown,
	};

	match integrity_rid(handle.0) {
		Some(theirs) => {
			let ours = our_integrity().unwrap_or(SECURITY_MANDATORY_MEDIUM_RID);
			if theirs > ours {
				TargetIntegrity::Higher
			} else {
				TargetIntegrity::NotHigher
			}
		}
		None => TargetIntegrity::Unknown,
	}
}

/// Our own integrity RID, computed once.
fn our_integrity() -> Option<u32> {
	static OURS: OnceLock<Option<u32>> = OnceLock::new();
	// SAFETY: a pseudo-handle valid for the current process, needing no close.
	*OURS.get_or_init(|| integrity_rid(unsafe { GetCurrentProcess() }))
}

/// `OpenProcessToken` → `GetTokenInformation(TokenIntegrityLevel)` → the last
/// sub-authority of the SID in the returned `TOKEN_MANDATORY_LABEL`.
fn integrity_rid(process: HANDLE) -> Option<u32> {
	let mut token = HANDLE::default();
	// SAFETY: `token` is written only on success.
	unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
	let token = OwnedHandle(token);

	// The first call sizes the buffer and is expected to fail with
	// ERROR_INSUFFICIENT_BUFFER, so its error is deliberately dropped.
	let mut needed = 0u32;
	// SAFETY: a null buffer with length 0 is the documented sizing call.
	let _ = unsafe { GetTokenInformation(token.0, TokenIntegrityLevel, None, 0, &mut needed) };
	if needed == 0 {
		return None;
	}

	// Backed by u64s, not u8s. The returned TOKEN_MANDATORY_LABEL is read through
	// a reference, so the buffer has to satisfy its alignment — a Vec<u8> only
	// does by accident of what the allocator happened to return, which task-001's
	// review flagged as worth fixing when this moved into the product.
	let words = (needed as usize).div_ceil(std::mem::size_of::<u64>());
	let mut buf = vec![0u64; words];
	// SAFETY: the buffer is `words * 8 >= needed` bytes and suitably aligned.
	unsafe {
		GetTokenInformation(
			token.0,
			TokenIntegrityLevel,
			Some(buf.as_mut_ptr().cast()),
			needed,
			&mut needed,
		)
	}
	.ok()?;

	// SAFETY: on success the buffer holds a TOKEN_MANDATORY_LABEL whose Label.Sid
	// points into that same buffer, which outlives this block.
	unsafe {
		let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
		let sid = label.Label.Sid;
		if sid.is_invalid() {
			return None;
		}
		let count_ptr = GetSidSubAuthorityCount(sid);
		if count_ptr.is_null() || *count_ptr == 0 {
			return None;
		}
		let rid_ptr = GetSidSubAuthority(sid, (*count_ptr - 1) as u32);
		if rid_ptr.is_null() {
			return None;
		}
		Some(*rid_ptr)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn our_own_integrity_reads() {
		// Every process can read its own token, so a None here means the SID walk
		// above is wrong rather than that the machine is unusual.
		assert!(our_integrity().is_some());
	}

	#[test]
	fn our_own_process_is_not_higher_than_itself() {
		let pid = std::process::id();
		assert_eq!(target_integrity(pid), TargetIntegrity::NotHigher);
	}

	#[test]
	fn a_process_that_does_not_exist_is_unknown_not_higher() {
		// pid 0 is the System Idle Process and cannot be opened. The point is the
		// mapping: denial must never present as "running as administrator".
		assert_eq!(target_integrity(0), TargetIntegrity::Unknown);
	}

	#[test]
	fn uiaccess_is_inactive_in_a_test_binary() {
		// Unsigned, not installed under a trusted path. If this ever fails the
		// probe is reading the wrong token field, since no test runner has UIAccess.
		assert!(!uiaccess_active());
	}
}
