//! Acceptance criterion 12: does a signed `uiAccess="true"` binary in
//! `%ProgramFiles%` actually read UI Automation text out of an **elevated**
//! window?
//!
//! This gates dsgn-001 Phase 4's per-machine install + admin-elevation strategy.
//! If it fails, uiAccess cannot ship and Phase 4 must revert to the unsigned
//! approach, which means blocking elevated windows outright.
//!
//! Run it, then within the countdown focus an elevated Windows Terminal and
//! select some text.
//!
//! The tool reports its own token state first, because the result is only
//! interpretable alongside it: a run that succeeds *because it was elevated*
//! proves nothing about uiAccess.

use std::time::Duration;

use capture::foreground::Foreground;
use capture::uia::{UiaOutcome, UiaService};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenUIAccess, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Read a DWORD-shaped token information class from our own token.
fn token_dword(class: windows::Win32::Security::TOKEN_INFORMATION_CLASS) -> Option<u32> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
    let mut value = 0u32;
    let mut returned = 0u32;
    let result = unsafe {
        GetTokenInformation(
            token,
            class,
            Some((&mut value as *mut u32).cast()),
            std::mem::size_of::<u32>() as u32,
            &mut returned,
        )
    };
    unsafe {
        let _ = CloseHandle(token);
    }
    result.ok().map(|()| value)
}

/// `TOKEN_ELEVATION` is a single `DWORD`, so this is the DWORD reader above.
fn is_elevated() -> Option<bool> {
    token_dword(TokenElevation).map(|v| v != 0)
}

fn in_secure_location(exe: &std::path::Path) -> bool {
    let secure = [
        std::env::var("ProgramFiles").ok(),
        std::env::var("ProgramFiles(x86)").ok(),
        std::env::var("SystemRoot").ok().map(|r| format!("{r}\\System32")),
    ];
    let exe = exe.to_string_lossy().to_ascii_lowercase();
    secure
        .into_iter()
        .flatten()
        .any(|root| exe.starts_with(&root.to_ascii_lowercase()))
}

fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let wait_secs: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(10);

    let exe = std::env::current_exe().unwrap_or_default();
    let ui_access = token_dword(TokenUIAccess);
    let elevated = is_elevated();
    let integrity = capture::foreground::our_integrity();
    let secure_location = in_secure_location(&exe);

    println!("== uiAccess validation (acceptance criterion 12) ==");
    println!("  executable        : {}", exe.display());
    println!(
        "  secure location   : {}  (required: %ProgramFiles%, %ProgramFiles(x86)% or System32)",
        if secure_location { "YES" } else { "NO" }
    );
    println!(
        "  token UIAccess    : {}",
        match ui_access {
            Some(1) => "YES - uiAccess is ACTIVE".to_owned(),
            Some(0) => "no - uiAccess is INACTIVE".to_owned(),
            Some(other) => format!("{other}"),
            None => "could not read the token".to_owned(),
        }
    );
    println!(
        "  token elevated    : {}",
        match elevated {
            Some(true) => "YES - note this confounds the test; see below",
            Some(false) => "no (correct for a uiAccess test)",
            None => "unknown",
        }
    );
    println!("  our integrity RID : {integrity:?}");

    if ui_access != Some(1) {
        println!(
            "\n  uiAccess is NOT active. The three requirements are: an Authenticode signature from a\n  \
             certificate in Trusted Root AND Trusted Publishers, a secure install location, and UAC\n  \
             enabled. Run spike\\scripts\\uiaccess-setup.ps1 to satisfy the first two."
        );
    }
    if elevated == Some(true) {
        println!(
            "\n  WARNING: this process is elevated. A successful read below would be explained by the\n  \
             elevation alone and would say nothing about uiAccess. Re-run it WITHOUT elevation."
        );
    }

    println!("\nFocus an ELEVATED window (e.g. Windows Terminal run as administrator) and select some");
    println!("text. Reading in {wait_secs} seconds...");
    for remaining in (1..=wait_secs).rev() {
        println!("  {remaining}...");
        std::thread::sleep(Duration::from_secs(1));
    }

    let Some(fg) = Foreground::current() else {
        println!("\nNo foreground window. Cannot test.");
        return std::process::ExitCode::FAILURE;
    };

    println!("\n  foreground        : {} (pid {})", fg.process, fg.pid);
    println!("  title             : {}", fg.title);
    println!(
        "  target integrity  : {} (RID {:?})",
        fg.integrity_label(),
        fg.integrity
    );
    println!("  higher than ours  : {}", fg.elevated);

    if !fg.elevated {
        println!(
            "\n  NOTE: the foreground window is not higher-integrity than this process, so this run\n  \
             does not exercise UIPI at all. Launch Windows Terminal as administrator and retry."
        );
    }

    // Deliberately bypass the cascade's `ForegroundElevated` short-circuit and
    // read anyway. That short-circuit is correct when uiAccess is inactive, but
    // it is exactly the thing uiAccess is supposed to make unnecessary — so
    // gating this test on it would make the test unable to succeed.
    let mut uia = UiaService::new();
    let outcome = uia.read(fg.hwnd, fg.pid, Duration::from_millis(2000));

    println!("\n== result ==");
    let verdict = match &outcome {
        UiaOutcome::Text { text, range_count } => {
            println!("  READ SUCCEEDED: {} characters in {range_count} range(s)", text.chars().count());
            println!("  preview: {}", text.chars().take(120).collect::<String>());
            true
        }
        other => {
            println!("  read did not return text: {other:?}");
            false
        }
    };
    println!("  uia init ms       : {:?}", uia.init_ms);

    println!("\n== verdict ==");
    match (ui_access == Some(1), fg.elevated, verdict) {
        (true, true, true) => println!(
            "  PASS - a signed uiAccess binary read text from a higher-integrity window.\n  \
             dsgn-001 Phase 4's per-machine install + Authenticode strategy is viable."
        ),
        (true, true, false) => println!(
            "  FAIL - uiAccess is active but the read still returned no text.\n  \
             Criterion 12 fails: Phase 4 must revert to unsigned/per-user and block elevated windows."
        ),
        (true, false, _) => println!("  INCONCLUSIVE - uiAccess is active but the target was not elevated."),
        (false, _, _) => println!("  INCONCLUSIVE - uiAccess is not active; sign and install first."),
    }

    if verdict {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
