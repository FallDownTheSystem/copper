//! Measure the hook callback's own duration against the `LowLevelHooksTimeout`
//! budget. Acceptance criterion 3.
//!
//! Windows silently removes a low-level hook whose callback exceeds
//! `LowLevelHooksTimeout`, with **no way for the application to detect it**.
//! That is the single hardest constraint on the callback's design, so the
//! margin has to be measured rather than assumed.
//!
//! Events are synthesized for virtual key **0xE8, which is unassigned** — no
//! application binds it, so a few hundred of them are inert. (`win-hotkeys`
//! ships the same key as its "silent key" for exactly this reason.) The trigger
//! is set to 0xE8 as well, so the fire path — the channel send — is measured
//! too. No capture cascade runs: this example never calls `attempt`, so nothing
//! touches the clipboard or any target application.
//!
//! ```text
//! cargo run --release --example hook-latency
//! ```
//!
//! Run it with `--release` for a figure representative of a shipped build; the
//! debug figure is also worth recording as a pessimistic bound.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use capture::hook::{self, DoubleTapConfig, TriggerKey};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY,
};

/// Unassigned virtual key. Chosen precisely because nothing reacts to it.
const VK_UNASSIGNED: u16 = 0xE8;

fn event(up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_UNASSIGNED),
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                // Deliberately NOT COPPER_INJECTED_TAG: tagged events are
                // filtered out early and would skip most of the callback.
                dwExtraInfo: 0,
            },
        },
    }
}

fn main() {
    let pairs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(500);

    let hooks_timeout = hook::low_level_hooks_timeout();
    println!("== hook callback latency (acceptance criterion 3) ==");
    println!(
        "  build profile        : {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!(
        "  LowLevelHooksTimeout : {}",
        hooks_timeout
            .map(|v| format!("{v} ms (explicitly set)"))
            .unwrap_or_else(|| "unset - system default, capped at 1000 ms on Win10 1709+".into())
    );

    let (tx, rx) = mpsc::channel();
    let handle = hook::install(TriggerKey::plain(VK_UNASSIGNED as u32), DoubleTapConfig::default(), tx)
        .expect("could not install the hook");

    let drained = thread::spawn(move || {
        let mut count = 0usize;
        while rx.recv_timeout(Duration::from_millis(500)).is_ok() {
            count += 1;
        }
        count
    });

    println!("\n  injecting {} down/up pairs of vk 0x{VK_UNASSIGNED:02X} (unassigned)...", pairs * 2);
    let before = hook::CALLBACK_COUNT.load(Ordering::Relaxed);

    for _ in 0..pairs {
        // down, up, down, up — a complete double-tap, so the fire path runs.
        let batch = [event(false), event(true), event(false), event(true)];
        unsafe { SendInput(&batch, std::mem::size_of::<INPUT>() as i32) };
        // Let the hook drain rather than flooding the input queue.
        thread::sleep(Duration::from_millis(1));
    }
    thread::sleep(Duration::from_millis(300));

    let observed = hook::CALLBACK_COUNT.load(Ordering::Relaxed) - before;
    let max_ns = hook::MAX_CALLBACK_NS.load(Ordering::Relaxed);
    let mean_ns = hook::mean_callback_ns();

    drop(handle);
    let triggers = drained.join().unwrap_or(0);

    println!("\n== results ==");
    println!("  key events through the callback : {observed}");
    println!("  triggers fired                  : {triggers}");
    println!("  MAX callback duration           : {:.4} ms", max_ns as f64 / 1e6);
    println!("  mean callback duration          : {:.4} ms", mean_ns as f64 / 1e6);

    let budget_ms = hooks_timeout.unwrap_or(1000) as f64;
    let max_ms = max_ns as f64 / 1e6;
    println!(
        "  margin against the budget       : {:.1} ms  ({:.0}x headroom)",
        budget_ms - max_ms,
        if max_ms > 0.0 { budget_ms / max_ms } else { f64::INFINITY }
    );
    println!(
        "\n  Performance criterion is 'under 1 ms per key event': {}",
        if max_ms < 1.0 { "PASS" } else { "FAIL" }
    );
}
