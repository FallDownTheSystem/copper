//! UI Automation selection reading on a dedicated, abandonable MTA thread.
//!
//! `IUIAutomation` has no client-settable timeout — `IGlobalOptions` and
//! `CoSetProxyBlanket` are not timeout mechanisms — so the bound used here is
//! external: the request goes to a thread we are willing to abandon, and the
//! caller does `recv_timeout`.
//!
//! Abandoning a thread is only sound under four conditions, all of which are
//! requirements rather than suggestions, because an abandoned thread is still
//! running, still holds COM state, and may return at any moment:
//!
//! 1. **No COM interface pointer ever leaves this thread.** [`UiaOutcome`] holds
//!    only owned plain data. A marshalled pointer escaping to the worker would
//!    make abandonment unsound rather than merely untidy.
//! 2. Its request channel is retired immediately on abandonment, so a thread
//!    that later unblocks cannot pick up new work and race its replacement.
//! 3. A late-returning thread observes the disconnected channel, calls
//!    `CoUninitialize`, and exits.
//! 4. Its `JoinHandle` is never even stored, let alone joined. Joining a thread
//!    blocked in a cross-process COM call hangs shutdown indefinitely, which
//!    looks exactly like the crash you would then start hunting.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationTextPattern, SupportedTextSelection_None,
    UIA_TextPatternId,
};

/// `UIA_E_ELEMENTNOTAVAILABLE`
const UIA_E_ELEMENTNOTAVAILABLE: i32 = 0x8004_0201_u32 as i32;
/// `UIA_E_NOTSUPPORTED`
const UIA_E_NOTSUPPORTED: i32 = 0x8004_0204_u32 as i32;

/// What one UI Automation read produced. Owned data only — see condition 1.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum UiaOutcome {
    Text {
        text: String,
        range_count: u32,
    },
    /// The automation object could not be created; UIA is never usable.
    Unavailable {
        hresult: i32,
    },
    NoTextPattern {
        hresult: i32,
    },
    NoSelectionSupport {
        reason: &'static str,
    },
    /// The focused element belongs to a different process than the foreground
    /// window we sampled.
    ForeignElement {
        element_pid: u32,
        foreground_pid: Option<u32>,
        /// True when the foreground itself moved to the element's process —
        /// i.e. focus changed under us rather than `GetFocusedElement` being
        /// global and wrong.
        foreground_moved: bool,
    },
    /// A degenerate caret-only range: there is an insertion point but nothing
    /// is selected.
    EmptySelection,
    Timeout,
    Error {
        hresult: i32,
        op: &'static str,
    },
}

struct Request {
    hwnd: isize,
    expect_pid: u32,
    reply: Sender<UiaOutcome>,
}

/// Owns the current UIA thread and replaces it when one has to be abandoned.
pub struct UiaService {
    req_tx: Option<Sender<Request>>,
    /// Time to create the automation object on the current thread. Recorded
    /// separately from per-capture timings because accessibility-tree
    /// activation cost lands on the first query and averaging it away hides it.
    pub init_ms: Option<u64>,
    pub abandoned: u32,
    pub spawns: u32,
}

impl Default for UiaService {
    fn default() -> Self {
        Self::new()
    }
}

impl UiaService {
    pub fn new() -> Self {
        Self {
            req_tx: None,
            init_ms: None,
            abandoned: 0,
            spawns: 0,
        }
    }

    /// Create the COM thread and the automation object ahead of any capture.
    ///
    /// Thread creation is not covered by the per-read budget, so paying for it
    /// here keeps the first capture as fast as every later one and stops it
    /// from blowing through `--uia-timeout-ms`. Failure is not fatal: `read`
    /// will try again and report `UiaUnavailable` if it still cannot.
    pub fn warm_up(&mut self) {
        match self.ensure_thread() {
            Ok(()) => tracing::info!(init_ms = ?self.init_ms, "UI Automation ready"),
            Err(hresult) => tracing::warn!(
                hresult = format!("0x{hresult:08X}"),
                "UI Automation could not be initialised; captures will report UiaUnavailable"
            ),
        }
    }

    /// Read the selection, bounded by `budget` from this side.
    ///
    /// Note the bound covers the read itself. If the thread has to be created
    /// first — the very first call, or the one after an abandonment — that adds
    /// up to a further second; see [`warm_up`](Self::warm_up).
    pub fn read(&mut self, hwnd: HWND, expect_pid: u32, budget: Duration) -> UiaOutcome {
        if let Err(hresult) = self.ensure_thread() {
            return UiaOutcome::Unavailable { hresult };
        }
        let Some(tx) = self.req_tx.as_ref() else {
            return UiaOutcome::Unavailable { hresult: 0 };
        };

        let (reply_tx, reply_rx) = mpsc::channel();
        let request = Request {
            hwnd: hwnd.0 as isize,
            expect_pid,
            reply: reply_tx,
        };
        if tx.send(request).is_err() {
            // The thread died on its own. Retire and let the next call respawn.
            self.req_tx = None;
            return UiaOutcome::Error {
                hresult: 0,
                op: "uia thread exited",
            };
        }

        match reply_rx.recv_timeout(budget) {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => {
                self.abandon();
                UiaOutcome::Timeout
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.req_tx = None;
                UiaOutcome::Error {
                    hresult: 0,
                    op: "uia thread exited mid-call",
                }
            }
        }
    }

    /// Retire the current thread's channel. The thread may still be blocked
    /// inside a cross-process COM call; when it unblocks it will find the
    /// channel disconnected, uninitialise COM, and exit on its own.
    fn abandon(&mut self) {
        self.abandoned += 1;
        self.req_tx = None;
        self.init_ms = None;
        tracing::warn!(
            abandoned_total = self.abandoned,
            "UIA read exceeded its budget; abandoning the thread and replacing it on the next capture"
        );
    }

    fn ensure_thread(&mut self) -> Result<(), i32> {
        if self.req_tx.is_some() {
            return Ok(());
        }
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u64, i32>>();

        // The JoinHandle is deliberately dropped rather than stored: condition 4.
        // A handle we cannot join is worse than no handle, because someone will
        // eventually join it.
        thread::Builder::new()
            .name("copper-uia".to_owned())
            .spawn(move || uia_thread(req_rx, ready_tx))
            .map_err(|_| -1)?;
        self.spawns += 1;

        // Bounded at 1 second, not 5. This wait is NOT covered by the caller's
        // `--uia-timeout-ms` budget, so a generous value here quietly makes the
        // UIA stage take far longer than the configured timeout — and it stalls
        // the worker thread, which is also responsible for pumping the clipboard
        // owner window. Measured init on this machine is 3 ms, so 1 s is already
        // three orders of magnitude of slack. See `warm_up`, which moves this
        // cost out of the capture path entirely in the common case.
        match ready_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(init_ms)) => {
                self.init_ms = Some(init_ms);
                self.req_tx = Some(req_tx);
                Ok(())
            }
            Ok(Err(hresult)) => Err(hresult),
            Err(_) => Err(0),
        }
    }
}

fn uia_thread(req_rx: Receiver<Request>, ready_tx: Sender<Result<u64, i32>>) {
    // The only thread in the process that initialises COM. UI Automation
    // clients should run MTA on a thread that owns no windows; STA causes
    // documented problems.
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() {
        let _ = ready_tx.send(Err(hr.0));
        return;
    }

    let started = Instant::now();
    let automation: IUIAutomation =
        match unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) } {
            Ok(a) => a,
            Err(e) => {
                let _ = ready_tx.send(Err(e.code().0));
                unsafe { CoUninitialize() };
                return;
            }
        };
    let _ = ready_tx.send(Ok(started.elapsed().as_millis() as u64));

    while let Ok(request) = req_rx.recv() {
        let outcome = read_selection(
            &automation,
            HWND(request.hwnd as *mut c_void),
            request.expect_pid,
        );
        // If this send fails we were abandoned while blocked. The next `recv`
        // will report the channel disconnected and this loop will end.
        let _ = request.reply.send(outcome);
    }

    drop(automation);
    unsafe { CoUninitialize() };
}

/// Resolve the focused element and read its selection. Runs on the UIA thread.
fn read_selection(automation: &IUIAutomation, hwnd: HWND, expect_pid: u32) -> UiaOutcome {
    let element = match unsafe { automation.GetFocusedElement() } {
        Ok(e) => Ok(e),
        Err(e) if e.code().0 == UIA_E_ELEMENTNOTAVAILABLE => {
            // Transient during window switches. One retry, then fall back to
            // resolving from the HWND we sampled.
            unsafe { automation.GetFocusedElement() }.map_err(|e| e.code().0)
        }
        Err(e) => Err(e.code().0),
    };

    let element = match element {
        Ok(e) => e,
        Err(first_error) => {
            // `ElementFromHandle` takes a plain HWND — there is no UIA_HWND
            // wrapper type in windows 0.61.3.
            match unsafe { automation.ElementFromHandle(hwnd) } {
                Ok(e) => e,
                Err(e) => {
                    return UiaOutcome::Error {
                        hresult: if first_error != 0 { first_error } else { e.code().0 },
                        op: "GetFocusedElement/ElementFromHandle",
                    }
                }
            }
        }
    };

    // Guard against reading the wrong window, on *process* identity rather than
    // HWND equality. `GetFocusedElement` is global: it returns the focused
    // element system-wide, which need not belong to the window we sampled.
    // Comparing native window handles would be too strict — elements
    // legitimately report a child HWND or 0, which is exactly what Chrome's
    // render widget does.
    let element_pid = unsafe { element.CurrentProcessId() }.unwrap_or(0) as u32;
    if element_pid != expect_pid {
        let foreground_pid =
            crate::foreground::foreground_hwnd().and_then(crate::foreground::pid_of_window);
        return UiaOutcome::ForeignElement {
            element_pid,
            foreground_pid,
            foreground_moved: foreground_pid == Some(element_pid),
        };
    }

    // Same process, different HWND is normal and must not fail the read.
    if let Ok(native) = unsafe { element.CurrentNativeWindowHandle() } {
        if native.0 as isize != hwnd.0 as isize {
            tracing::debug!(
                element_hwnd = native.0 as isize,
                foreground_hwnd = hwnd.0 as isize,
                "focused element reports a different HWND within the same process"
            );
        }
    }

    let pattern =
        match unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
        {
            Ok(p) => p,
            Err(e) => {
                let hresult = e.code().0;
                // Two shapes both mean "no TextPattern here": the documented
                // UIA_E_NOTSUPPORTED, and a null out-parameter that windows-rs
                // converts into an Err carrying a *success* code. Same trap as
                // GetSelection below.
                if hresult == UIA_E_NOTSUPPORTED || hresult == 0 {
                    return UiaOutcome::NoTextPattern { hresult };
                }
                return UiaOutcome::Error {
                    hresult,
                    op: "GetCurrentPatternAs",
                };
            }
        };

    // Check this before calling GetSelection at all: a provider with no
    // selection support sets the out-parameter to NULL, which is not an error
    // return at the Win32 level and would be a crash to dereference in C.
    match unsafe { pattern.SupportedTextSelection() } {
        Ok(supported) => {
            if supported == SupportedTextSelection_None {
                return UiaOutcome::NoSelectionSupport {
                    reason: "SupportedTextSelection_None",
                };
            }
        }
        Err(e) => {
            return UiaOutcome::Error {
                hresult: e.code().0,
                op: "SupportedTextSelection",
            }
        }
    }

    let selection = match unsafe { pattern.GetSelection() } {
        Ok(s) => s,
        Err(e) if e.code().0 == 0 => {
            // The discriminator is an Err whose code() is S_OK, not E_POINTER.
            // windows 0.61.3 routes the null out-parameter through
            // `Type::from_abi`; windows-core 0.61.2 returns `Error::empty()`
            // for a null interface ABI; windows-result 0.3.4 normalises
            // `Error::empty().code()` to HRESULT(0). Matching E_POINTER here
            // would push the most common no-selection case into the catch-all
            // error bucket as an "error" with a success code.
            log_null_selection_once();
            return UiaOutcome::NoSelectionSupport {
                reason: "GetSelection returned a null range array",
            };
        }
        Err(e) => {
            return UiaOutcome::Error {
                hresult: e.code().0,
                op: "GetSelection",
            }
        }
    };

    let count = match unsafe { selection.Length() } {
        Ok(n) => n,
        Err(e) => {
            return UiaOutcome::Error {
                hresult: e.code().0,
                op: "IUIAutomationTextRangeArray::Length",
            }
        }
    };
    if count <= 0 {
        return UiaOutcome::NoSelectionSupport {
            reason: "empty range array",
        };
    }

    // Iterate every range, not just GetElement(0). UIA returns one range per
    // selection, and non-contiguous multi-selection is not exotic on our
    // targets — VS Code and Cursor multi-cursor selection is exactly this.
    let mut text = String::new();
    for i in 0..count {
        let range = match unsafe { selection.GetElement(i) } {
            Ok(r) => r,
            Err(e) => {
                return UiaOutcome::Error {
                    hresult: e.code().0,
                    op: "IUIAutomationTextRangeArray::GetElement",
                }
            }
        };
        match unsafe { range.GetText(-1) } {
            Ok(bstr) => text.push_str(&bstr.to_string()),
            Err(e) => {
                return UiaOutcome::Error {
                    hresult: e.code().0,
                    op: "IUIAutomationTextRange::GetText",
                }
            }
        }
    }

    if text.is_empty() {
        // A caret with no selection: one degenerate zero-length range whose
        // text is empty. Distinct from "this control has no selection support".
        return UiaOutcome::EmptySelection;
    }

    UiaOutcome::Text {
        text,
        range_count: count as u32,
    }
}

/// Confirm the null-`GetSelection` mapping empirically the first time it fires.
fn log_null_selection_once() {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        tracing::info!(
            "GetSelection returned Err with HRESULT 0 (S_OK) — the documented null out-parameter, \
             mapped to UiaNoSelectionSupport. This confirms the windows-rs behaviour the plan predicted."
        );
    }
}
