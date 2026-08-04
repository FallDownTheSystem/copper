//! Console harness for the Phase 0 capture spike.
//!
//! Run it, put something in the foreground, double-tap Shift. Every attempt is
//! printed live and appended to `findings.jsonl`. Ctrl+C prints the summary,
//! including which failure-taxonomy variants have been reached and which have
//! not — which is the check acceptance criterion 5 needs.
//!
//! While it is running, typing a line into stdin changes the `note` label
//! stamped onto subsequent records, so a manual test pass can mark
//! "chrome / page text / no selection" without restarting or guessing after the
//! fact.

use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use capture::capture::{AttemptRecord, CaptureConfig, CaptureOutcome, Capturer, Stage};
use capture::findings::{AttemptContext, FindingsSink};
use capture::foreground::Foreground;
use capture::hook::{self, DoubleTapConfig, RawKey, Trigger, TriggerKey};

const DEFAULT_FINDINGS: &str = "findings.jsonl";

struct Args {
    trigger_key: u32,
    double_tap_ms: u32,
    tap_max_ms: u32,
    hold_max_ms: u32,
    uia_timeout_ms: u64,
    clipboard_timeout_ms: u64,
    note: String,
    log_raw_keys: bool,
    findings: PathBuf,
}

impl Default for Args {
    fn default() -> Self {
        let defaults = DoubleTapConfig::default();
        let cap = CaptureConfig::default();
        Self {
            trigger_key: hook::VK_LSHIFT,
            double_tap_ms: defaults.gap_max_ms,
            tap_max_ms: defaults.tap_max_ms,
            hold_max_ms: defaults.hold_max_ms,
            uia_timeout_ms: cap.uia_timeout.as_millis() as u64,
            clipboard_timeout_ms: cap.clipboard_timeout.as_millis() as u64,
            note: "unlabelled".to_owned(),
            log_raw_keys: false,
            findings: PathBuf::from(DEFAULT_FINDINGS),
        }
    }
}

const USAGE: &str = "\
copper capture spike — Phase 0 evidence gathering

USAGE:
    capture-spike [OPTIONS]

OPTIONS:
    --trigger-key <vk>            Trigger key as a virtual-key code, decimal or 0x-hex, or one
                                  of: shift, ctrl, alt.  [default: shift]
                                  Any of a modifier's three codes selects the whole modifier.
    --double-tap-ms <ms>          Max gap between the first key-up and the second key-down.
                                  [default: 400]
    --tap-max-ms <ms>             Max duration of one press, key-down to its own key-up.
                                  [default: 250]
    --hold-max-ms <ms>            Absolute hold ceiling per press. [default: 500]
    --uia-timeout-ms <ms>         UI Automation budget. Use 1 to force UiaTimeout. [default: 250]
    --clipboard-timeout-ms <ms>   Clipboard sequence-change polling window. [default: 200]
    --note <label>                Label stamped onto subsequent records. Can also be changed at
                                  any time by typing a new line into stdin.
    --log-raw-keys                Log every key event. Off by default: it makes the callback do
                                  more work than the measured hot path.
    --findings <path>             JSONL output path. [default: ./findings.jsonl]
    -h, --help                    Print this help.

WHILE RUNNING:
    Type a line and press Enter to change the note label.
    Ctrl+C prints the summary and exits cleanly.
";

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);

    while let Some(arg) = it.next() {
        let mut value = |name: &str| -> Result<String, String> {
            it.next().ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--trigger-key" => args.trigger_key = parse_vk(&value("--trigger-key")?)?,
            "--double-tap-ms" => args.double_tap_ms = parse_num(&value("--double-tap-ms")?)?,
            "--tap-max-ms" => args.tap_max_ms = parse_num(&value("--tap-max-ms")?)?,
            "--hold-max-ms" => args.hold_max_ms = parse_num(&value("--hold-max-ms")?)?,
            "--uia-timeout-ms" => args.uia_timeout_ms = parse_num(&value("--uia-timeout-ms")?)?,
            "--clipboard-timeout-ms" => {
                args.clipboard_timeout_ms = parse_num(&value("--clipboard-timeout-ms")?)?
            }
            "--note" => args.note = value("--note")?,
            "--log-raw-keys" => args.log_raw_keys = true,
            "--findings" => args.findings = PathBuf::from(value("--findings")?),
            other => return Err(format!("unrecognised argument: {other}")),
        }
    }
    Ok(args)
}

fn parse_num<T: std::str::FromStr>(s: &str) -> Result<T, String> {
    s.parse().map_err(|_| format!("`{s}` is not a valid number"))
}

fn parse_vk(s: &str) -> Result<u32, String> {
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "shift" => Ok(hook::VK_SHIFT),
        "ctrl" | "control" => Ok(hook::VK_CONTROL),
        "alt" | "menu" => Ok(hook::VK_MENU),
        _ => {
            if let Some(hex) = lower.strip_prefix("0x") {
                u32::from_str_radix(hex, 16).map_err(|_| format!("`{s}` is not a valid hex vk"))
            } else {
                lower
                    .parse()
                    .map_err(|_| format!("`{s}` is not a vk code or a known key name"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Session statistics
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ProcessStats {
    first_uia_ms: Option<u64>,
    later_uia_ms: Vec<u64>,
    clipboard_delay_ms: Vec<u64>,
}

#[derive(Default)]
struct Stats {
    attempts: u64,
    by_outcome: BTreeMap<&'static str, u64>,
    observed: BTreeSet<&'static str>,
    owner_mismatches: u64,
    abandoned_uia: u32,
    uia_spawns: u32,
    uia_init_ms: Option<u64>,
    /// Worst end-to-end time for an attempt whose clipboard fallback ran. This
    /// is the figure to compare against the design's 200 ms assumption — the
    /// polling window is only one part of it.
    max_clipboard_path_total_ms: u64,
    max_total_ms: u64,
    per_process: BTreeMap<String, ProcessStats>,
    restore_skipped: u64,
    restore_failed: u64,
}

impl Stats {
    fn record(&mut self, process: &str, rec: &AttemptRecord, first_for_process: bool) {
        self.attempts += 1;
        *self.by_outcome.entry(rec.outcome_name()).or_insert(0) += 1;
        for name in rec.observed_variants() {
            self.observed.insert(name);
        }
        if rec.clipboard_owner_mismatch {
            self.owner_mismatches += 1;
        }
        if matches!(
            rec.restore,
            Stage::Failed(CaptureOutcome::ClipboardRestoreSkipped { .. })
        ) {
            self.restore_skipped += 1;
        }
        if matches!(
            rec.restore,
            Stage::Failed(CaptureOutcome::ClipboardRestoreFailed { .. })
        ) {
            self.restore_failed += 1;
        }
        self.max_total_ms = self.max_total_ms.max(rec.timings.total_ms);
        if !matches!(rec.clipboard, Stage::NotRun) {
            self.max_clipboard_path_total_ms =
                self.max_clipboard_path_total_ms.max(rec.timings.total_ms);
        }

        let entry = self.per_process.entry(process.to_owned()).or_default();
        if let Some(ms) = rec.timings.uia_ms {
            if first_for_process && entry.first_uia_ms.is_none() {
                entry.first_uia_ms = Some(ms);
            } else {
                entry.later_uia_ms.push(ms);
            }
        }
        if let Some(ms) = rec.clipboard_seq_delay_ms {
            entry.clipboard_delay_ms.push(ms);
        }
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

fn worker(
    rx: Receiver<Trigger>,
    cfg: CaptureConfig,
    findings_path: PathBuf,
    note: Arc<Mutex<String>>,
    stats: Arc<Mutex<Stats>>,
    shutdown: Arc<AtomicBool>,
) {
    // The clipboard owner window belongs to this thread and never leaves it.
    let mut capturer = match Capturer::new(cfg) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                error = %e,
                "could not create the clipboard owner window; the clipboard fallback cannot run"
            );
            return;
        }
    };
    let mut sink = match FindingsSink::open(&findings_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, path = %findings_path.display(), "could not open the findings file");
            return;
        }
    };
    tracing::info!(path = %sink.path().display(), "appending findings");

    let mut seen_processes: BTreeSet<String> = BTreeSet::new();

    loop {
        // This thread owns a window, so it must drain its queue rather than
        // blocking indefinitely on recv(). A window whose thread never pumps
        // will hang any process that sends it a message.
        capturer.owner().pump();

        let trigger = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(t) => t,
            Err(RecvTimeoutError::Timeout) => {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };

        handle_trigger(
            &mut capturer,
            &mut sink,
            &mut seen_processes,
            &note,
            &stats,
            trigger,
        );

        if shutdown.load(Ordering::Relaxed) {
            break;
        }
    }

    let mut guard = stats.lock().unwrap();
    guard.abandoned_uia = capturer.uia.abandoned;
    guard.uia_spawns = capturer.uia.spawns;
    guard.uia_init_ms = capturer.uia.init_ms;
}

fn handle_trigger(
    capturer: &mut Capturer,
    sink: &mut FindingsSink,
    seen_processes: &mut BTreeSet<String>,
    note: &Arc<Mutex<String>>,
    stats: &Arc<Mutex<Stats>>,
    trigger: Trigger,
) {
    let label = note.lock().unwrap().clone();
    let foreground = Foreground::current();

    // `NoForegroundWindow` is recorded here rather than by `attempt`, whose
    // signature already presupposes a foreground window.
    let (rec, process, pid, title, elevated, integrity, first_for_process) = match &foreground {
        Some(fg) => {
            let first = seen_processes.insert(fg.process.clone());
            let rec = capturer.attempt(fg);
            (
                rec,
                fg.process.clone(),
                fg.pid,
                fg.title.clone(),
                fg.elevated,
                fg.integrity_label(),
                first,
            )
        }
        None => {
            let rec = AttemptRecord {
                foreground: Stage::Failed(CaptureOutcome::NoForegroundWindow),
                final_outcome: Some(CaptureOutcome::NoForegroundWindow),
                ..Default::default()
            };
            (
                rec,
                "<no foreground window>".to_owned(),
                0,
                String::new(),
                false,
                "unknown",
                false,
            )
        }
    };

    let ctx = AttemptContext {
        note: &label,
        process: &process,
        pid,
        title: &title,
        elevated,
        integrity,
        trigger_injected: trigger.injected,
        trigger_side: trigger.side,
        uia_first_query_for_process: first_for_process,
        uia_init_ms: capturer.uia.init_ms,
        uia_abandoned_total: capturer.uia.abandoned,
    };

    if let Err(e) = sink.append(&ctx, &rec) {
        tracing::error!(error = %e, "could not append to the findings file");
    }

    print_attempt(&label, &process, pid, &title, &rec);
    stats
        .lock()
        .unwrap()
        .record(&process, &rec, first_for_process);
}

fn print_attempt(label: &str, process: &str, pid: u32, title: &str, rec: &AttemptRecord) {
    let preview = rec
        .text
        .as_deref()
        .map(|t| {
            let flat: String = t
                .chars()
                .take(60)
                .map(|c| if c == '\n' || c == '\r' { '\u{23CE}' } else { c })
                .collect();
            let ellipsis = if t.chars().nth(60).is_some() { "…" } else { "" };
            format!(" | {flat}{ellipsis}")
        })
        .unwrap_or_default();

    tracing::info!(
        note = label,
        process,
        pid,
        title = %truncate(title, 48),
        outcome = rec.outcome_name(),
        foreground = rec.foreground.label(),
        uia = rec.uia.label(),
        clipboard = rec.clipboard.label(),
        restore = rec.restore.label(),
        chars = rec.chars,
        uia_ms = rec.timings.uia_ms,
        clipboard_ms = rec.timings.clipboard_ms,
        seq_delay_ms = rec.clipboard_seq_delay_ms,
        total_ms = rec.timings.total_ms,
        "capture attempt{preview}"
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

fn print_summary(stats: &Stats, hooks_timeout: Option<u32>, findings: &Path) {
    let max_ns = hook::MAX_CALLBACK_NS.load(Ordering::Relaxed);
    let mean_ns = hook::mean_callback_ns();
    let events = hook::CALLBACK_COUNT.load(Ordering::Relaxed);

    println!("\n══════════════════════════ SESSION SUMMARY ══════════════════════════");
    println!("Findings file             : {}", findings.display());
    println!("Capture attempts          : {}", stats.attempts);

    println!("\n-- Outcomes --");
    if stats.by_outcome.is_empty() {
        println!("  (none)");
    }
    for (name, count) in &stats.by_outcome {
        println!("  {count:>4}  {name}");
    }

    println!("\n-- Failure taxonomy coverage (acceptance criterion 5) --");
    let unobserved: Vec<&str> = CaptureOutcome::ALL_VARIANTS
        .iter()
        .copied()
        .filter(|v| !stats.observed.contains(v))
        .collect();
    println!(
        "  observed   ({:>2}/{}): {}",
        stats.observed.len(),
        CaptureOutcome::ALL_VARIANTS.len(),
        if stats.observed.is_empty() {
            "-".to_owned()
        } else {
            stats.observed.iter().copied().collect::<Vec<_>>().join(", ")
        }
    );
    println!(
        "  UNOBSERVED ({:>2}/{}): {}",
        unobserved.len(),
        CaptureOutcome::ALL_VARIANTS.len(),
        if unobserved.is_empty() {
            "- all reached".to_owned()
        } else {
            unobserved.join(", ")
        }
    );

    println!("\n-- Hook callback (acceptance criterion 3) --");
    println!("  key events seen              : {events}");
    println!(
        "  max callback duration        : {:.3} ms",
        max_ns as f64 / 1_000_000.0
    );
    println!(
        "  mean callback duration       : {:.3} ms",
        mean_ns as f64 / 1_000_000.0
    );
    match hooks_timeout {
        Some(ms) => {
            let margin = ms as f64 - (max_ns as f64 / 1_000_000.0);
            println!("  LowLevelHooksTimeout         : {ms} ms (explicitly set) - margin {margin:.1} ms");
        }
        None => println!(
            "  LowLevelHooksTimeout         : unset - system default applies (capped at 1000 ms \
             on Windows 10 1709+)"
        ),
    }
    println!(
        "  generic-modifier side unresolved : {}",
        hook::GENERIC_SIDE_UNRESOLVED.load(Ordering::Relaxed)
    );
    println!(
        "  triggers from injected input     : {}",
        hook::INJECTED_TRIGGER_COUNT.load(Ordering::Relaxed)
    );
    println!(
        "  own injected events filtered     : {}",
        hook::SELF_INJECTED_FILTERED.load(Ordering::Relaxed)
    );

    println!("\n-- UI Automation --");
    println!("  automation object init  : {:?}", stats.uia_init_ms);
    println!("  threads spawned         : {}", stats.uia_spawns);
    println!("  threads abandoned       : {}", stats.abandoned_uia);

    println!("\n-- Clipboard --");
    println!("  owner-mismatch attempts : {}", stats.owner_mismatches);
    println!("  restores skipped        : {}", stats.restore_skipped);
    println!(
        "  restores FAILED         : {}{}",
        stats.restore_failed,
        if stats.restore_failed > 0 {
            "   <- user-visible data loss; report this separately in FINDINGS.md"
        } else {
            ""
        }
    );

    println!("\n-- Latency --");
    println!(
        "  worst attempt overall             : {} ms",
        stats.max_total_ms
    );
    println!(
        "  worst attempt using the clipboard : {} ms  (end-to-end, NOT the polling window)",
        stats.max_clipboard_path_total_ms
    );
    if !stats.per_process.is_empty() {
        println!("\n  per process, milliseconds:");
        println!(
            "    {:<26} {:>10} {:>10} {:>10} {:>14}",
            "process", "uia_first", "uia_med", "uia_max", "seq_delay_med"
        );
        for (name, p) in &stats.per_process {
            println!(
                "    {:<26} {:>10} {:>10} {:>10} {:>14}",
                truncate(name, 26),
                p.first_uia_ms
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                median(&p.later_uia_ms),
                p.later_uia_ms
                    .iter()
                    .max()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                median(&p.clipboard_delay_ms),
            );
        }
    }
    println!("=====================================================================");
}

fn median(values: &[u64]) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    v[v.len() / 2].to_string()
}

// ---------------------------------------------------------------------------

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let trigger = TriggerKey::from_vk(args.trigger_key);
    let dt_cfg = DoubleTapConfig {
        tap_max_ms: args.tap_max_ms,
        gap_max_ms: args.double_tap_ms,
        hold_max_ms: args.hold_max_ms,
    };
    let cap_cfg = CaptureConfig {
        uia_timeout: Duration::from_millis(args.uia_timeout_ms),
        clipboard_timeout: Duration::from_millis(args.clipboard_timeout_ms),
        ..Default::default()
    };
    let hooks_timeout = hook::low_level_hooks_timeout();

    println!("copper capture spike - Phase 0");
    println!(
        "  trigger              : double-tap {} (vk 0x{:02X})",
        trigger.label(),
        args.trigger_key
    );
    println!(
        "  tap_max / gap_max    : {} ms / {} ms   (hold ceiling {} ms)",
        dt_cfg.tap_max_ms, dt_cfg.gap_max_ms, dt_cfg.hold_max_ms
    );
    println!(
        "  uia / clipboard      : {} ms / {} ms",
        args.uia_timeout_ms, args.clipboard_timeout_ms
    );
    println!(
        "  LowLevelHooksTimeout : {}",
        hooks_timeout
            .map(|v| format!("{v} ms"))
            .unwrap_or_else(|| "unset - system default".to_owned())
    );
    println!("  note                 : {}", args.note);
    println!(
        "  our integrity        : {:?}",
        capture::foreground::our_integrity()
    );
    println!(
        "\nDouble-tap {} to capture. Type a line to change the note label. Ctrl+C to finish.\n",
        trigger.label()
    );

    let (trigger_tx, trigger_rx) = mpsc::channel::<Trigger>();
    let (raw_tx, raw_rx) = mpsc::channel::<RawKey>();

    let hook_handle = match hook::install_with_raw(
        trigger,
        dt_cfg,
        trigger_tx,
        args.log_raw_keys.then_some(raw_tx),
    ) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("A hook that failed to install presents as a trigger that never fires.");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(
        thread_id = hook_handle.thread_id(),
        "WH_KEYBOARD_LL installed"
    );

    if args.log_raw_keys {
        thread::spawn(move || {
            while let Ok(k) = raw_rx.recv() {
                tracing::info!(
                    vk = format!("0x{:02X}", k.vk),
                    up = k.is_up,
                    injected = k.injected,
                    "raw key"
                );
            }
        });
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    let note = Arc::new(Mutex::new(args.note.clone()));
    let stats = Arc::new(Mutex::new(Stats::default()));

    {
        let shutdown = Arc::clone(&shutdown);
        if let Err(e) = ctrlc::set_handler(move || shutdown.store(true, Ordering::Relaxed)) {
            eprintln!("warning: could not install the Ctrl+C handler: {e}");
        }
    }

    {
        let note = Arc::clone(&note);
        // Detached: it blocks on stdin forever and the process exits out from
        // under it, which is fine for a harness.
        thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let label = line.trim().to_owned();
                if label.is_empty() {
                    continue;
                }
                println!("-- note is now: {label}");
                *note.lock().unwrap() = label;
            }
        });
    }

    let worker_handle = {
        let note = Arc::clone(&note);
        let stats = Arc::clone(&stats);
        let shutdown = Arc::clone(&shutdown);
        let findings = args.findings.clone();
        thread::Builder::new()
            .name("copper-worker".to_owned())
            .spawn(move || worker(trigger_rx, cap_cfg, findings, note, stats, shutdown))
            .expect("could not spawn the worker thread")
    };

    while !shutdown.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(100));
    }
    println!("\nshutting down...");

    let _ = worker_handle.join();
    print_summary(&stats.lock().unwrap(), hooks_timeout, &args.findings);

    // Uninstalls the hook and joins its thread.
    drop(hook_handle);
    ExitCode::SUCCESS
}
