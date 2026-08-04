//! The JSONL evidence sink: one JSON object per attempt, flushed immediately.
//!
//! Flushed per line deliberately — a crash mid-session must not lose the run,
//! and the whole point of this binary is the data it leaves behind.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::capture::{AttemptRecord, CaptureOutcome, Stage, Strategy};
use crate::clipboard::FormatInfo;
use crate::hook::KeySide;

/// Everything about an attempt that the cascade itself does not know.
pub struct AttemptContext<'a> {
    pub note: &'a str,
    pub process: &'a str,
    pub pid: u32,
    pub title: &'a str,
    pub elevated: bool,
    pub integrity: &'a str,
    pub trigger_injected: bool,
    pub trigger_side: KeySide,
    /// First UIA query against this process in this session. Accessibility-tree
    /// activation cost lands on the first query; averaging it away hides it.
    pub uia_first_query_for_process: bool,
    pub uia_init_ms: Option<u64>,
    pub uia_abandoned_total: u32,
}

#[derive(Serialize)]
struct Stages<'a> {
    foreground: &'a Stage,
    uia: &'a Stage,
    clipboard: &'a Stage,
    restore: &'a Stage,
    #[serde(rename = "final")]
    final_outcome: &'a Option<CaptureOutcome>,
}

#[derive(Serialize)]
struct Row<'a> {
    timestamp: String,
    note: &'a str,
    process: &'a str,
    pid: u32,
    title: &'a str,
    elevated: bool,
    integrity: &'a str,
    trigger_injected: bool,
    trigger_side: String,

    // Per-stage names. These are what make acceptance criterion 5 checkable:
    // the final outcome alone cannot show that a forced failure was reached.
    foreground_result: &'static str,
    uia_result: &'static str,
    clipboard_result: &'static str,
    restore_result: &'static str,
    outcome: &'static str,

    strategy: Option<Strategy>,
    chars: usize,
    uia_range_count: Option<u32>,
    send_input_inserted: Option<u32>,
    foreground_ms: Option<u64>,
    uia_ms: Option<u64>,
    clipboard_ms: Option<u64>,
    restore_ms: Option<u64>,
    clipboard_seq_delay_ms: Option<u64>,
    total_ms: u64,
    unrestorable_formats: &'a [FormatInfo],
    snapshot_formats: &'a [FormatInfo],
    clipboard_owner_mismatch: bool,
    clipboard_owner_pid: Option<u32>,
    uia_first_query_for_process: bool,
    uia_init_ms: Option<u64>,
    uia_abandoned_total: u32,

    /// Full payloads for every stage, so a failure that a later stage rescued
    /// keeps its detail rather than being flattened away.
    stages: Stages<'a>,
}

pub struct FindingsSink {
    writer: BufWriter<File>,
    path: PathBuf,
}

impl FindingsSink {
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&mut self, ctx: &AttemptContext<'_>, rec: &AttemptRecord) -> std::io::Result<()> {
        let row = Row {
            timestamp: iso8601_utc_now(),
            note: ctx.note,
            process: ctx.process,
            pid: ctx.pid,
            title: ctx.title,
            elevated: ctx.elevated,
            integrity: ctx.integrity,
            trigger_injected: ctx.trigger_injected,
            trigger_side: ctx.trigger_side.to_string(),

            foreground_result: rec.foreground.label(),
            uia_result: rec.uia.label(),
            clipboard_result: rec.clipboard.label(),
            restore_result: rec.restore.label(),
            outcome: rec.outcome_name(),

            strategy: rec.strategy,
            chars: rec.chars,
            uia_range_count: rec.uia_range_count,
            send_input_inserted: rec.send_input_inserted,
            foreground_ms: rec.timings.foreground_ms,
            uia_ms: rec.timings.uia_ms,
            clipboard_ms: rec.timings.clipboard_ms,
            restore_ms: rec.timings.restore_ms,
            clipboard_seq_delay_ms: rec.clipboard_seq_delay_ms,
            total_ms: rec.timings.total_ms,
            unrestorable_formats: &rec.unrestorable_formats,
            snapshot_formats: &rec.snapshot_formats,
            clipboard_owner_mismatch: rec.clipboard_owner_mismatch,
            clipboard_owner_pid: rec.clipboard_owner_pid,
            uia_first_query_for_process: ctx.uia_first_query_for_process,
            uia_init_ms: ctx.uia_init_ms,
            uia_abandoned_total: ctx.uia_abandoned_total,

            stages: Stages {
                foreground: &rec.foreground,
                uia: &rec.uia,
                clipboard: &rec.clipboard,
                restore: &rec.restore,
                final_outcome: &rec.final_outcome,
            },
        };

        // The captured text itself is deliberately never written here.
        serde_json::to_writer(&mut self.writer, &row)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` without pulling in a date crate.
pub fn iso8601_utc_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_iso8601(now.as_secs() as i64, now.subsec_millis())
}

fn format_iso8601(unix_secs: i64, millis: u32) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_the_epoch() {
        assert_eq!(format_iso8601(0, 0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn formats_a_known_instant() {
        // 2026-08-03T12:30:45Z
        let secs = 1_785_760_245;
        assert_eq!(format_iso8601(secs, 123), "2026-08-03T12:30:45.123Z");
    }

    #[test]
    fn handles_a_leap_day() {
        // 2024-02-29T00:00:00Z
        assert_eq!(format_iso8601(1_709_164_800, 0), "2024-02-29T00:00:00.000Z");
    }

    #[test]
    fn handles_the_end_of_a_year() {
        // 2025-12-31T23:59:59Z
        assert_eq!(format_iso8601(1_767_225_599, 999), "2025-12-31T23:59:59.999Z");
    }
}
