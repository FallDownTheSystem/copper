# Copper Phase 0 — capture spike findings

Deliverable for `task-001-win32-capture-spike`. Evidence and a verdict on whether
the capture architecture in `backlog/designs/dsgn-001-copper-windows-architecture.md`
survives Phase 0.

**Status: INCOMPLETE — automated evidence is in, manual verification is pending.**
Everything reachable without a human at the keyboard has been built, run and
recorded below. The per-application matrix, the `Win+V` checks, the elevated-window
case, the uiAccess signing test and the Tauri probe focus scenarios all need a
person present; each has step-by-step instructions in the
**MANUAL VERIFICATION PENDING** section.

---

## Interim verdict

**Provisional GO on the mechanisms; the architecture is not yet cleared.**

Everything that could be proven mechanically has passed, including the two things
most likely to sink the design:

- The hook callback runs in **7.8 microseconds worst case** against a budget of up
  to 1000 ms. That is roughly **128,000x headroom** on the constraint the plan
  called "the single hardest constraint on the callback design".
- The clipboard write/restore round-trip **works exactly**, which is the code path
  that, done wrong, destroys the user clipboard silently.

What is genuinely unresolved is not a mechanism but a _product_ question: whether
UI Automation returns usable selections from VS Code, Cursor and Windows Terminal,
or whether the clipboard fallback ends up serving the primary editing targets. The
one incidental data point so far — Microsoft Edge returning `UiaNoTextPattern` — is
mildly discouraging and is discussed under Open questions.

An external codex review found **three data-loss defects in the clipboard restore
path** that this document previously reported as passing. All three are now fixed
and the automated evidence is stronger than it was; see
[External review](#external-review--codex-2026-08-03). Read that section before
trusting criterion 7 — the code no longer contains a known path that destroys the
user's clipboard, but the claim is only fully settled by the manual pass.

No finding so far invalidates a binding decision in dsgn-001. Several findings
**correct** it; those are collected under Corrections.

---

## What was built

Three Cargo projects under `spike/`, deliberately not a workspace.

| Path                   | What it is                                                  |
| ---------------------- | ----------------------------------------------------------- |
| `spike/capture/`       | lib + bin. Every Win32 mechanism, plus the console harness. |
| `spike/tauri-probe/`   | Minimal Tauri 2 app that installs the same hook in-process. |
| `spike/uiaccess-test/` | Tiny signed-binary test for acceptance criterion 12.        |

Inside `spike/capture/src/`:

| File            | Responsibility                                                                                                                               |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `hook.rs`       | `WH_KEYBOARD_LL` install/uninstall, dedicated thread with a `GetMessageW` pump, the pure double-tap state machine, callback instrumentation. |
| `foreground.rs` | Foreground identity: HWND, PID, title, process name, integrity.                                                                              |
| `uia.rs`        | COM MTA thread, `IUIAutomation`, `TextPattern` selection read, external timeout with thread abandonment.                                     |
| `clipboard.rs`  | Message-only owner window, RAII clipboard guard, snapshot/restore, injected `Ctrl+C`, sequence polling, history-exclusion formats.           |
| `capture.rs`    | The cascade, `CaptureOutcome`, per-stage `AttemptRecord`.                                                                                    |
| `findings.rs`   | One JSON object per attempt, flushed, to `findings.jsonl`.                                                                                   |
| `main.rs`       | Console harness.                                                                                                                             |

Four examples, which exist because several acceptance criteria are about the
cascade rather than the hook, and pairing them with a hand-performed double-tap
makes them needlessly fiddly to reproduce:

| Example              | Purpose                                                                                                                                                                         |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cascade-selftest`   | Run one capture attempt against the current foreground window, no keystroke needed. The easiest way to force `UiaTimeout`, `ClipboardBusy` and `ClipboardEmptyText` repeatably. |
| `clipboard-selftest` | Clipboard write/restore round-trip on its own. No focus change, no synthesized keys.                                                                                            |
| `hook-latency`       | Acceptance criterion 3. Injects unassigned-key events and reports callback duration against the budget.                                                                         |
| `integrity-probe`    | Diagnoses where integrity detection fails and for which processes.                                                                                                              |

Two scripts in `spike/scripts/`:

| Script               | Purpose                                                                                          |
| -------------------- | ------------------------------------------------------------------------------------------------ |
| `verify-cascade.ps1` | End-to-end cascade test against Notepad, with the caller clipboard saved and restored around it. |
| `uiaccess-setup.ps1` | Certificate generation, signing, `%ProgramFiles%` install, and the removal path. Criterion 12.   |

---

## How to run it

```powershell
# From spike/, so findings.jsonl lands in spike/.
cd spike

# The harness. Double-tap Shift to capture; type a line to relabel; Ctrl+C for the summary.
cargo run --manifest-path capture/Cargo.toml -- --note "chrome / input / selected"

# One attempt against whatever is in the foreground, no keystroke needed.
cargo run --manifest-path capture/Cargo.toml --example cascade-selftest -- --delay-ms 3000

# The dangerous path on its own.
cargo run --manifest-path capture/Cargo.toml --example clipboard-selftest

# Criterion 3.
cargo run --release --manifest-path capture/Cargo.toml --example hook-latency

# The Tauri probe (no npm install needed; frontendDist serves static files).
cargo run --manifest-path tauri-probe/src-tauri/Cargo.toml
```

The harness writes `findings.jsonl` to the current directory; override with
`--findings <path>`. The captured **text is deliberately never written to it** —
only a character count.

---

## Automated evidence

### Build, test, lint

| Check                                       | Result                                 |
| ------------------------------------------- | -------------------------------------- |
| `cargo build` (all three crates)            | Clean, zero warnings                   |
| `cargo test`                                | **38 passed, 0 failed**                |
| `cargo clippy --all-targets -- -D warnings` | Clean on all three crates              |
| Toolchain                                   | rustc 1.88.0, `x86_64-pc-windows-msvc` |
| Machine                                     | Windows 11 Pro 10.0.26200              |

The 38 tests are all pure logic: the double-tap state machine, the trigger-key
classifier, clipboard UTF-16 decoding, the timestamp formatter, and the
UIPI-reachability rule. Nothing in the suite touches Win32 state, so it is
reproducible on any machine.

### Acceptance criterion 3 — hook callback duration

`cargo run --release --example hook-latency`

| Build   | Events | Triggers fired | Max callback  | Mean callback | Headroom  |
| ------- | ------ | -------------- | ------------- | ------------- | --------- |
| release | 2000   | 500 / 500      | **0.0078 ms** | 0.0008 ms     | ~128,000x |
| debug   | 1200   | 300 / 300      | **0.0116 ms** | 0.0019 ms     | ~86,000x  |

`LowLevelHooksTimeout` is **unset** on this machine, so the system default applies
(capped at 1000 ms on Windows 10 1709+). Both figures are far below the 1 ms
performance criterion. **PASS.**

Worth noting beyond the timing: 500 injected double-taps produced **exactly 500
triggers**, and 300 produced exactly 300. That exercises the real hook callback,
the real classifier and the real state machine in situ, so criterion 2 mechanism
is verified end-to-end through Win32, not only by unit test. What remains manual
for criterion 2 is the _negative_ cases performed by hand.

Events were synthesized for virtual key `0xE8`, which is unassigned, so no
application reacts to them. (`win-hotkeys` ships the same key as its "silent key"
for exactly this reason.) No capture cascade runs during this test.

### Acceptance criterion 7 — clipboard snapshot and restore

`cargo run --example clipboard-selftest`. The test seeds a known
`CF_UNICODETEXT` **and** `"HTML Format"` pair, snapshots it, overwrites it with a
different payload, restores, and compares **raw payload bytes** — not decoded
strings, which would pass even if the restore mangled the encoding or silently
dropped HTML.

| Check                                                                           | Result            |
| ------------------------------------------------------------------------------- | ----------------- |
| Message-only owner window created                                               | PASS              |
| `write_excluded` wrote and read back byte-exact                                 | PASS              |
| The overwrite genuinely removed the seeded HTML (so the restore is not a no-op) | PASS              |
| `CF_UNICODETEXT` restored byte-identical                                        | PASS (98 bytes)   |
| `"HTML Format"` restored byte-identical                                         | PASS (177 bytes)  |
| Sequence number across our own writes                                           | 430 -> 438 -> 447 |

The sequence figures confirm the design reasoning directly: **our own restore
bumps the sequence number too**, which is why the poll must stop before the
restore and why the expected value is tracked explicitly rather than watching for
"any change".

Formats present in the snapshot were `CF_UNICODETEXT`, `CF_LOCALE`, `CF_TEXT`,
`CF_OEMTEXT`. The last three are **system-synthesized**, so the `unrestorable`
list contains entries that are not real losses — exactly as the design predicted.
Anyone reading `findings.jsonl` needs to know that before drawing conclusions from
that field.

Still manual: the `Win+V` half of criterion 8, and the behaviour when a real
target application writes the clipboard.

### windows crate — exactly one version resolves

`cargo tree -i windows` in `tauri-probe/src-tauri/`:

```
windows v0.61.3
  capture v0.1.0
  tao v0.35.3 -> tauri-runtime-wry v2.11.4 -> tauri v2.11.5
  webview2-com v0.38.2
  wry v0.55.1
```

**One version, `0.61.3`, across Tauri 2.11.5 and `capture`.** This turns the
version reasoning in the task Notes from an assumption into a verified fact: the
`HWND` newtypes unify, and the boundary cast dsgn-001 chose the full `windows`
crate to eliminate stays eliminated through the Phase 4 merge. Tauri resolved to
**2.11.5**, matching dsgn-001 stated version.

### uiAccess manifest

`mt.exe -inputresource:uiaccess-test.exe;#1` extracts:

```xml
<requestedExecutionLevel level="asInvoker" uiAccess="true"></requestedExecutionLevel>
```

Embedded through the MSVC linker (`/MANIFEST:EMBED` plus `/MANIFESTINPUT`) rather
than a helper crate, so the manifest is auditable. Signing and the elevated-read
test remain manual — see criterion 12 below.

### Integrity-level detection

`cargo run --example integrity-probe`, across 151 processes on this machine:

| Outcome                                                 | Count |
| ------------------------------------------------------- | ----- |
| Integrity read with `PROCESS_QUERY_LIMITED_INFORMATION` | 147   |
| `OpenProcessToken` denied (`ERROR_ACCESS_DENIED`)       | 4     |

The four are `audiodg.exe` and three `Discord.exe` instances. Escalating to
`PROCESS_QUERY_INFORMATION` does not help — `OpenProcess` itself is denied for
those. So `PROCESS_QUERY_LIMITED_INFORMATION` is the correct access right, as the
design specified. See correction 2 for the bug this surfaced.

---

## Hook crate decision

**Verdict: hand-roll. Confirmed, and the case is stronger than the plan recorded.**

Both crates were evaluated by reading their published sources, which for these
specific claims is better evidence than a hand-run: "is the `KeyUp` variant ever
constructed" is a question source answers definitively and observation cannot.

### win-hotkeys 0.5.1 — all three blockers reproduce

| Blocker                  | Evidence                                                                                                                                                   |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Fires only on key-down   | `hook.rs:207-235`: `hook_proc` matches `WM_KEYDOWN` / `WM_SYSKEYDOWN` and discards everything else through a bare `_ => {}`.                               |
| `KeyUp` is unreachable   | `KeyboardEvent::KeyUp` is declared at `hook.rs:62` and **never constructed anywhere in the crate**. It is dead API surface.                                |
| 250 ms in-callback block | `const TIMEOUT: Duration = Duration::from_millis(250)` at `hook.rs:22`, used at `hook.rs:221` as `response_rx.recv_timeout(TIMEOUT)` — inside `hook_proc`. |

Two things worse than the plan recorded, found while confirming it:

- The 250 ms wait is on **every key-down**, not only on keys it has a hotkey for.
  The send-and-wait happens before any hotkey matching. At a `LowLevelHooksTimeout`
  of 250 ms — a value users do set — that is not "uncomfortably close" to the
  silent-removal threshold, it is _at_ it.
- `hook.rs:218` calls `.unwrap()` on the channel send, i.e. **can panic inside a
  low-level hook callback**. It also takes two `RwLock` reads and a `Mutex` lock
  per event. Our callback measured 7.8 microseconds; this design is not comparable.

Our double-tap fires on the second key-**up**, which `win-hotkeys` structurally
cannot deliver. That alone is disqualifying.

### willhook 0.6.3 — right shape, one hard blocker

What works, contrary to the risk the plan flagged:

- **Delivers both key-down and key-up.** `details.rs:63-66` maps `WM_KEYDOWN` to
  `Down`, `WM_KEYUP` to `Up`, `WM_SYSKEYDOWN` to `Down(System)`, `WM_SYSKEYUP` to
  `Up(System)`.
- **Distinguishes Shift sides.** `details.rs:198-199` maps `VK_LSHIFT` to
  `LeftShift` and `VK_RSHIFT` to `RightShift`.

The blocker:

- **`dwExtraInfo` is not surfaced anywhere.** The only injection-related field on
  its event types is `is_injected: Option<IsEventInjected>` (`event.rs:37`), and
  `details.rs:48` shows it is derived from the `LLKHF_INJECTED` **flag bit**, not
  from `dwExtraInfo`.

That is decisive, and precisely because of a deliberate design decision in this
plan. We must filter _our own_ injected `Ctrl+C` by tag while continuing to accept
other injected input, because keyboard remappers — PowerToys Keyboard Manager,
AutoHotkey — deliver genuine user intent that way. With only a boolean
`is_injected` the options are:

1. accept all injected input, and feed our own `Ctrl+C` back into the state
   machine, or
2. reject all injected input, and silently break the trigger for every remapper user.

Neither is acceptable, and neither is fixable from outside the crate.

### Conclusion

Neither crate implements double-tap, so on top of either we would write the same
state machine — which, at 38 unit tests and about 180 lines, is the part worth
owning anyway. `win-hotkeys` would additionally need its in-callback blocking and
missing key-ups worked around; `willhook` would need a source change to expose
`dwExtraInfo`, and has had no release since December 2023. Hand-rolling costs
roughly 300 lines of Win32 and removes both dependencies from the trust boundary
of the hottest, most failure-prone code in the product.

---

## Corrections to dsgn-001 and to the task plan

These are things the plan got wrong or did not anticipate. None invalidates a
binding decision; all of them would have cost time later.

### 1. GetCurrentPatternAs has the same null-out-parameter trap as GetSelection

**Observed live**, against `msedge.exe`: `UiaNoTextPattern { hresult: 0 }`.

The plan identified, carefully and correctly, that `GetSelection` returns an `Err`
whose `code()` is `S_OK` (0) rather than `E_POINTER`, because windows-rs converts a
null out-parameter into `Error::empty()`. It did **not** anticipate that
`GetCurrentPatternAs` behaves identically — it too returns `Err` with HRESULT 0
when the provider reports no pattern, rather than the documented
`UIA_E_NOTSUPPORTED` (0x80040204).

Had the implementation matched only on `UIA_E_NOTSUPPORTED`, as the plan text
implied, **the most common browser case would have landed in the catch-all
`UiaError { hresult: 0 }` bucket** — an "error" carrying a success code, which is
exactly the failure mode the plan warned about for `GetSelection` and then walked
into one call earlier. Both call sites now accept either shape.

This is the single most useful thing the spike has found so far, and it was found
by accident.

### 2. A process whose token cannot be read is NOT elevated

`Foreground::elevated` gates the entire cascade — `true` short-circuits to
`ForegroundElevated` and nothing else runs. The first implementation followed the
plan rule ("if `OpenProcess` fails with `ERROR_ACCESS_DENIED`, treat that as
elevated") but over-applied it to a _token_ read failure as well.

The probe above shows why that is wrong: Discord and `audiodg.exe` refuse
`OpenProcessToken` while running at **medium** integrity. They are hardened, not
elevated. Folding that into "elevated" would have silently disabled capture for
ordinary applications, and the symptom — "Copper does nothing in Discord" — gives
no hint of the cause.

Three states are now kept apart, with a unit test on each:

| State                            | Meaning                       | elevated                                  |
| -------------------------------- | ----------------------------- | ----------------------------------------- |
| `Integrity::Level(rid)`          | RID read from the token       | `rid > ours`                              |
| `Integrity::ProcessInaccessible` | `OpenProcess` itself denied   | `true` (per dsgn-001)                     |
| `Integrity::TokenUnreadable`     | Process opened, token did not | **`false`** — proceed and let UIPI answer |

### 3. An unsigned binary carrying uiAccess="true" cannot be launched at all

Attempting to start the unsigned `uiaccess-test.exe` via `CreateProcess` fails
with **`ERROR_ELEVATION_REQUIRED` (740)**. It does not launch with uiAccess quietly
denied; it does not launch.

dsgn-001 says: _"Dev builds run unsigned from the dev tree, so uiAccess is inactive
in dev."_ The conclusion is right but the mechanism is not — that state is only
reachable if the **dev build omits the manifest entirely**, not merely goes
unsigned. `uiaccess-test` now has a default-on `uiaccess` Cargo feature; building
`--no-default-features` produces a launchable binary that correctly reports
`token UIAccess: no`, which is how this was confirmed in both directions.

**Action for Phase 4 / task-009:** the uiAccess manifest must be conditional on the
release build. A dev build that inherits it will not start.

### 4. With uiAccess active, elevated must stop being a hard short-circuit

uiAccess exists precisely so a medium-integrity process _can_ read a
high-integrity one. Once it is active, short-circuiting on
`target_integrity > our_integrity` blocks the exact case uiAccess was enabled to
serve. `uiaccess-test` deliberately bypasses the short-circuit for this reason.

**Action for Phase 4:** gate the `ForegroundElevated` short-circuit on whether our
own token has the UIAccess flag, rather than on the integrity comparison alone.

### 5. Two windows crate features are missing from dsgn-001 list

- **`Win32_Graphics_Gdi`** is required. windows-rs gates `RegisterClassW` and
  `WNDCLASSW` behind it (`WNDCLASSW` embeds `HICON`/`HCURSOR`/`HBRUSH`), and the
  clipboard owner window cannot be created without registering a class. The owner
  window is itself non-negotiable — it is the fix for the most consequential
  defect the external review caught.
- **`CF_UNICODETEXT`** lives behind `Win32_System_Ole`. Rather than pull the whole
  OLE surface in for one ABI-fixed integer, it is defined locally as `13` with a
  comment. Worth knowing before someone adds the feature by reflex.

### 6. Windows focus-stealing prevention breaks naive test automation

`WScript.Shell.AppActivate` silently failed to bring Notepad forward during an
automated run, and the capture measured Microsoft Edge instead — injecting
`Ctrl+C` into the wrong application. `verify-cascade.ps1` now verifies the
foreground process against the target and **aborts rather than injecting** if they
disagree. Anyone scripting against this spike needs the same guard.

---

## Per-application results

One row is filled from an incidental run. The remaining rows need a person; the
procedure is in MANUAL VERIFICATION PENDING below.

| Target                      | Case                                 | Outcome              | UIA stage                      | Clipboard stage                | uia_ms | seq delay | Notes                                                                                                                           |
| --------------------------- | ------------------------------------ | -------------------- | ------------------------------ | ------------------------------ | ------ | --------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Microsoft Edge              | no selection, browser chrome focused | `ClipboardUnchanged` | `UiaNoTextPattern` (hresult 0) | sequence never moved in 200 ms | 29     | none      | `Ctrl+C` with no selection was **inert** — the sequence number did not move at all. Total attempt 230 ms. UIA object init 3 ms. |
| Notepad                     | editable / selected                  |                      |                                |                                |        |           |                                                                                                                                 |
| Notepad                     | no selection                         |                      |                                |                                |        |           |                                                                                                                                 |
| Windows Terminal            | selection (line)                     |                      |                                |                                |        |           |                                                                                                                                 |
| Windows Terminal            | selection (Alt-drag box)             |                      |                                |                                |        |           |                                                                                                                                 |
| Windows Terminal            | no selection                         |                      |                                |                                |        |           | **Sends an interrupt to whatever is running.**                                                                                  |
| Chrome                      | input element selection              |                      |                                |                                |        |           |                                                                                                                                 |
| Chrome                      | page text selection                  |                      |                                |                                |        |           |                                                                                                                                 |
| Chrome                      | no selection                         |                      |                                |                                |        |           |                                                                                                                                 |
| VS Code                     | editor selection                     |                      |                                |                                |        |           |                                                                                                                                 |
| VS Code                     | editor selection, Narrator on        |                      |                                |                                |        |           |                                                                                                                                 |
| VS Code                     | no selection                         |                      |                                |                                |        |           | **`editor.emptySelectionClipboard` defaults true — expect a false capture of the whole line.**                                  |
| Cursor                      | editor selection                     |                      |                                |                                |        |           | Confirm the UI shell first.                                                                                                     |
| Cursor                      | editor selection, Narrator on        |                      |                                |                                |        |           |                                                                                                                                 |
| Cursor                      | no selection                         |                      |                                |                                |        |           |                                                                                                                                 |
| Windows Terminal (elevated) | selection                            |                      |                                |                                |        |           | Expect `ForegroundElevated`, promptly.                                                                                          |

### Latency

| Measurement                                               | Value                     |
| --------------------------------------------------------- | ------------------------- |
| UIA automation-object creation (first, once per thread)   | **3 ms**                  |
| UIA read, Microsoft Edge                                  | **29 ms** (budget 250 ms) |
| Clipboard polling window (configured)                     | 200 ms                    |
| Clipboard end-to-end, failed attempt (Edge, no selection) | **230 ms**                |
| Hook callback, worst case                                 | **0.0078 ms**             |

The end-to-end figure is stated separately from the polling window deliberately.
200 ms bounds the _poll only_; the worst-case fallback also includes up to ~1 s of
`OpenClipboard` retry, up to 300 ms of modifier-release waiting, and two further
clipboard sessions for snapshot and restore. **The measured worst case so far is
230 ms, but that path did not exercise the retry or modifier branches.** The
harness prints `worst attempt using the clipboard` on shutdown; record that figure
after the manual pass, because it is the number Phase 4 budget must be set from.

---

## Failure taxonomy coverage

Acceptance criterion 5 requires every variant to end as **observed**, **forced** or
**unverified with a reason**. The harness prints this reconciliation on Ctrl+C, so
it is a five-second check at the end of a session rather than a trawl through the
JSONL.

| Variant                   | State                     | Evidence / how to reach it                                                                                                                                   |
| ------------------------- | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `Success { Uia }`         | pending                   | Any selection in Notepad.                                                                                                                                    |
| `Success { Clipboard }`   | pending                   | A selection where UIA returns nothing.                                                                                                                       |
| `NoForegroundWindow`      | **unverified — expected** | No reliable force is known; it occurs transiently during window-switch races. dsgn-001 accepts this.                                                         |
| `ForegroundElevated`      | pending                   | Elevated Windows Terminal.                                                                                                                                   |
| `ForegroundChanged`       | pending                   | Alt-Tab during the modifier wait; easiest with `--clipboard-timeout-ms 2000`.                                                                                |
| `UiaUnavailable`          | **unverified**            | Requires `CoCreateInstance(CUIAutomation8)` to fail, which does not happen on a healthy machine.                                                             |
| `UiaNoTextPattern`        | **OBSERVED**              | Microsoft Edge, browser chrome focused. HRESULT 0, not `UIA_E_NOTSUPPORTED` — see correction 1.                                                              |
| `UiaNoSelectionSupport`   | pending                   | A control with `SupportedTextSelection_None`.                                                                                                                |
| `UiaForeignElement`       | pending                   | Hard to force deliberately; watch for it during the matrix.                                                                                                  |
| `UiaEmptySelection`       | pending                   | Caret in Notepad with nothing selected.                                                                                                                      |
| `UiaTimeout`              | pending                   | `--uia-timeout-ms 1`.                                                                                                                                        |
| `UiaError`                | pending                   | Close the target window immediately after triggering.                                                                                                        |
| `ModifierHeld`            | pending                   | Hold Ctrl while triggering.                                                                                                                                  |
| `SendInputFailed`         | pending                   | Needs the elevation short-circuit temporarily removed — see the note below.                                                                                  |
| `ClipboardBusy`           | pending                   | Hold the clipboard open from a second process across a capture.                                                                                              |
| `ClipboardUnchanged`      | **OBSERVED**              | Microsoft Edge, no selection.                                                                                                                                |
| `ClipboardEmptyText`      | pending                   | Explorer with a _file_ selected: `Ctrl+C` puts `CF_HDROP` on the clipboard, so the sequence moves but `CF_UNICODETEXT` is absent.                            |
| `ClipboardSnapshotFailed` | pending                   | Rare; watch for it.                                                                                                                                          |
| `ClipboardRestoreSkipped` | pending                   | Copy something manually during the polling window. Easiest with `--clipboard-timeout-ms 3000`.                                                               |
| `ClipboardRestoreFailed`  | **not observed — good**   | This one means **user-visible data loss**. It has not occurred in any run. If it ever does, that is a finding in its own right and is logged at error level. |

Note on `SendInputFailed`: the cascade short-circuits to `ForegroundElevated`
before reaching the injection, so against an elevated target this variant is not
naturally reachable. Reaching it needs the short-circuit temporarily removed. That
is worth doing once, because a partial `SendInput` insert is the failure that
leaves **Ctrl stuck down system-wide**; the recovery key-up path deserves one real
exercise.

---

## MANUAL VERIFICATION PENDING

Everything below needs a person at the machine. Each block is self-contained.

> **Before you start.** This pass deliberately fires `Ctrl+C` at applications with
> nothing selected. Close or park anything you care about. In particular **do not
> run the Windows Terminal cases against a shell with a build, dev server, or long
> query running** — the no-selection case sends that process an interrupt, and
> restoring the clipboard does not undo an application-side action.
>
> **Enable clipboard history first**: Settings > System > Clipboard > Clipboard
> history > On. It is off by default on Windows 11, and criterion 8 tested against
> a disabled history proves nothing.

### 1. Per-application matrix (criteria 4, 6, 9)

```powershell
cd spike
cargo run --manifest-path capture/Cargo.toml -- --note "start"
```

While it runs, type a line into the console to relabel subsequent records — that is
how each row gets tagged without guessing afterwards.

For each of **Notepad, Windows Terminal, Chrome, VS Code, Cursor**, run three cases:

1. text selected in an editable field,
2. text selected in a non-editable/static region where the app has one,
3. nothing selected.

Type the label (e.g. `chrome / page text / selected`), focus the target, make the
selection, then **double-tap Shift**. Fifteen rows minimum.

Application-specific additions:

- **Chrome** — test an input-element selection and a plain page selection as
  separate cases. Note whether the _first_ UIA query after Chrome starts behaves
  differently from later ones (accessibility-tree activation lag; the harness
  reports `uia_first` per process on shutdown). Record `chrome.exe` total memory in
  Task Manager **before and after** the first query, so the accepted Chromium
  accessibility cost has a number attached (R-Q25).
- **Windows Terminal** — test a normal line selection _and_ an Alt-dragged box
  selection. The provider is known to report box selections as if they were line
  selections; confirm or refute.
- **VS Code and Cursor** — run each pass twice, once normally and once **with
  Narrator running**, since Electron gates its accessibility tree on detecting
  assistive technology. Before testing Cursor, confirm which UI shell it presents
  and record it — do not assume it is still the VS Code editor surface.

**For each of the five no-selection cases, record what happened to the target
application, not just what we captured** (criterion 9). Specifically: did VS Code
and Cursor copy the current line and produce a false capture of text the user never
selected? Did Windows Terminal interrupt its running process? These are the results
most likely to change a design decision — record them plainly even where the answer
is inconvenient.

Fill in the per-application table above as you go, then paste the harness shutdown
summary into this file.

### 2. Trigger negative cases (criterion 2)

With the harness running, confirm each of these produces **zero** capture attempts:

| Action                                               | Expected |
| ---------------------------------------------------- | -------- |
| One Shift tap                                        | nothing  |
| Hold Shift for ~1 s, release                         | nothing  |
| `Shift+A`                                            | nothing  |
| Two Shift taps deliberately slower than 400 ms apart | nothing  |
| Left-Shift then right-Shift                          | nothing  |

The boundary, auto-repeat and tick-rollover cases are already covered by unit tests
(they cannot be reproduced reliably with fingers). The positive path is covered by
the latency example, which produced exactly 500 triggers from 500 injected
double-taps.

### 3. Win+V clipboard history (criterion 8)

Run a capture cycle that **actually used the clipboard path** — a UIA success writes
nothing and proves nothing here. Check for `strategy: clipboard` in the console line.

Then press `Win+V` and record **two results separately**:

| Question                                             | Expected                                                                                   | Result |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------ | ------ |
| Did **our restore** produce a history entry?         | **No** — hard pass/fail. This is fully within our control via the three exclusion formats. |        |
| Did **the target application own copy** produce one? | Possibly yes. We cannot prevent it and are measuring it, not preventing it (R-Q3).         |        |

If the target entry appears, say so plainly here and note what it means for the
dsgn-001 privacy premise — which is already weakened to _"Copper own writes never
seed history"_.

You can also check our write in isolation, with no target application involved:

```powershell
cargo run --manifest-path capture/Cargo.toml --example clipboard-selftest
```

Then `Win+V`: neither the test payload nor the restore should appear.

### 4. Elevated window (criteria 5, 6)

Launch Windows Terminal **as administrator**, with the spike running unelevated.
Select text, double-tap Shift.

Expected: `ForegroundElevated`, arriving **promptly** — not as a hang, an empty
success, or a crash. Record the elapsed time.

### 5. Forced failure variants (criterion 5)

Each of these is a separate short run. `cascade-selftest` is usually easier than
performing a double-tap at the right moment.

```powershell
cd spike

# UiaTimeout
cargo run --manifest-path capture/Cargo.toml -- --uia-timeout-ms 1 --note "forced/UiaTimeout"

# ClipboardEmptyText - Explorer with a FILE selected. Ctrl+C puts CF_HDROP on the
# clipboard, so the sequence moves but CF_UNICODETEXT is absent.
cargo run --manifest-path capture/Cargo.toml --example cascade-selftest -- --delay-ms 5000

# ModifierHeld - hold Ctrl down while triggering.
cargo run --manifest-path capture/Cargo.toml -- --note "forced/ModifierHeld"

# ClipboardRestoreSkipped - long polling window, then copy something manually during it.
cargo run --manifest-path capture/Cargo.toml --example cascade-selftest -- --delay-ms 3000 --clipboard-timeout-ms 3000

# UiaError - close the target window immediately after triggering.
cargo run --manifest-path capture/Cargo.toml -- --note "forced/UiaError"
```

**ClipboardBusy** needs a second process holding the clipboard open across a
capture. In a separate PowerShell window, P/Invoke `OpenClipboard(IntPtr.Zero)`,
trigger a capture in the first window, wait about two seconds, then
`CloseClipboard()`. The capture should record `ClipboardBusy` with the attempt
count and elapsed time in its detail field.

Then reconcile against the harness own observed/unobserved summary on Ctrl+C and
update the taxonomy table. Anything still unreached must be listed as unverified
**with the reason**.

### 6. Sticky Keys interaction

Tapping Shift twice is also the Windows gesture that latches Sticky Keys. This is a
viability question for the chosen gesture, so record the result plainly even if it
is bad news.

1. Settings > Accessibility > Keyboard > Sticky keys > On (and enable the shortcut).
2. Run the harness, double-tap Shift.
3. Record: does our capture still fire? Does Sticky Keys latch? Does its
   confirmation dialog steal focus and change the foreground window out from under
   the capture — i.e. do you see `ForegroundChanged`?
4. Turn Sticky Keys off again.

If the dialog does steal focus, that is the exact scenario the cascade foreground
revalidation exists for, and seeing it fire would be a good result rather than a
bad one.

### 7. Tauri probe (criterion 10)

```powershell
cd spike
cargo run --manifest-path tauri-probe/src-tauri/Cargo.toml
```

The probe window logs triggers in a table and prints them to the console. Test
**three states**, and all three must register for the architecture to hold:

| State                | How                                                            | Result |
| -------------------- | -------------------------------------------------------------- | ------ |
| Probe window focused | Click the probe window, double-tap Shift                       |        |
| Another app focused  | Click any other window, double-tap Shift                       |        |
| Probe window hidden  | Press "Hide for 12 seconds", double-tap Shift while it is gone |        |

Then repeat all three with each `device_event_filter` setting — this is the setting
that resolved the upstream issue. The env var accepts `never`, `always`, or
`unfocused` (which is also Tauri default when the variable is unset):

```powershell
$env:COPPER_DEVICE_EVENT_FILTER = "never";     cargo run --manifest-path tauri-probe/src-tauri/Cargo.toml
$env:COPPER_DEVICE_EVENT_FILTER = "always";    cargo run --manifest-path tauri-probe/src-tauri/Cargo.toml
$env:COPPER_DEVICE_EVENT_FILTER = "unfocused"; cargo run --manifest-path tauri-probe/src-tauri/Cargo.toml
```

Record whether the setting makes **any** difference for a bare modifier.

**System-key control.** With the probe window focused, press `Win`, `Alt+Tab` and
`Ctrl+Shift+Esc`. These are the OS-reserved combinations the upstream report was
actually about; the probe logs them as `system key` rows. Record whether the
reported symptom reproduces at all on Tauri 2.11.5. This distinguishes "our case is
fine and theirs was real" from "the issue no longer reproduces".

Only if the Tauri probe **fails**: build a bare `tao` window as a second probe to
isolate whether the event loop or WebView2 is responsible. Skip entirely if it
passes.

### 8. Criterion 12 — uiAccess validation

This gates dsgn-001 Phase 4 per-machine install plus admin-elevation strategy. If
it fails, uiAccess cannot ship and Phase 4 must revert to unsigned/per-user,
blocking elevated windows outright.

**Step 1 — in an ELEVATED PowerShell** (certificate stores and `%ProgramFiles%`
both need admin):

```powershell
pwsh -File spike\scripts\uiaccess-setup.ps1
```

That builds the release binary, generates a self-signed code-signing certificate,
installs it into **both** `LocalMachine\Root` and `LocalMachine\TrustedPublisher`
(Root alone is not enough — Windows checks Trusted Publishers for the uiAccess
decision specifically), copies the binary to `%ProgramFiles%\copper-test\`, signs it
in place with `signtool`, and verifies the signature.

**Step 2 — in a NORMAL, UNELEVATED shell.** This matters: uiAccess exists so the
process does _not_ need elevation, and running the test elevated would grant high
integrity for an unrelated reason and prove nothing. The tool warns you if you do.

1. Open Windows Terminal **as administrator** and type some text into it.
2. In an ordinary shell, run `%ProgramFiles%\copper-test\uiaccess-test.exe`.
3. During the countdown, focus the elevated terminal and select text.

**What to look for:**

| Field              | Required value                         |
| ------------------ | -------------------------------------- |
| `token UIAccess`   | `YES - uiAccess is ACTIVE`             |
| `token elevated`   | `no`                                   |
| `higher than ours` | `true` (the target really is elevated) |
| result             | `READ SUCCEEDED`                       |

The tool prints its own verdict combining all four. Record the whole output here.

**Note:** if the binary refuses to launch with "requires elevation", that means the
signature or location requirement is not satisfied — see correction 3.

To undo everything, run the same script with the `-Remove` switch.

**If this fails,** state here which dsgn-001 decision it invalidates: the
per-machine install under `%ProgramFiles%` with admin elevation at install time,
and the Authenticode signature strategy. The fallback is per-user install with
elevated-window capture unavailable, and the Q27 elevated-window notice becomes the
permanent behaviour rather than a dev-build-only one.

---

## Open questions and flags for the orchestrator

### UIA may not be the primary strategy

_(dsgn-001 Open Question 7, and the spike highest-uncertainty area.)_

The only UIA data point so far is Microsoft Edge returning **`UiaNoTextPattern`**
for its browser chrome. That is one unrepresentative sample and should not be
over-read — the accessibility tree may not have been active, and the focused
element was chrome rather than page content.

But it is worth flagging now because of what the manual pass might show. If Chrome,
VS Code and Cursor also return nothing selection-shaped, then **the clipboard path
is what actually serves the primary editing targets**, and the dsgn-001 framing of
UIA as the primary strategy is worth revisiting — the accepted Chromium
accessibility cost would be being paid for very little. The blind cascade handles
it either way, so there is no architectural break; the question is whether the
cascade order is still the right default.

**Not a Phase 0 decision.** Recorded so the finding gets interpreted rather than
merely recorded.

### Design changes NOT made, per the plan instruction

The plan is explicit that Phase 0 produces evidence and does not redesign the
cascade. Two things were therefore observed and left alone:

- **The cascade does not terminate on a trusted-empty UIA answer** (R-Q2), so the
  injected `Ctrl+C` runs even where it has known side effects — a false whole-line
  capture in VS Code and Cursor, an interrupt in Windows Terminal. Criterion 9
  gathers the evidence; changing when the fallback runs would alter a binding
  Decision in dsgn-001.
- **Multi-range selections are concatenated with no separator**, per the design
  "concatenate the ranges GetText(-1) in order". For VS Code multi-cursor selection
  a newline separator would almost certainly be more useful. Flagged rather than
  changed.

### CoCancelCall remains unverified

_(dsgn-001 Open Question 10.)_ The spike uses thread abandonment, which works
regardless. Whether the UIA proxy honours `CoEnableCallCancellation` /
`CoCancelCall` was not tested — proving it out belongs to Phase 4 production design
rather than to a throwaway spike. Recorded so Phase 4 tests it rather than
inheriting the earlier incorrect claim that cancellation does not exist.

### Divergences from the task Design section

Small, deliberate, and listed so review can check them rather than discover them:

| Design said                                        | Built as                          | Why                                                                                                                                 |
| -------------------------------------------------- | --------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `ShiftSide`                                        | `KeySide`                         | The trigger key is configurable, so a Shift-specific name would be wrong.                                                           |
| `pub fn attempt(fg: &Foreground) -> AttemptRecord` | Method on `Capturer`              | The cascade needs the long-lived UIA service and the clipboard owner window; neither can be a global.                               |
| `write_excluded(entries)`                          | `write_excluded(owner, entries)`  | Writing requires the owner window; making it a parameter makes that impossible to forget.                                           |
| `uia: Option<CaptureOutcome>` per stage            | `Stage { NotRun, Ok, Failed(_) }` | `Option` conflates "passed" with "never ran", which criterion 5 needs distinguished.                                                |
| 11 windows features                                | 12                                | `Win32_Graphics_Gdi` added; see correction 5.                                                                                       |
| (not specified)                                    | 4 examples, 2 scripts             | Several criteria are about the cascade rather than the hook; forcing them through a hand-performed double-tap is needlessly fiddly. |

Extra payload fields were added to some `CaptureOutcome` variants (`reason`,
`hresult`, `budget_ms`, and so on). The variant **set** is exactly the 19 the
design lists.

### Restore is lossy for non-text formats — a real data-loss vector for Phase 4

Called out separately because it is easy to miss behind the "synthesized formats
are not real losses" note above.

`snapshot()` captures only `CF_UNICODETEXT` and `"HTML Format"`. `restore()` goes
through `write_excluded`, which calls `EmptyClipboard` first. So if the user has an
**image, a file list (`CF_HDROP`), or any OLE-backed format** on the clipboard when
a capture takes the clipboard fallback path, that content is **destroyed** — the
restore puts back text and HTML only.

In the spike this is in scope and deliberate: the design says to record what we
would be dropping rather than attempt faithful restoration, and
`unrestorable_formats` in `findings.jsonl` does exactly that. Note that the
`ClipboardEmptyText` force procedure (Explorer with a file selected) walks straight
into this case, so expect to see it during the manual pass.

For **Phase 4 this is not acceptable as-is**, and it is not visible from the
"capture silently succeeded" user experience. Options, in rough order of cost:
restore `CF_DIB`/`CF_DIBV5` and `CF_HDROP` too; or skip the restore entirely when
the snapshot contained any format we cannot reproduce, leaving the target
application's copy on the clipboard rather than replacing it with a lossy
reconstruction. The second is much cheaper and arguably more honest. Flagged for
the orchestrator rather than decided here — it changes clipboard behaviour, which
is a binding design area.

---

## External review — codex, 2026-08-03

Run against the whole spike. **Note on transport:** the Converse MCP server was not
connected to the session (`mcp__converse__chat` absent from the tool list, though
present in the settings allowlist), so the review was run through the `codex` CLI
0.146.0 directly — `codex exec --sandbox read-only`. Same model, different
transport. No model substitution was made.

It returned six findings. All six were verified against the code before being
acted on. **Three were real bugs that could destroy user clipboard data**, and the
reviewer was right that they made criterion 7 unmet.

### Applied

**1. High — the restore's sequence check was not atomic with the write.** The old
code sampled `seq()` and then called `restore()`, which can spend up to a second
retrying `OpenClipboard` before it reaches `EmptyClipboard`. Anything the user
copied during that window was destroyed. Verified: the window is real and bounded
only by the ~1 s acquisition budget.

_Fix:_ `write_excluded` now takes `expected_seq` and re-checks the sequence
**inside the open write session**, where no other process can interleave, and
returns `ClipboardError::Superseded` instead of emptying. That is the only check
that means anything — once we hold the clipboard, nobody else can write.

**2. High — a foreign-owner write was restored over.** The design mandates
treating the owner check as a _soft_ signal for the capture, and that was
implemented. But the same soft treatment was wrongly applied to the **restore**:
if the observed clipboard change came from a process other than the target — i.e.
almost certainly something the user copied themselves — the code restored the
older snapshot straight over it.

_Fix:_ the owner check is now soft for the capture and **hard for the restore**.
`poll_for_sequence_change` reports whether the observed write was foreign, the
owner is re-checked after the read, and either signal withholds the restore and
records `ClipboardRestoreSkipped`. The capture still takes the observation, so the
binding design decision is untouched.

**3. High — a partial `SendInput` skipped the restore entirely.** The old code
returned `SendInputFailed` immediately after sending the recovery key-ups. But if
Ctrl-down and C-down went in and the key-ups did not, the target may already have
copied — leaving its content on the user's clipboard with the snapshot never put
back. The recovery key-ups addressed the stuck-modifier risk and nothing else.

_Fix:_ the poll/read/restore sequence now runs regardless of a short insert; the
outcome is still recorded as `SendInputFailed`.

**4. Medium — the configured UIA timeout did not bound the UIA stage.** Thread
creation had its own separate 5 s wait outside `--uia-timeout-ms`, so even
`--uia-timeout-ms 1` could take five seconds — and it stalls the worker thread,
which is also responsible for pumping the clipboard owner window.

_Fix:_ `UiaService::warm_up()` is called from `Capturer::new`, moving COM and
automation-object creation out of the capture path entirely, and the remaining
init bound is cut from 5 s to 1 s (measured init is 3 ms). A replacement spawn
after an abandonment still adds up to 1 s; that is now documented on `read`
rather than silent.

**5. Medium — the clipboard evidence was overstated.** This document claimed the
original contents were restored "exactly", but the self-test compared decoded
Unicode strings and the machine's clipboard happened to contain no HTML — so
neither raw-byte identity nor the HTML path was actually proven.

_Fix:_ the self-test now seeds text **and** HTML, compares raw payload bytes, and
asserts the overwrite really dropped the HTML first so the restore cannot pass
trivially. The table above reports the stronger result. This was the most useful
finding of the six: the claim was not false, but it was not supported either.

**6. Low — criterion 4's per-stage timing was incomplete.** No foreground or
restore duration was recorded. Both are now in `Timings` and in the JSONL.

### One fix went further than the reviewer suggested

While applying #1 it became clear that expecting the _pre-read_ sequence value
would withhold the restore every time we capture from an application that uses
**delayed rendering** — reading a delayed-rendered format makes the owner call
`SetClipboardData`, which bumps the sequence. That is most of the interesting
targets. The expected value is therefore sampled **after** the read, which is
still safe: anything written after that point is caught by the in-session check.
Worth watching for during the manual pass — an unexpectedly high
`ClipboardRestoreSkipped` rate would point back here.

### Not applied

The reviewer noted that `TOKEN_MANDATORY_LABEL` is read through a `Vec<u8>`,
which relies on the Windows allocator's alignment in practice rather than
guaranteeing it. Correct observation, and it ranked it below the clipboard
defects itself. Left as-is for a throwaway spike; worth an explicitly aligned
allocation when this moves into the product at Phase 4.

### What the review confirmed

Worth recording, because these were the parts most likely to be wrong:

- The clipboard **owner-window** sequence is correct: write sessions use a real
  owner, `EmptyClipboard` follows `OpenClipboard`, successful `SetClipboardData`
  transfers ownership, failed calls free their `GMEM_MOVEABLE` allocation, and no
  panic or early-return path bypasses `CloseClipboard`.
- The hook callback's `KBDLLHOOKSTRUCT` dereference follows the callback
  contract; `try_borrow_mut` means re-entry cannot panic; queue creation precedes
  thread-id publication; `Drop` only joins after a successful quit post.
- **All four UIA thread-abandonment conditions are honoured** — only plain data
  crosses the channel, abandonment drops the request sender, a late-returning
  thread exits on the disconnected channel, and no `JoinHandle` is retained.
- Nothing materially beyond Phase 0 scope was built.

### Status after the fixes

Criterion 7 was **not met** at review time. The three data-loss paths are now
closed and the automated evidence is stronger than it was. Criterion 7 still
cannot be marked fully met until the manual pass exercises a real target
application, but the code no longer contains a known path that destroys the
user's clipboard.
