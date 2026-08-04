//! Diagnose exactly where integrity-level detection fails, and for which
//! processes.
//!
//! `Foreground::elevated` gates the whole cascade: a false positive
//! short-circuits every capture to `ForegroundElevated`. So the failure modes
//! need to be visible rather than inferred, with real error codes attached.
//!
//! ```text
//! cargo run --example integrity-probe
//! ```

use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows::Win32::Security::{
    GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_ACCESS_RIGHTS,
    PROCESS_NAME_WIN32, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
};

fn probe(pid: u32, rights: PROCESS_ACCESS_RIGHTS, label: &str) -> String {
    let handle = match unsafe { OpenProcess(rights, false, pid) } {
        Ok(h) => h,
        Err(e) => return format!("{label}: OpenProcess failed 0x{:08X}", e.code().0),
    };

    let mut name_buf = [0u16; 512];
    let mut len = name_buf.len() as u32;
    let name = match unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(name_buf.as_mut_ptr()),
            &mut len,
        )
    } {
        Ok(()) => String::from_utf16_lossy(&name_buf[..len as usize])
            .rsplit('\\')
            .next()
            .unwrap_or("?")
            .to_owned(),
        Err(e) => format!("<name failed 0x{:08X}>", e.code().0),
    };

    let mut token = HANDLE::default();
    let token_result = unsafe { OpenProcessToken(handle, TOKEN_QUERY, &mut token) };
    let outcome = match token_result {
        Err(e) => format!(
            "OpenProcessToken FAILED 0x{:08X} (last error {})",
            e.code().0,
            unsafe { GetLastError() }.0
        ),
        Ok(()) => {
            let mut needed = 0u32;
            let _ =
                unsafe { GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed) };
            if needed == 0 {
                format!("sizing call returned 0 (last error {})", unsafe {
                    GetLastError()
                }
                .0)
            } else {
                let mut buf = vec![0u8; needed as usize];
                match unsafe {
                    GetTokenInformation(
                        token,
                        TokenIntegrityLevel,
                        Some(buf.as_mut_ptr().cast()),
                        needed,
                        &mut needed,
                    )
                } {
                    Err(e) => format!("GetTokenInformation FAILED 0x{:08X}", e.code().0),
                    Ok(()) => unsafe {
                        let tml = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
                        let sid = tml.Label.Sid;
                        let count = *windows::Win32::Security::GetSidSubAuthorityCount(sid);
                        let rid = *windows::Win32::Security::GetSidSubAuthority(
                            sid,
                            (count - 1) as u32,
                        );
                        format!("RID 0x{rid:04X}")
                    },
                }
            }
        }
    };

    unsafe {
        if !token.is_invalid() {
            let _ = CloseHandle(token);
        }
        let _ = CloseHandle(handle);
    }
    format!("{name:<24} {label:<10} {outcome}")
}

fn main() {
    // Walk a spread of pids rather than enumerating: this only needs enough
    // samples to tell "one process is special" from "the access right is wrong".
    let mut pids: Vec<u32> = Vec::new();
    if let Some(arg) = std::env::args().nth(1) {
        pids.push(arg.parse().expect("pid"));
    } else {
        let ours = std::process::id();
        pids.push(ours);
        // Even pids only; Windows pids are multiples of 4.
        pids.extend((1..=6000u32).map(|n| n * 4).filter(|p| *p != ours));
    }

    println!("{:<24} {:<10} TokenIntegrityLevel outcome", "process", "rights");
    println!("{}", "-".repeat(88));

    let mut shown = 0;
    let mut limited_ok = 0;
    let mut limited_fail = 0;

    for pid in pids {
        let limited = probe(pid, PROCESS_QUERY_LIMITED_INFORMATION, "LIMITED");
        if limited.contains("OpenProcess failed") {
            continue;
        }
        if limited.contains("RID 0x") {
            limited_ok += 1;
        } else {
            limited_fail += 1;
            // Only interesting when LIMITED failed: does the fuller right help?
            let full = probe(pid, PROCESS_QUERY_INFORMATION, "FULL");
            println!("{limited}");
            println!("{full}");
            shown += 1;
        }
        if shown >= 12 {
            break;
        }
    }

    println!("\n-- summary --");
    println!("  PROCESS_QUERY_LIMITED_INFORMATION read the integrity level : {limited_ok}");
    println!("  ... and failed to                                          : {limited_fail}");
    if limited_fail == 0 {
        println!(
            "\n  PROCESS_QUERY_LIMITED_INFORMATION is sufficient for OpenProcessToken here.\n  \
             A None result therefore means the process is genuinely out of reach."
        );
    }
}
