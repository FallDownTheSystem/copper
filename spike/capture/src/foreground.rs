//! Foreground window identity: HWND, PID, title, process name, integrity level.
//!
//! `pid` is not decoration. Both the clipboard owner-mismatch check and the UIA
//! foreign-element check compare against it, and neither can be written without
//! it.

use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE, HWND};
use windows::Win32::Security::{
    GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
};

/// Well-known integrity-level RIDs, from `winnt.h`.
pub const SECURITY_MANDATORY_LOW_RID: u32 = 0x1000;
pub const SECURITY_MANDATORY_MEDIUM_RID: u32 = 0x2000;
pub const SECURITY_MANDATORY_HIGH_RID: u32 = 0x3000;
pub const SECURITY_MANDATORY_SYSTEM_RID: u32 = 0x4000;

/// What we managed to learn about a process's integrity level.
///
/// The three states are kept apart deliberately. `Foreground::elevated` gates
/// the entire cascade — a false positive short-circuits every capture to
/// `ForegroundElevated` — so "we could not find out" must not be silently
/// folded into "it is elevated".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Integrity {
    /// The RID was read from the process token.
    Level(u32),
    /// `OpenProcess` itself was denied. Per dsgn-001 this counts as elevated:
    /// a process we cannot even open a limited handle to is one we certainly
    /// cannot read UI Automation from.
    ProcessInaccessible,
    /// The process opened but its **token** did not.
    ///
    /// Measured on this machine (see `examples/integrity-probe.rs`): 147 of 151
    /// processes read fine with `PROCESS_QUERY_LIMITED_INFORMATION`, and the
    /// four that failed — `audiodg.exe` and three `Discord.exe` instances —
    /// returned `ERROR_ACCESS_DENIED` from `OpenProcessToken` despite running
    /// at *medium* integrity. They are hardened, not elevated. Treating this as
    /// elevated would disable capture for ordinary applications, so the cascade
    /// proceeds and lets UIPI give the real answer.
    TokenUnreadable,
}

impl Integrity {
    pub fn rid(self) -> Option<u32> {
        match self {
            Integrity::Level(rid) => Some(rid),
            _ => None,
        }
    }
}

/// A `HANDLE` that closes itself. Manual pairing is how handle leaks happen.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Foreground {
    pub hwnd: HWND,
    pub pid: u32,
    pub title: String,
    pub process: String,
    /// The foreground process runs at a higher integrity level than we do, so
    /// UIPI blocks both UI Automation reads and `SendInput` against it.
    pub elevated: bool,
    /// Kept for the findings so `elevated` can be interpreted rather than
    /// merely believed.
    pub integrity: Integrity,
}

impl Foreground {
    /// Sample the current foreground window.
    ///
    /// `None` is the `NoForegroundWindow` outcome. It is deliberately *not*
    /// producible by `attempt(fg: &Foreground)`, whose signature already
    /// presupposes a foreground window — the worker records it instead.
    pub fn current() -> Option<Foreground> {
        let hwnd = foreground_hwnd()?;
        let pid = pid_of_window(hwnd)?;

        let title = window_title(hwnd);
        let (process, integrity) = process_identity(pid);
        let elevated = out_of_reach(
            integrity,
            our_integrity().unwrap_or(SECURITY_MANDATORY_MEDIUM_RID),
        );

        Some(Foreground {
            hwnd,
            pid,
            title,
            process,
            elevated,
            integrity,
        })
    }

    /// Human-readable integrity, for logs and findings.
    pub fn integrity_label(&self) -> &'static str {
        match self.integrity {
            Integrity::ProcessInaccessible => "process-inaccessible",
            Integrity::TokenUnreadable => "token-unreadable",
            Integrity::Level(rid) if rid >= SECURITY_MANDATORY_SYSTEM_RID => "system",
            Integrity::Level(rid) if rid >= SECURITY_MANDATORY_HIGH_RID => "high",
            Integrity::Level(rid) if rid >= SECURITY_MANDATORY_MEDIUM_RID => "medium",
            Integrity::Level(rid) if rid >= SECURITY_MANDATORY_LOW_RID => "low",
            Integrity::Level(_) => "untrusted",
        }
    }
}

/// Whether a target is out of reach under UIPI, given our own integrity.
///
/// Pure, and separated out because it gates the entire cascade: a `true` here
/// short-circuits the attempt to `ForegroundElevated` and nothing else runs.
fn out_of_reach(target: Integrity, ours: u32) -> bool {
    match target {
        Integrity::Level(level) => level > ours,
        Integrity::ProcessInaccessible => true,
        // Opaque, not elevated. See the note on the variant.
        Integrity::TokenUnreadable => false,
    }
}

/// The current foreground `HWND`, for revalidating that focus has not moved
/// between the trigger and the injection.
pub fn foreground_hwnd() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.is_invalid()).then_some(hwnd)
}

/// The process owning a window, for the clipboard owner-mismatch check.
pub fn pid_of_window(hwnd: HWND) -> Option<u32> {
    if hwnd.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

/// Executable file name and integrity for a pid.
///
/// `PROCESS_QUERY_LIMITED_INFORMATION` is sufficient for `OpenProcessToken`;
/// verified on this machine against 151 processes with
/// `examples/integrity-probe.rs`, where the fuller `PROCESS_QUERY_INFORMATION`
/// helped in exactly none of the failing cases (`OpenProcess` was denied
/// outright for those).
fn process_identity(pid: u32) -> (String, Integrity) {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) };
    let handle = match handle {
        Ok(h) => OwnedHandle(h),
        Err(e) => {
            let label = if e.code() == ERROR_ACCESS_DENIED.to_hresult() {
                "<access denied>"
            } else {
                "<unknown>"
            };
            return (label.to_owned(), Integrity::ProcessInaccessible);
        }
    };

    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    let name = unsafe {
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    let full = match name {
        Ok(()) => String::from_utf16_lossy(&buf[..len as usize]),
        Err(_) => String::new(),
    };
    let file_name = full
        .rsplit(['\\', '/'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("<unknown>")
        .to_owned();

    let integrity = match integrity_of(handle.0) {
        Some(rid) => Integrity::Level(rid),
        None => Integrity::TokenUnreadable,
    };
    (file_name, integrity)
}

/// Integrity RID from a process handle:
/// `OpenProcessToken(TOKEN_QUERY)` → `GetTokenInformation(TokenIntegrityLevel)`
/// → last sub-authority of the SID in `TOKEN_MANDATORY_LABEL`.
fn integrity_of(process: HANDLE) -> Option<u32> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    let token = OwnedHandle(token);

    // First call sizes the buffer; it is expected to fail with
    // ERROR_INSUFFICIENT_BUFFER, so its error is deliberately ignored.
    let mut needed = 0u32;
    let _ = unsafe { GetTokenInformation(token.0, TokenIntegrityLevel, None, 0, &mut needed) };
    if needed == 0 {
        return None;
    }

    let mut buf = vec![0u8; needed as usize];
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
        let count_ptr = windows::Win32::Security::GetSidSubAuthorityCount(sid);
        if count_ptr.is_null() {
            return None;
        }
        let count = *count_ptr;
        if count == 0 {
            return None;
        }
        let rid_ptr = windows::Win32::Security::GetSidSubAuthority(sid, (count - 1) as u32);
        if rid_ptr.is_null() {
            return None;
        }
        Some(*rid_ptr)
    }
}

/// Our own integrity RID, computed once.
pub fn our_integrity() -> Option<u32> {
    static OURS: OnceLock<Option<u32>> = OnceLock::new();
    *OURS.get_or_init(|| integrity_of(unsafe { GetCurrentProcess() }))
}

pub fn our_pid() -> u32 {
    unsafe { GetCurrentProcessId() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEDIUM: u32 = SECURITY_MANDATORY_MEDIUM_RID;

    #[test]
    fn a_higher_integrity_target_is_out_of_reach() {
        assert!(out_of_reach(
            Integrity::Level(SECURITY_MANDATORY_HIGH_RID),
            MEDIUM
        ));
        assert!(out_of_reach(
            Integrity::Level(SECURITY_MANDATORY_SYSTEM_RID),
            MEDIUM
        ));
    }

    #[test]
    fn equal_or_lower_integrity_is_reachable() {
        assert!(!out_of_reach(Integrity::Level(MEDIUM), MEDIUM));
        assert!(!out_of_reach(
            Integrity::Level(SECURITY_MANDATORY_LOW_RID),
            MEDIUM
        ));
    }

    #[test]
    fn a_process_we_cannot_open_counts_as_out_of_reach() {
        assert!(out_of_reach(Integrity::ProcessInaccessible, MEDIUM));
    }

    #[test]
    fn an_opaque_token_does_not_count_as_elevated() {
        // Discord and audiodg.exe run at medium integrity but refuse
        // OpenProcessToken. Folding that into "elevated" would short-circuit
        // the cascade for ordinary applications and silently disable capture
        // for them — the exact bug this separation exists to prevent.
        assert!(!out_of_reach(Integrity::TokenUnreadable, MEDIUM));
    }

    #[test]
    fn an_elevated_copper_can_reach_a_high_integrity_target() {
        // Relevant to the uiAccess path and to anyone running the spike
        // elevated: the comparison is relative, not absolute.
        assert!(!out_of_reach(
            Integrity::Level(SECURITY_MANDATORY_HIGH_RID),
            SECURITY_MANDATORY_HIGH_RID
        ));
    }

    #[test]
    fn integrity_rid_is_only_available_for_a_real_level() {
        assert_eq!(Integrity::Level(0x2000).rid(), Some(0x2000));
        assert_eq!(Integrity::TokenUnreadable.rid(), None);
        assert_eq!(Integrity::ProcessInaccessible.rid(), None);
    }
}
