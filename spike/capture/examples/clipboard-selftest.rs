//! Exercise the clipboard write/restore round-trip on its own.
//!
//! This is the most dangerous code in the spike: a write path that opens the
//! clipboard with a NULL owner empties it and then cannot repopulate it,
//! destroying whatever the user had copied. That failure is silent until
//! someone pastes. So it gets its own test that touches nothing else — no
//! focus change, no synthesized keystrokes, no target application.
//!
//! ```text
//! cargo run --example clipboard-selftest
//! ```
//!
//! The comparison is over **raw payload bytes**, not decoded strings, and it
//! seeds a `"HTML Format"` payload as well as text so the HTML restore path is
//! actually exercised rather than merely present. Comparing decoded strings
//! would pass even if the restore mangled the encoding or dropped HTML.
//!
//! Exits non-zero if any stage fails. Prints a JSON report on stdout.

use capture::clipboard::{self, OwnerWindow, Snapshot};

const SEED_TEXT: &str = "copper clipboard self-test SEED — ✓ unicode ✓ \u{1F600}";
const SEED_HTML: &[u8] = b"Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000185\r\n\
StartFragment:0000000141\r\nEndFragment:0000000149\r\n<html><body>\r\n<!--StartFragment-->\
seed<!--EndFragment-->\r\n</body></html>";
const PAYLOAD: &str = "copper clipboard self-test PAYLOAD — should be fully replaced";

fn utf16_bytes(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|c| c.to_ne_bytes())
        .collect()
}

fn describe(bytes: &Option<Vec<u8>>) -> serde_json::Value {
    match bytes {
        Some(b) => serde_json::json!({ "present": true, "bytes": b.len() }),
        None => serde_json::json!({ "present": false }),
    }
}

fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let owner = match OwnerWindow::create() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("could not create the clipboard owner window: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    // 1. Seed a known text + HTML pair so the round-trip has something with
    //    real structure to preserve, and so the HTML path is exercised even on
    //    a machine whose clipboard happened to hold only plain text.
    let seed_text_bytes = utf16_bytes(SEED_TEXT);
    if let Err(e) = clipboard::write_excluded(
        &owner,
        &[
            (clipboard::unicode_text_format_id(), &seed_text_bytes),
            (clipboard::html_format_id(), SEED_HTML),
        ],
        None,
    ) {
        eprintln!("could not seed the clipboard: {e}");
        return std::process::ExitCode::FAILURE;
    }
    owner.pump();

    let seq_start = clipboard::seq();

    // 2. Snapshot it.
    let before: Snapshot = match clipboard::snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("snapshot failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    // 3. Overwrite with a different payload, through the same path the restore
    //    uses.
    let payload_bytes = utf16_bytes(PAYLOAD);
    let write_result = clipboard::write_excluded(
        &owner,
        &[(clipboard::unicode_text_format_id(), &payload_bytes)],
        None,
    );
    owner.pump();

    let seq_after_write = clipboard::seq();
    let read_back = clipboard::read_text().ok().flatten();
    let write_ok = write_result.is_ok() && read_back.as_deref() == Some(PAYLOAD);
    // The overwrite must genuinely have removed the HTML, or "restored" below
    // would be meaningless.
    let html_gone = clipboard::snapshot().map(|s| s.html.is_none()).unwrap_or(false);

    // 4. Put the original back.
    let restore_result = clipboard::restore(&owner, &before, None);
    owner.pump();

    // 5. Compare RAW BYTES, not decoded strings.
    let after = clipboard::snapshot().ok();
    let text_identical = after
        .as_ref()
        .map(|a| a.unicode_text == before.unicode_text)
        .unwrap_or(false);
    let html_identical = after
        .as_ref()
        .map(|a| a.html == before.html)
        .unwrap_or(false);
    let restore_ok = restore_result.is_ok() && text_identical && html_identical;
    let seq_end = clipboard::seq();

    let report = serde_json::json!({
        "seq_start": seq_start,
        "seq_after_write": seq_after_write,
        "seq_end": seq_end,
        "sequence_moved_on_our_write": seq_after_write != seq_start,
        "snapshot_formats": before.present,
        "unrestorable_formats": before.unrestorable,
        "before": { "text": describe(&before.unicode_text), "html": describe(&before.html) },
        "after": after.as_ref().map(|a| serde_json::json!({
            "text": describe(&a.unicode_text), "html": describe(&a.html)
        })),
        "write_result": write_result.as_ref().err().map(|e| e.to_string()),
        "read_back_matches_payload": read_back.as_deref() == Some(PAYLOAD),
        "html_removed_by_the_overwrite": html_gone,
        "restore_result": restore_result.as_ref().err().map(|e| e.to_string()),
        "text_bytes_identical": text_identical,
        "html_bytes_identical": html_identical,
        "write_ok": write_ok,
        "restore_ok": restore_ok,
    });
    println!("{report}");

    println!("\n-- checks --");
    println!(
        "  owner window created            : PASS  (SetClipboardData needs a real HWND; a NULL-owner\n\
         \x20                                        session that calls EmptyClipboard cannot write)"
    );
    println!(
        "  write_excluded + read back      : {}",
        if write_ok { "PASS" } else { "FAIL" }
    );
    println!(
        "  overwrite really dropped HTML   : {}",
        if html_gone { "PASS" } else { "FAIL" }
    );
    println!(
        "  CF_UNICODETEXT bytes restored   : {}",
        if text_identical { "PASS" } else { "FAIL" }
    );
    println!(
        "  CF_HTML bytes restored          : {}",
        if html_identical { "PASS" } else { "FAIL" }
    );
    println!(
        "\n  Manual step this cannot check: open Win+V and confirm NEITHER the payload above nor the\n  \
         restore produced a clipboard-history entry. That is acceptance criterion 8's first half,\n  \
         and it needs clipboard history enabled first (Settings > System > Clipboard)."
    );
    println!(
        "  Note: this test leaves its own seed text on the clipboard, not whatever you had before."
    );

    if write_ok && restore_ok && html_gone {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
