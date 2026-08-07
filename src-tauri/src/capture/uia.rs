//! UI Automation selection reading, on a dedicated abandonable MTA thread.
//!
//! `IUIAutomation` has **no client-settable timeout** — `IGlobalOptions` and
//! `CoSetProxyBlanket` are not timeout mechanisms, and
//! `IUIAutomation2::TransactionTimeout` bounds an individual provider request
//! rather than a strategy that makes several calls in sequence. So the bound here
//! is external: the request goes to a thread Copper is willing to abandon, and
//! the caller does `recv_timeout`.
//!
//! Abandoning a running thread is sound only under four conditions, all of them
//! requirements rather than preferences, because an abandoned thread is still
//! running, still holds COM state, and may return at any moment:
//!
//! 1. **No COM interface pointer ever leaves this thread.** [`UiaOutcome`] holds
//!    owned plain data only. A marshalled pointer escaping to the worker would
//!    make abandonment unsound rather than merely untidy.
//! 2. Its request channel is retired immediately on abandonment, so a thread that
//!    later unblocks cannot pick up new work and race its replacement.
//! 3. A late-returning thread observes the disconnected channel, calls
//!    `CoUninitialize`, and exits.
//! 4. Its `JoinHandle` is never even stored, let alone joined. Joining a thread
//!    blocked in a cross-process COM call hangs shutdown indefinitely — which
//!    looks exactly like the crash you would then go hunting for.

use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
	CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
	CUIAutomation8, IUIAutomation, IUIAutomationTextPattern, SupportedTextSelection_None,
	UIA_TextPatternId,
};

use crate::diagnostics;
use crate::win32::foreground::Target;

use super::{normalise, Evidence, StrategyResult};

/// `UIA_E_ELEMENTNOTAVAILABLE`
const UIA_E_ELEMENTNOTAVAILABLE: i32 = 0x8004_0201_u32 as i32;
/// `UIA_E_NOTSUPPORTED`
const UIA_E_NOTSUPPORTED: i32 = 0x8004_0204_u32 as i32;

/// How long to wait for a replacement thread to finish COM init. Not covered by
/// the caller's read budget, so a generous value here would quietly make the UIA
/// stage take far longer than `UIA_TIMEOUT`. Task-001 measured init at 3 ms.
const THREAD_INIT_TIMEOUT: Duration = Duration::from_secs(1);

/// How many threads may be abandoned before UI Automation is given up on for the
/// rest of the session.
///
/// Abandoning is safe but not free: each abandoned thread is still alive, still
/// holds COM state, and is never joined. A provider that hangs once usually hangs
/// again, so without a ceiling a single pathological application would leak one
/// thread per capture for as long as Copper runs. Past the ceiling the cascade
/// goes straight to the clipboard fallback, which is the strategy that was going
/// to serve the capture anyway.
const MAX_ABANDONED_UIA_THREADS: u32 = 3;

/// What one read produced. Owned data only — see condition 1.
#[derive(Debug, Clone)]
enum UiaOutcome {
	Text(String),
	/// The automation object could not be created; UIA is unusable this session.
	Unavailable,
	/// Too many threads have been abandoned, so UIA is no longer attempted.
	GivenUp,
	/// No `TextPattern`, or a control that supports no text selection at all.
	NoTextPattern,
	/// A degenerate caret-only range: an insertion point with nothing selected.
	CaretOnly,
	/// The focused element belongs to another process than the sampled target.
	ForeignElement { foreground_moved: bool },
	Timeout,
	Error { hresult: i32, op: &'static str },
}

struct Request {
	/// An `isize` rather than an `HWND`, because the handle has to cross a thread
	/// boundary and `HWND` is not `Send`.
	hwnd: isize,
	expect_pid: u32,
	reply: Sender<UiaOutcome>,
}

/// Owns the current UIA thread and replaces it when one has to be abandoned.
pub struct UiaService {
	requests: Option<Sender<Request>>,
	abandoned: u32,
}

impl UiaService {
	pub fn new() -> Self {
		Self {
			requests: None,
			abandoned: 0,
		}
	}

	/// Creates the COM thread and the automation object ahead of any capture.
	///
	/// Thread creation is not covered by the per-read budget, so paying for it
	/// here keeps the first capture as fast as every later one instead of letting
	/// it blow straight through `UIA_TIMEOUT`. Failure is not fatal: the next read
	/// tries again and the cascade falls through to the clipboard either way.
	pub fn warm_up(&mut self) {
		if self.ensure_thread().is_err() {
			diagnostics::log_error(
				"[copper] capture: UI Automation could not be initialised; \
				 captures will fall through to the clipboard",
			);
		}
	}

	/// Reads the selection, bounded by `budget` from this side.
	pub fn read(&mut self, target: Target, budget: Duration) -> StrategyResult {
		let outcome = self.request(target, budget);
		to_result(outcome)
	}

	fn request(&mut self, target: Target, budget: Duration) -> UiaOutcome {
		if self.abandoned >= MAX_ABANDONED_UIA_THREADS {
			// Given up on for the session. Not an error to report: the cascade falls
			// through to the clipboard fallback, which is what has been serving these
			// captures anyway.
			return UiaOutcome::GivenUp;
		}
		if self.ensure_thread().is_err() {
			return UiaOutcome::Unavailable;
		}
		let Some(requests) = self.requests.as_ref() else {
			return UiaOutcome::Unavailable;
		};

		let (reply_tx, reply_rx) = mpsc::channel();
		let request = Request {
			hwnd: target.hwnd.0 as isize,
			expect_pid: target.pid,
			reply: reply_tx,
		};
		if requests.send(request).is_err() {
			// The thread died on its own. Retire it and let the next call respawn.
			self.requests = None;
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
				self.requests = None;
				UiaOutcome::Error {
					hresult: 0,
					op: "uia thread exited mid-call",
				}
			}
		}
	}

	/// Retires the current thread's channel. The thread may still be blocked in a
	/// cross-process COM call; when it unblocks it finds the channel disconnected,
	/// uninitialises COM, and exits on its own.
	fn abandon(&mut self) {
		self.abandoned += 1;
		self.requests = None;
		if self.abandoned >= MAX_ABANDONED_UIA_THREADS {
			diagnostics::log_error(&format!(
				"[copper] capture: {} UI Automation threads have been abandoned this session; \
				 giving up on UI Automation and using the clipboard fallback alone, rather than \
				 leaking a thread per capture",
				self.abandoned
			));
		} else {
			diagnostics::log_error(&format!(
				"[copper] capture: a UI Automation read exceeded its budget; abandoning the thread \
				 and replacing it on the next capture (abandoned {} so far this session)",
				self.abandoned
			));
		}
	}

	fn ensure_thread(&mut self) -> Result<(), ()> {
		if self.requests.is_some() {
			return Ok(());
		}
		let (requests_tx, requests_rx) = mpsc::channel::<Request>();
		let (ready_tx, ready_rx) = mpsc::channel::<bool>();

		// The JoinHandle is deliberately dropped rather than stored: condition 4.
		// A handle nobody may join is worse than no handle, because eventually
		// somebody joins it.
		thread::Builder::new()
			.name("copper-uia".to_owned())
			.spawn(move || uia_thread(requests_rx, ready_tx))
			.map_err(|_| ())?;

		match ready_rx.recv_timeout(THREAD_INIT_TIMEOUT) {
			Ok(true) => {
				self.requests = Some(requests_tx);
				Ok(())
			}
			_ => Err(()),
		}
	}
}

fn uia_thread(requests: Receiver<Request>, ready: Sender<bool>) {
	// A UI Automation client must run MTA on a thread that owns no windows — which
	// is why this cannot be the worker, whose clipboard writes need a message-only
	// window of their own. It is the only *MTA* thread in the process, and it used
	// to be the only one initialising COM at all: task-015's `trash` initialises an
	// STA on whichever thread runs the attachment sweep, which is never this one,
	// so the two apartments never meet.
	// SAFETY: called once on this thread, paired with CoUninitialize below.
	let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
	if init.is_err() {
		let _ = ready.send(false);
		return;
	}

	// `CUIAutomation8` is the class constant in windows 0.61.3, not
	// `CLSID_CUIAutomation8`.
	// SAFETY: COM is initialised on this thread.
	let automation: IUIAutomation =
		match unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) } {
			Ok(automation) => automation,
			Err(_) => {
				let _ = ready.send(false);
				// SAFETY: paired with the CoInitializeEx above, on this thread.
				unsafe { CoUninitialize() };
				return;
			}
		};
	let _ = ready.send(true);

	while let Ok(request) = requests.recv() {
		let outcome = read_selection(
			&automation,
			HWND(request.hwnd as *mut c_void),
			request.expect_pid,
		);
		// A failed send means this thread was abandoned while blocked. The next
		// `recv` reports the channel disconnected and the loop ends.
		let _ = request.reply.send(outcome);
	}

	drop(automation);
	// SAFETY: paired with the CoInitializeEx above, on this thread, after every
	// interface pointer has been dropped.
	unsafe { CoUninitialize() };
}

/// Resolves the focused element and reads its selection. Runs on the UIA thread.
fn read_selection(automation: &IUIAutomation, hwnd: HWND, expect_pid: u32) -> UiaOutcome {
	// SAFETY (whole function): every call is on the COM-initialised UIA thread,
	// against interfaces obtained on that same thread, and no pointer escapes it.
	unsafe {
		let focused = match automation.GetFocusedElement() {
			Ok(element) => Ok(element),
			// Transient during window switches. One retry, then fall back to
			// resolving from the handle that was sampled.
			Err(err) if err.code().0 == UIA_E_ELEMENTNOTAVAILABLE => {
				automation.GetFocusedElement().map_err(|err| err.code().0)
			}
			Err(err) => Err(err.code().0),
		};

		let element = match focused {
			Ok(element) => element,
			Err(first) => {
				// `ElementFromHandle` takes a plain HWND; there is no UIA_HWND
				// wrapper type in windows 0.61.3.
				match automation.ElementFromHandle(hwnd) {
					Ok(element) => element,
					Err(err) => {
						return UiaOutcome::Error {
							hresult: if first != 0 { first } else { err.code().0 },
							op: "GetFocusedElement/ElementFromHandle",
						}
					}
				}
			}
		};

		// Guard against reading the wrong window on *process* identity rather than
		// handle equality. `GetFocusedElement` is global: it returns the focused
		// element system-wide, which need not belong to the sampled window.
		// Comparing native handles would be too strict — elements legitimately
		// report a child handle or zero, which is exactly what Chrome's render
		// widget does.
		let element_pid = element.CurrentProcessId().unwrap_or(0) as u32;
		if element_pid != expect_pid {
			let foreground_pid = Target::current().map(|target| target.pid);
			return UiaOutcome::ForeignElement {
				foreground_moved: foreground_pid == Some(element_pid),
			};
		}

		let pattern = match element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
		{
			Ok(pattern) => pattern,
			Err(err) => {
				let hresult = err.code().0;
				// Two shapes both mean "no TextPattern here": the documented
				// UIA_E_NOTSUPPORTED, and a null out-parameter that windows-rs turns
				// into an Err carrying a *success* code. Task-001 observed the second
				// live against Microsoft Edge and recorded it as the single most
				// useful thing the spike found — matching only on UIA_E_NOTSUPPORTED
				// would push the most common browser case into the catch-all error
				// bucket as an "error" with HRESULT 0.
				if hresult == UIA_E_NOTSUPPORTED || hresult == 0 {
					return UiaOutcome::NoTextPattern;
				}
				return UiaOutcome::Error {
					hresult,
					op: "GetCurrentPatternAs",
				};
			}
		};

		// Asked before `GetSelection` at all: a provider with no selection support
		// answers this without a call.
		match pattern.SupportedTextSelection() {
			Ok(supported) if supported == SupportedTextSelection_None => {
				return UiaOutcome::NoTextPattern
			}
			Ok(_) => {}
			Err(err) => {
				return UiaOutcome::Error {
					hresult: err.code().0,
					op: "SupportedTextSelection",
				}
			}
		}

		let selection = match pattern.GetSelection() {
			Ok(selection) => selection,
			// The discriminator is an `Err` whose code is S_OK, not E_POINTER:
			// windows 0.61.3 routes a null out-parameter through `Type::from_abi`,
			// windows-core returns `Error::empty()`, and windows-result normalises
			// its code to HRESULT(0). Testing `is_err()` alone would turn the single
			// most common outcome into a spurious UIA failure.
			Err(err) if err.code().0 == 0 => return UiaOutcome::NoTextPattern,
			Err(err) => {
				return UiaOutcome::Error {
					hresult: err.code().0,
					op: "GetSelection",
				}
			}
		};

		let count = match selection.Length() {
			Ok(count) => count,
			Err(err) => {
				return UiaOutcome::Error {
					hresult: err.code().0,
					op: "IUIAutomationTextRangeArray::Length",
				}
			}
		};
		if count <= 0 {
			return UiaOutcome::NoTextPattern;
		}

		// Every range, not just `GetElement(0)`. A multi-cursor selection in VS
		// Code or Cursor is several ranges, and taking the first would hand the
		// user one fragment of what they selected with no indication anything was
		// dropped. Joined with a newline per the checkpoint-1 ruling — the spike
		// concatenated with no separator and flagged that as almost certainly
		// wrong for exactly this case.
		let mut ranges: Vec<String> = Vec::with_capacity(count as usize);
		for index in 0..count {
			let range = match selection.GetElement(index) {
				Ok(range) => range,
				Err(err) => {
					return UiaOutcome::Error {
						hresult: err.code().0,
						op: "IUIAutomationTextRangeArray::GetElement",
					}
				}
			};
			match range.GetText(-1) {
				Ok(text) => ranges.push(text.to_string()),
				Err(err) => {
					return UiaOutcome::Error {
						hresult: err.code().0,
						op: "IUIAutomationTextRange::GetText",
					}
				}
			}
		}

		let text = ranges.join("\n");
		if text.is_empty() {
			// A caret with no selection: one degenerate zero-length range whose text
			// is empty. Distinct from a control with no selection support at all,
			// because the two resolve to different causes.
			return UiaOutcome::CaretOnly;
		}
		UiaOutcome::Text(text)
	}
}

/// Maps a UIA outcome to the cascade's currency: normalised text, or evidence.
///
/// The normalisation happens here rather than in the dispatcher so the "first
/// non-empty wins" test is applied to normalised text. Without that a
/// whitespace-only selection stops the cascade, normalises to empty, and gets
/// misreported as `Unsupported` when it is really `NoSelection`.
fn to_result(outcome: UiaOutcome) -> StrategyResult {
	match outcome {
		UiaOutcome::Text(text) => {
			let normalised = normalise(&text);
			if normalised.is_empty() {
				StrategyResult::nothing(Evidence {
					empty_after_normalisation: true,
					..Evidence::default()
				})
			} else {
				StrategyResult::captured(normalised, Evidence::default())
			}
		}
		UiaOutcome::NoTextPattern => StrategyResult::nothing(Evidence {
			no_text_pattern: true,
			..Evidence::default()
		}),
		UiaOutcome::CaretOnly => StrategyResult::nothing(Evidence {
			caret_without_selection: true,
			..Evidence::default()
		}),
		UiaOutcome::ForeignElement { foreground_moved } => StrategyResult::nothing(Evidence {
			foreground_changed: foreground_moved,
			..Evidence::default()
		}),
		UiaOutcome::Timeout => StrategyResult::nothing(Evidence {
			uia_timed_out: true,
			..Evidence::default()
		}),
		// Neither leaves evidence of its own: the clipboard fallback runs next and
		// its findings are what the precedence rule should decide on. Both are
		// logged, because a UIA layer that has stopped working otherwise presents
		// only as every capture quietly taking the slower path.
		UiaOutcome::Unavailable => {
			diagnostics::log_error("[copper] capture: UI Automation is unavailable");
			StrategyResult::nothing(Evidence::default())
		}
		// Already reported once, when the ceiling was reached. Logging it per
		// capture afterwards would be noise about a decision already taken.
		UiaOutcome::GivenUp => StrategyResult::nothing(Evidence::default()),
		UiaOutcome::Error { hresult, op } => {
			diagnostics::log_error(&format!(
				"[copper] capture: UI Automation {op} failed with 0x{hresult:08X}"
			));
			StrategyResult::nothing(Evidence::default())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_whitespace_only_selection_becomes_evidence_not_text() {
		let result = to_result(UiaOutcome::Text("  \r\n\t ".to_owned()));
		assert!(result.text.is_none());
		assert!(result.evidence.empty_after_normalisation);
	}

	#[test]
	fn text_crosses_the_boundary_normalised() {
		let result = to_result(UiaOutcome::Text("  first\r\nsecond  ".to_owned()));
		assert_eq!(result.text.as_deref(), Some("first\nsecond"));
		assert_eq!(result.evidence, Evidence::default());
	}

	#[test]
	fn a_caret_and_a_missing_pattern_are_different_evidence() {
		// They resolve to different causes: "Nothing was selected." against "This
		// app didn't give Copper anything to capture."
		assert!(to_result(UiaOutcome::CaretOnly).evidence.caret_without_selection);
		assert!(to_result(UiaOutcome::NoTextPattern).evidence.no_text_pattern);
	}

	#[test]
	fn a_foreign_element_only_reports_a_foreground_change_when_focus_really_moved() {
		assert!(
			to_result(UiaOutcome::ForeignElement {
				foreground_moved: true
			})
			.evidence
			.foreground_changed
		);
		// GetFocusedElement is global and can simply be wrong; that is not the
		// user switching windows.
		assert!(
			!to_result(UiaOutcome::ForeignElement {
				foreground_moved: false
			})
			.evidence
			.foreground_changed
		);
	}

	#[test]
	fn a_timeout_is_recorded_but_does_not_stop_the_cascade() {
		let result = to_result(UiaOutcome::Timeout);
		assert!(result.evidence.uia_timed_out);
		assert!(!result.terminal);
	}

	#[test]
	fn no_strategy_result_is_terminal_in_this_phase() {
		// R-Q2: the cascade never terminates on a trusted-empty UIA answer.
		for outcome in [
			UiaOutcome::CaretOnly,
			UiaOutcome::NoTextPattern,
			UiaOutcome::Timeout,
			UiaOutcome::Unavailable,
			UiaOutcome::GivenUp,
		] {
			assert!(!to_result(outcome).terminal);
		}
	}

	#[test]
	fn giving_up_on_uia_leaves_no_evidence_of_its_own() {
		// The clipboard fallback runs next, and its findings are what the
		// precedence rule should decide on. Evidence here would misattribute the
		// cause to a strategy that did not run.
		let result = to_result(UiaOutcome::GivenUp);
		assert_eq!(result.evidence, Evidence::default());
		assert!(result.text.is_none());
	}

	#[test]
	fn the_abandonment_ceiling_stops_spawning_replacements() {
		let mut service = UiaService::new();
		service.abandoned = MAX_ABANDONED_UIA_THREADS;
		// No thread is created and none is needed: the request short-circuits
		// before `ensure_thread`, which is what stops the per-capture thread leak.
		assert!(matches!(
			service.request(
				Target {
					hwnd: windows::Win32::Foundation::HWND(std::ptr::null_mut()),
					pid: 0,
				},
				Duration::from_millis(1)
			),
			UiaOutcome::GivenUp
		));
		assert!(service.requests.is_none());
	}
}
