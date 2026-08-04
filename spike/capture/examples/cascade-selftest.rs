//! Run the capture cascade once against the current foreground window, without
//! needing a keystroke to trigger it.
//!
//! This exists because several acceptance criteria are about the *cascade*, not
//! about the hook, and pairing them with a hand-performed double-tap makes them
//! fiddly to reproduce. It is also the easiest way to force
//! `ClipboardBusy`, `ClipboardEmptyText` and `UiaTimeout` repeatably.
//!
//! ```text
//! cargo run --example cascade-selftest -- --delay-ms 3000 --uia-timeout-ms 1
//! ```
//!
//! The delay gives you time to focus the target window and make a selection.
//! The full per-stage record is printed as JSON on stdout.

use std::time::Duration;

use capture::capture::{CaptureConfig, CaptureOutcome, Capturer, Stage};
use capture::foreground::Foreground;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let mut delay_ms = 0u64;
    let mut cfg = CaptureConfig::default();
    let mut repeat = 1u32;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("missing value");
        match arg.as_str() {
            "--delay-ms" => delay_ms = next().parse().expect("bad --delay-ms"),
            "--uia-timeout-ms" => {
                cfg.uia_timeout = Duration::from_millis(next().parse().expect("bad value"))
            }
            "--clipboard-timeout-ms" => {
                cfg.clipboard_timeout = Duration::from_millis(next().parse().expect("bad value"))
            }
            "--repeat" => repeat = next().parse().expect("bad --repeat"),
            other => panic!("unrecognised argument: {other}"),
        }
    }

    if delay_ms > 0 {
        eprintln!("focus the target window now — capturing in {delay_ms} ms");
        std::thread::sleep(Duration::from_millis(delay_ms));
    }

    let mut capturer = Capturer::new(cfg).expect("could not create the clipboard owner window");

    for round in 1..=repeat {
        let Some(fg) = Foreground::current() else {
            println!(
                "{}",
                serde_json::json!({ "round": round, "outcome": "NoForegroundWindow" })
            );
            continue;
        };

        let rec = capturer.attempt(&fg);
        let report = serde_json::json!({
            "round": round,
            "process": fg.process,
            "pid": fg.pid,
            "title": fg.title,
            "elevated": fg.elevated,
            "integrity": fg.integrity_label(),
            "outcome": rec.outcome_name(),
            "chars": rec.chars,
            "strategy": rec.strategy,
            "stages": {
                "foreground": rec.foreground.label(),
                "uia": rec.uia.label(),
                "clipboard": rec.clipboard.label(),
                "restore": rec.restore.label(),
            },
            "detail": {
                "foreground": rec.foreground,
                "uia": rec.uia,
                "clipboard": rec.clipboard,
                "restore": rec.restore,
            },
            "uia_range_count": rec.uia_range_count,
            "uia_ms": rec.timings.uia_ms,
            "clipboard_ms": rec.timings.clipboard_ms,
            "clipboard_seq_delay_ms": rec.clipboard_seq_delay_ms,
            "total_ms": rec.timings.total_ms,
            "uia_init_ms": capturer.uia.init_ms,
            "uia_abandoned": capturer.uia.abandoned,
            "unrestorable_formats": rec.unrestorable_formats,
            "snapshot_formats": rec.snapshot_formats,
            "clipboard_owner_mismatch": rec.clipboard_owner_mismatch,
            // A short preview only, so the tool is usable without dumping
            // whatever the user happened to have selected into a log.
            "preview": rec.text.as_deref().map(|t| t.chars().take(80).collect::<String>()),
        });
        println!("{report}");

        if matches!(
            rec.restore,
            Stage::Failed(CaptureOutcome::ClipboardRestoreFailed { .. })
        ) {
            eprintln!("*** CLIPBOARD RESTORE FAILED — previous clipboard contents are gone ***");
        }

        if round < repeat {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}
