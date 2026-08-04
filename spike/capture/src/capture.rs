//! The capture cascade, the failure taxonomy, and the per-stage attempt record.
//!
//! The record keeps **each stage's own result** rather than collapsing to a
//! single verdict. That is not tidiness: a forced `UiaTimeout` rescued by the
//! clipboard path would otherwise be recorded as a plain `Success { Clipboard }`
//! and the variant we deliberately provoked would be nowhere in the data, which
//! makes acceptance criterion 5 unverifiable.

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::clipboard::{self, ClipboardError, FormatInfo, OwnerWindow};

use crate::foreground::{self, Foreground};
use crate::uia::{UiaOutcome, UiaService};

/// Which strategy produced the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    Uia,
    Clipboard,
}

/// The cascade order, as a single named constant at module scope.
///
/// dsgn-001 commits to this specifically so that reordering — should the
/// Chromium accessibility cost prove real in practice — is a one-line change
/// rather than a redesign. Do not inline this order into `attempt`.
pub const CASCADE: [Strategy; 2] = [Strategy::Uia, Strategy::Clipboard];

/// The failure taxonomy. Every variant must end up recorded as observed,
/// forced, or explicitly unverified.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum CaptureOutcome {
    Success {
        strategy: Strategy,
        chars: usize,
    },
    /// Recorded by the worker, never by `attempt` — see the note on
    /// [`AttemptRecord`].
    NoForegroundWindow,
    /// UIPI: the foreground process outranks us. Detected before attempting
    /// anything.
    ForegroundElevated {
        integrity: &'static str,
    },
    /// The foreground window changed between the trigger and the injection.
    ForegroundChanged,
    UiaUnavailable {
        hresult: i32,
    },
    UiaNoTextPattern {
        hresult: i32,
    },
    UiaNoSelectionSupport {
        reason: &'static str,
    },
    UiaForeignElement {
        element_pid: u32,
        foreground_pid: Option<u32>,
        foreground_moved: bool,
    },
    UiaEmptySelection,
    UiaTimeout {
        budget_ms: u64,
    },
    UiaError {
        hresult: i32,
        op: &'static str,
    },
    ModifierHeld {
        keys: String,
    },
    SendInputFailed {
        inserted: u32,
        error: u32,
    },
    ClipboardBusy {
        detail: String,
    },
    ClipboardUnchanged {
        waited_ms: u64,
    },
    ClipboardEmptyText {
        reason: String,
    },
    ClipboardSnapshotFailed {
        reason: String,
    },
    /// The clipboard changed again after our read, so the restore was withheld.
    /// A withheld restore is a much smaller harm than a clobbered one.
    ClipboardRestoreSkipped {
        seq_at_read: u32,
        seq_now: u32,
    },
    /// The one outcome here that is user-visible **data loss** rather than a
    /// failed capture. Logged at error level and called out separately in
    /// FINDINGS.md; if it occurs at all, that is a finding in its own right.
    ClipboardRestoreFailed {
        reason: String,
    },
}

impl CaptureOutcome {
    /// Every variant name, for the observed/unobserved reconciliation that makes
    /// acceptance criterion 5 a five-second check instead of a manual trawl.
    pub const ALL_VARIANTS: &'static [&'static str] = &[
        "Success",
        "NoForegroundWindow",
        "ForegroundElevated",
        "ForegroundChanged",
        "UiaUnavailable",
        "UiaNoTextPattern",
        "UiaNoSelectionSupport",
        "UiaForeignElement",
        "UiaEmptySelection",
        "UiaTimeout",
        "UiaError",
        "ModifierHeld",
        "SendInputFailed",
        "ClipboardBusy",
        "ClipboardUnchanged",
        "ClipboardEmptyText",
        "ClipboardSnapshotFailed",
        "ClipboardRestoreSkipped",
        "ClipboardRestoreFailed",
    ];

    pub fn variant_name(&self) -> &'static str {
        match self {
            CaptureOutcome::Success { .. } => "Success",
            CaptureOutcome::NoForegroundWindow => "NoForegroundWindow",
            CaptureOutcome::ForegroundElevated { .. } => "ForegroundElevated",
            CaptureOutcome::ForegroundChanged => "ForegroundChanged",
            CaptureOutcome::UiaUnavailable { .. } => "UiaUnavailable",
            CaptureOutcome::UiaNoTextPattern { .. } => "UiaNoTextPattern",
            CaptureOutcome::UiaNoSelectionSupport { .. } => "UiaNoSelectionSupport",
            CaptureOutcome::UiaForeignElement { .. } => "UiaForeignElement",
            CaptureOutcome::UiaEmptySelection => "UiaEmptySelection",
            CaptureOutcome::UiaTimeout { .. } => "UiaTimeout",
            CaptureOutcome::UiaError { .. } => "UiaError",
            CaptureOutcome::ModifierHeld { .. } => "ModifierHeld",
            CaptureOutcome::SendInputFailed { .. } => "SendInputFailed",
            CaptureOutcome::ClipboardBusy { .. } => "ClipboardBusy",
            CaptureOutcome::ClipboardUnchanged { .. } => "ClipboardUnchanged",
            CaptureOutcome::ClipboardEmptyText { .. } => "ClipboardEmptyText",
            CaptureOutcome::ClipboardSnapshotFailed { .. } => "ClipboardSnapshotFailed",
            CaptureOutcome::ClipboardRestoreSkipped { .. } => "ClipboardRestoreSkipped",
            CaptureOutcome::ClipboardRestoreFailed { .. } => "ClipboardRestoreFailed",
        }
    }
}

/// One stage's own result.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// The stage never ran — an earlier stage already succeeded, or the cascade
    /// short-circuited before reaching it.
    #[default]
    NotRun,
    /// The stage ran and produced what was asked of it.
    Ok,
    /// The stage ran and failed. This is retained even when a later stage
    /// rescues the capture.
    Failed(CaptureOutcome),
}

impl Stage {
    pub fn outcome(&self) -> Option<&CaptureOutcome> {
        match self {
            Stage::Failed(o) => Some(o),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Stage::NotRun => "not_run",
            Stage::Ok => "ok",
            Stage::Failed(o) => o.variant_name(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Timings {
    pub foreground_ms: Option<u64>,
    pub uia_ms: Option<u64>,
    pub clipboard_ms: Option<u64>,
    /// The restore is timed separately from the clipboard stage: it happens
    /// after the capture has already succeeded or failed, and a slow restore is
    /// a different problem from a slow capture.
    pub restore_ms: Option<u64>,
    pub total_ms: u64,
}

/// One capture attempt, with every stage's own result preserved.
#[derive(Debug, Clone, Default)]
pub struct AttemptRecord {
    pub foreground: Stage,
    pub uia: Stage,
    pub clipboard: Stage,
    /// The restore is tracked separately from the capture outcome: a skipped or
    /// failed restore does not make a successful capture unsuccessful, and
    /// collapsing the two would hide the one outcome that means data loss.
    pub restore: Stage,
    pub final_outcome: Option<CaptureOutcome>,
    pub timings: Timings,
    /// Kept for the console preview only — deliberately never written to the
    /// findings file.
    pub text: Option<String>,
    pub strategy: Option<Strategy>,
    pub chars: usize,
    pub uia_range_count: Option<u32>,
    pub send_input_inserted: Option<u32>,
    pub clipboard_seq_delay_ms: Option<u64>,
    pub clipboard_owner_mismatch: bool,
    pub clipboard_owner_pid: Option<u32>,
    pub unrestorable_formats: Vec<FormatInfo>,
    pub snapshot_formats: Vec<FormatInfo>,
}

impl AttemptRecord {
    /// Every outcome this attempt touched, across all stages, for the
    /// observed-variant tally.
    pub fn observed_variants(&self) -> Vec<&'static str> {
        let mut seen = Vec::new();
        for stage in [&self.foreground, &self.uia, &self.clipboard, &self.restore] {
            if let Some(o) = stage.outcome() {
                seen.push(o.variant_name());
            }
        }
        if let Some(o) = &self.final_outcome {
            seen.push(o.variant_name());
        }
        seen.sort_unstable();
        seen.dedup();
        seen
    }

    pub fn outcome_name(&self) -> &'static str {
        self.final_outcome
            .as_ref()
            .map(CaptureOutcome::variant_name)
            .unwrap_or("None")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureConfig {
    pub uia_timeout: Duration,
    pub clipboard_timeout: Duration,
    pub modifier_wait: Duration,
    pub poll_interval: Duration,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            uia_timeout: Duration::from_millis(250),
            clipboard_timeout: Duration::from_millis(200),
            modifier_wait: Duration::from_millis(300),
            poll_interval: Duration::from_millis(15),
        }
    }
}

/// Owns everything the cascade needs. Lives on the worker thread.
///
/// (`attempt` is a method rather than the free function the design sketched,
/// because the cascade needs the long-lived UIA service and the clipboard owner
/// window, and neither can be a global.)
pub struct Capturer {
    pub uia: UiaService,
    cfg: CaptureConfig,
    owner: OwnerWindow,
}

impl Capturer {
    pub fn new(cfg: CaptureConfig) -> Result<Self, ClipboardError> {
        let mut uia = UiaService::new();
        // Pay for COM and the automation object now rather than inside the
        // first capture, where it would not be covered by the read budget.
        uia.warm_up();
        Ok(Self {
            uia,
            cfg,
            owner: OwnerWindow::create()?,
        })
    }

    pub fn owner(&self) -> &OwnerWindow {
        &self.owner
    }

    /// Run the cascade against a foreground window.
    ///
    /// `NoForegroundWindow` is not producible here — this signature already
    /// presupposes a foreground window. The worker records that outcome when
    /// `Foreground::current()` returns `None` and never calls this at all.
    pub fn attempt(&mut self, fg: &Foreground) -> AttemptRecord {
        let started = Instant::now();
        let mut rec = AttemptRecord::default();

        if fg.elevated {
            let outcome = CaptureOutcome::ForegroundElevated {
                integrity: fg.integrity_label(),
            };
            rec.foreground = Stage::Failed(outcome.clone());
            rec.final_outcome = Some(outcome);
            rec.timings.foreground_ms = Some(started.elapsed().as_millis() as u64);
            rec.timings.total_ms = started.elapsed().as_millis() as u64;
            return rec;
        }
        rec.foreground = Stage::Ok;
        rec.timings.foreground_ms = Some(started.elapsed().as_millis() as u64);

        for strategy in CASCADE {
            match strategy {
                Strategy::Uia => {
                    let t = Instant::now();
                    let outcome = self.uia.read(fg.hwnd, fg.pid, self.cfg.uia_timeout);
                    rec.timings.uia_ms = Some(t.elapsed().as_millis() as u64);

                    match outcome {
                        UiaOutcome::Text { text, range_count } => {
                            rec.uia = Stage::Ok;
                            rec.uia_range_count = Some(range_count);
                            rec.chars = text.chars().count();
                            rec.strategy = Some(Strategy::Uia);
                            rec.final_outcome = Some(CaptureOutcome::Success {
                                strategy: Strategy::Uia,
                                chars: rec.chars,
                            });
                            rec.text = Some(text);
                        }
                        other => {
                            let mapped = self.map_uia(other);
                            rec.uia = Stage::Failed(mapped.clone());
                            rec.final_outcome = Some(mapped);
                        }
                    }
                }
                Strategy::Clipboard => {
                    let t = Instant::now();
                    match self.clipboard_fallback(fg, &mut rec) {
                        Ok(text) => {
                            rec.clipboard = Stage::Ok;
                            rec.chars = text.chars().count();
                            rec.strategy = Some(Strategy::Clipboard);
                            rec.final_outcome = Some(CaptureOutcome::Success {
                                strategy: Strategy::Clipboard,
                                chars: rec.chars,
                            });
                            rec.text = Some(text);
                        }
                        Err(outcome) => {
                            rec.clipboard = Stage::Failed(outcome.clone());
                            rec.final_outcome = Some(outcome);
                        }
                    }
                    rec.timings.clipboard_ms = Some(t.elapsed().as_millis() as u64);
                }
            }

            if rec.strategy.is_some() {
                break;
            }
        }

        rec.timings.total_ms = started.elapsed().as_millis() as u64;
        rec
    }

    fn map_uia(&self, outcome: UiaOutcome) -> CaptureOutcome {
        match outcome {
            // Handled by the caller; mapping it here would lose the text.
            UiaOutcome::Text { text, .. } => CaptureOutcome::Success {
                strategy: Strategy::Uia,
                chars: text.chars().count(),
            },
            UiaOutcome::Unavailable { hresult } => CaptureOutcome::UiaUnavailable { hresult },
            UiaOutcome::NoTextPattern { hresult } => CaptureOutcome::UiaNoTextPattern { hresult },
            UiaOutcome::NoSelectionSupport { reason } => {
                CaptureOutcome::UiaNoSelectionSupport { reason }
            }
            UiaOutcome::ForeignElement {
                element_pid,
                foreground_pid,
                foreground_moved,
            } => CaptureOutcome::UiaForeignElement {
                element_pid,
                foreground_pid,
                foreground_moved,
            },
            UiaOutcome::EmptySelection => CaptureOutcome::UiaEmptySelection,
            UiaOutcome::Timeout => CaptureOutcome::UiaTimeout {
                budget_ms: self.cfg.uia_timeout.as_millis() as u64,
            },
            UiaOutcome::Error { hresult, op } => CaptureOutcome::UiaError { hresult, op },
        }
    }

    /// Snapshot → modifier check → foreground revalidation → `seq_before` →
    /// inject → poll → read → re-check → restore.
    ///
    /// The order is load-bearing; each step avoids a specific real failure.
    fn clipboard_fallback(
        &mut self,
        fg: &Foreground,
        rec: &mut AttemptRecord,
    ) -> Result<String, CaptureOutcome> {
        self.owner.pump();

        let mut snapshot = match clipboard::snapshot() {
            Ok(s) => s,
            Err(ClipboardError::Busy {
                attempts,
                elapsed_ms,
            }) => {
                return Err(CaptureOutcome::ClipboardBusy {
                    detail: format!("snapshot: {attempts} attempts over {elapsed_ms} ms"),
                })
            }
            Err(e) => {
                return Err(CaptureOutcome::ClipboardSnapshotFailed {
                    reason: e.to_string(),
                })
            }
        };
        rec.unrestorable_formats = std::mem::take(&mut snapshot.unrestorable);
        rec.snapshot_formats = std::mem::take(&mut snapshot.present);

        // The trigger fires on a key-up, so nothing should be held — but the
        // user can genuinely be holding something else.
        if let Err(held) = clipboard::wait_for_modifier_release(self.cfg.modifier_wait) {
            return Err(CaptureOutcome::ModifierHeld {
                keys: held.join("+"),
            });
        }

        // Revalidate before injecting. The trigger fired on a key-up and the
        // modifier wait can add 300 ms, during which focus can move — a Sticky
        // Keys confirmation dialog is exactly this scenario. Injecting Ctrl+C
        // into a window that is no longer the one we sampled sends a keystroke
        // to the wrong application.
        match foreground::foreground_hwnd() {
            Some(now) if now.0 as isize == fg.hwnd.0 as isize => {}
            _ => return Err(CaptureOutcome::ForegroundChanged),
        }

        // Sampled immediately before SendInput, not before the snapshot.
        // Reading the clipboard during the snapshot can itself advance the
        // sequence number if a format was delayed-rendered, and every
        // millisecond between the sample and the injection is a window in which
        // an unrelated write gets attributed to us.
        let seq_before = clipboard::seq();

        let injected_at = Instant::now();
        // A SHORT insert is not the same as no insert. If Ctrl-down and C-down
        // went in but the key-ups did not, the target may already have copied —
        // so the clipboard still has to be polled and put back. Returning early
        // here would leave the target's copy sitting on the user's clipboard
        // with the snapshot never restored.
        let send_result = clipboard::send_ctrl_c();
        rec.send_input_inserted = Some(match send_result {
            Ok(()) => 4,
            Err(short) => short.inserted,
        });

        let observed = self.poll_for_sequence_change(fg, rec, seq_before, injected_at);

        let Some((seq_at_read, delay_ms, foreign_owner)) = observed else {
            // Nothing moved, so nothing to restore and nothing to undo.
            return Err(match send_result {
                Ok(()) => CaptureOutcome::ClipboardUnchanged {
                    waited_ms: injected_at.elapsed().as_millis() as u64,
                },
                Err(short) => CaptureOutcome::SendInputFailed {
                    inserted: short.inserted,
                    error: short.error,
                },
            });
        };
        rec.clipboard_seq_delay_ms = Some(delay_ms);

        let read_result = clipboard::read_text();

        // Re-sample AFTER the read, not before it. Reading a delayed-rendered
        // format makes the owning application call `SetClipboardData`, which
        // bumps the sequence number — so expecting the pre-read value would
        // withhold the restore every time we capture from an application that
        // uses delayed rendering, which is most of the interesting ones.
        // Expecting the post-read value still cannot clobber a user copy,
        // because anything written after this point is caught by the check
        // inside the write session.
        let seq_after_read = clipboard::seq();
        // And re-check the owner: if somebody else has taken the clipboard
        // since, that content is not ours to overwrite either.
        let foreign_now = clipboard::owner_pid().is_some_and(|pid| pid != fg.pid);

        let restore_started = Instant::now();
        if foreign_owner || foreign_now {
            // The write we observed came from somebody other than our target,
            // which means it is almost certainly something the user copied
            // themselves. Restoring the snapshot over it would destroy it. The
            // capture still takes the observation (the design mandates treating
            // the owner check as a soft signal), but the restore must not run.
            tracing::warn!(
                seq_at_read,
                seq_after_read,
                foreign_at_observation = foreign_owner,
                foreign_now,
                "the clipboard is owned by another process; withholding the restore rather than \
                 overwriting what is almost certainly something the user copied"
            );
            rec.restore = Stage::Failed(CaptureOutcome::ClipboardRestoreSkipped {
                seq_at_read,
                seq_now: seq_after_read,
            });
        } else {
            // The sequence check happens INSIDE the write session, because
            // acquiring the clipboard can take up to a second of retries and a
            // copy made during that window would otherwise be destroyed by the
            // EmptyClipboard. Do not resume polling after this point — the
            // restore bumps the sequence itself.
            match clipboard::restore(&self.owner, &snapshot, Some(seq_after_read)) {
                Ok(()) => rec.restore = Stage::Ok,
                Err(ClipboardError::Superseded { expected, actual }) => {
                    tracing::warn!(
                        expected,
                        actual,
                        "clipboard changed again before the restore could take effect; \
                         restore withheld"
                    );
                    rec.restore = Stage::Failed(CaptureOutcome::ClipboardRestoreSkipped {
                        seq_at_read: expected,
                        seq_now: actual,
                    });
                }
                Err(e) => {
                    // User-visible data loss, not merely a failed capture.
                    tracing::error!(
                        error = %e,
                        "CLIPBOARD RESTORE FAILED — the user's previous clipboard contents are gone"
                    );
                    rec.restore = Stage::Failed(CaptureOutcome::ClipboardRestoreFailed {
                        reason: e.to_string(),
                    });
                }
            }
        }
        rec.timings.restore_ms = Some(restore_started.elapsed().as_millis() as u64);
        self.owner.pump();

        // A short insert is reported as such even when text came back, because
        // the evidence matters more than the capture: a partial insert is the
        // failure that can leave Ctrl stuck down system-wide.
        if let Err(short) = send_result {
            return Err(CaptureOutcome::SendInputFailed {
                inserted: short.inserted,
                error: short.error,
            });
        }

        match read_result {
            Ok(Some(text)) if !text.is_empty() => Ok(text),
            Ok(Some(_)) => Err(CaptureOutcome::ClipboardEmptyText {
                reason: "CF_UNICODETEXT present but empty".to_owned(),
            }),
            Ok(None) => Err(CaptureOutcome::ClipboardEmptyText {
                reason: "CF_UNICODETEXT absent after the sequence number moved".to_owned(),
            }),
            Err(ClipboardError::Busy {
                attempts,
                elapsed_ms,
            }) => Err(CaptureOutcome::ClipboardBusy {
                detail: format!("read: {attempts} attempts over {elapsed_ms} ms"),
            }),
            Err(e) => Err(CaptureOutcome::ClipboardEmptyText {
                reason: e.to_string(),
            }),
        }
    }

    /// Poll until the sequence number moves, returning
    /// `(seq, delay_ms, came_from_a_foreign_owner)`.
    ///
    /// A bare "sequence number moved" is not a sufficient discriminator:
    /// clipboard managers, Office, and browsers with clipboard listeners all
    /// write during the polling window and any of them would satisfy it. So the
    /// owner process is checked too — as a **soft** signal for the *capture*
    /// (applications set the clipboard with no owner window or through OLE, so a
    /// mismatch must never by itself discard an otherwise good capture) but as a
    /// **hard** signal for the *restore*: content somebody else just wrote is
    /// not ours to overwrite.
    fn poll_for_sequence_change(
        &self,
        fg: &Foreground,
        rec: &mut AttemptRecord,
        seq_before: u32,
        injected_at: Instant,
    ) -> Option<(u32, u64, bool)> {
        let deadline = injected_at + self.cfg.clipboard_timeout;
        let mut observed: Option<(u32, u64, bool)> = None;
        let mut reported: Vec<u32> = Vec::new();

        loop {
            let now = clipboard::seq();
            if now != seq_before {
                let owner = clipboard::owner_pid();
                let delay_ms = injected_at.elapsed().as_millis() as u64;
                let mismatch = owner.is_some_and(|pid| pid != fg.pid);
                observed = Some((now, delay_ms, mismatch));

                if !mismatch {
                    break;
                }
                // Log once per distinct sequence value, not once per poll
                // iteration — otherwise a single foreign write produces a dozen
                // identical lines and buries the signal.
                if !reported.contains(&now) {
                    reported.push(now);
                    rec.clipboard_owner_mismatch = true;
                    rec.clipboard_owner_pid = owner;
                    tracing::warn!(
                        seq = now,
                        owner_pid = owner,
                        target_pid = fg.pid,
                        "clipboard changed but the owner is not the target process; \
                         continuing to poll and taking the last observation"
                    );
                }
            }

            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(self.cfg.poll_interval);
        }

        observed
    }
}
