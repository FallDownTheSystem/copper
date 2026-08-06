//! In-app updates.
//!
//! The whole updater surface is here and the WebView never touches the plugin:
//! it invokes the three commands below, and those are ordinary application
//! commands, allowed for every window without a capability entry. Nothing grants
//! `updater:*` to the panel, so the plugin's own four commands stay unreachable
//! from JavaScript and `removeUnusedCommands` prunes them out of the binary
//! entirely. That is the design, not an oversight: if a permission error ever
//! appears at runtime it means the frontend started calling the plugin directly,
//! and the fix is there rather than in `capabilities/default.json`.
//!
//! Two facts about the plugin shape everything below, and both are
//! implementation details rather than API — which is why `tauri-plugin-updater`
//! is pinned exactly:
//!
//! - **The signature is verified during the download**, after the artifact has
//!   buffered, not during the check. A tampered artifact therefore fails in
//!   `install_update`, not in `check_for_update`.
//! - **The Windows install never returns.** `download_and_install` runs the
//!   `on_before_exit` hook, launches the installer through `ShellExecuteW`,
//!   discards the result, and calls `std::process::exit(0)` on the next line. No
//!   code after the await runs, no error comes back if the installer failed to
//!   start, and there is no torn-down-but-alive state to recover from. Relaunch
//!   afterwards is the installer's doing — passive mode passes `/R`, which the
//!   NSIS template's `.onInstSuccess` acts on — so `app.restart()` here would be
//!   dead code as well as unnecessary.

use std::sync::{
	atomic::{AtomicBool, Ordering},
	Mutex,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Download progress, emitted while `install_update` runs.
const PROGRESS_EVENT: &str = "update://progress";

/// A ceiling on the artifact download, which otherwise has none.
///
/// `check()` hardcodes `timeout: None` into the `Update` it builds, and
/// `download()` applies a timeout only when the field is set — so an unpatched
/// retained `Update` hands a stalled server an unbounded wait, and the panel
/// shows a progress bar that never moves and never fails. The field is public, so
/// the fix is to set it.
///
/// Generous rather than tight, because reqwest's is a *total* deadline covering
/// the whole response body and not an idle timeout. Ten minutes is roughly
/// 10 KB/s for an installer this size: slow enough to survive a bad connection,
/// short enough that a dead one eventually says so.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// How much has to accumulate before another progress event is emitted.
///
/// reqwest yields chunks in the tens of kilobytes, so emitting per chunk would
/// put hundreds of events through the IPC boundary for a single download to move
/// one progress bar. At 64 KiB a several-megabyte installer costs a manageable
/// number of events and still updates often enough to look continuous.
const PROGRESS_STRIDE: usize = 64 * 1024;

/// What the check hands the WebView.
///
/// Deliberately not the `Update` itself: that value is the thing the install
/// consumes, and handing a copy of its innards to the frontend would invite a
/// second source of truth for which version was approved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
	/// The version the endpoint is offering.
	pub version: String,
	/// The version running now, so the row can state both ends of the change
	/// without the frontend keeping its own copy of either.
	pub current_version: String,
	/// The manifest's `notes`, plain text.
	pub notes: Option<String>,
	/// The manifest's `pub_date`, narrowed to its date. The full timestamp is
	/// RFC 3339 with an offset, which is more than a settings row can use.
	pub date: Option<String>,
}

/// Download progress. `total` is optional because `Content-Length` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
	downloaded: usize,
	total: Option<u64>,
}

/// The `Update` the user approved, held between the check and the install.
///
/// Retained rather than re-derived. Re-checking inside the install would cost a
/// second manifest request and, worse, open a window in which the user approves
/// version B and version C is what installs.
///
/// The lock is held only for the moment it takes to store or take the value, and
/// never across an `.await`. A `std::sync::MutexGuard` held across one blocks an
/// executor thread and makes the command future non-`Send`, which Tauri's async
/// command machinery rejects at compile time. Serialising the two operations is
/// [`UpdateGate`]'s job instead, deliberately separate from this.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

/// The in-flight gate: at most one check or install at a time.
///
/// Non-blocking by construction. A second concurrent call is told "already in
/// progress" straight away rather than queueing behind the first, because
/// queueing behind a download means a button press that appears to do nothing for
/// a minute and then does something the user has forgotten asking for.
#[derive(Default)]
pub struct UpdateGate(AtomicBool);

impl UpdateGate {
	/// True if this caller now holds the gate.
	fn try_acquire(&self) -> bool {
		self.0
			.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
			.is_ok()
	}

	fn release(&self) {
		self.0.store(false, Ordering::Release);
	}
}

/// Holds the gate for as long as it is alive, and releases it on every exit path
/// — including the `?` returns below, which is the point of it being a guard
/// rather than a pair of calls.
///
/// Owns an `AppHandle` rather than borrowing the state, so it is `Send + 'static`
/// and can be held across the `.await`s in both commands.
struct InFlight(AppHandle);

impl InFlight {
	fn acquire(app: &AppHandle) -> Option<Self> {
		app.state::<UpdateGate>()
			.try_acquire()
			.then(|| Self(app.clone()))
	}
}

impl Drop for InFlight {
	fn drop(&mut self) {
		self.0.state::<UpdateGate>().release();
	}
}

const BUSY: &str = "Copper is already working on an update. Give it a moment.";
const NOTHING_PENDING: &str =
	"There's no update ready to install. Check for updates first, then try again.";

/// Asks the endpoint whether there is a newer version, and remembers the answer.
///
/// The pending update is cleared **before** the request rather than in each
/// branch afterwards, so "no update" and every failure both leave the state
/// empty. Only a successful check that found something puts a value back.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
	let _in_flight = InFlight::acquire(&app).ok_or(BUSY)?;
	take_pending(&app);

	// One builder, built here, and the `Update` it returns carries the
	// `on_before_exit` hook with it — `check()` clones the hook into the value.
	// That is what makes it impossible for the install to run without the
	// teardown: `install_update` consumes this `Update` rather than building a
	// second updater that could be missing it.
	let updater = app
		.updater_builder()
		.on_before_exit({
			let app = app.clone();
			move || before_exit(&app)
		})
		.build()
		.map_err(check_failed)?;

	let Some(update) = updater.check().await.map_err(check_failed)? else {
		return Ok(None);
	};

	let info = UpdateInfo {
		version: update.version.clone(),
		current_version: update.current_version.clone(),
		notes: update.body.clone(),
		// `date()` narrows the offset timestamp to `YYYY-MM-DD`. Reached through
		// the value rather than by naming `time::OffsetDateTime`, which would mean
		// adding a direct dependency on a crate this module needs one method from.
		date: update.date.map(|date| date.date().to_string()),
	};
	store_pending(&app, update);
	Ok(Some(info))
}

/// Installs the update the last check found. Does not check again.
///
/// On success this never returns: the plugin exits the process from inside
/// `download_and_install`. Everything after the await is the failure path.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
	let _in_flight = InFlight::acquire(&app).ok_or(BUSY)?;
	let mut retained = Retained::take(&app).ok_or(NOTHING_PENDING)?;
	retained.bound_download(DOWNLOAD_TIMEOUT);

	let result = retained
		.update()
		.download_and_install(
			{
				let app = app.clone();
				let mut downloaded = 0usize;
				let mut emitted = 0usize;
				move |chunk, total| {
					downloaded += chunk;
					// The last partial chunk deliberately goes unreported. Either the
					// process is about to be replaced by the installer, or verification
					// has just failed and the row is about to show an error — and a
					// progress bar resting at 99% for the moment in between is cheaper
					// than the shared counter a final emit would need.
					if downloaded - emitted < PROGRESS_STRIDE {
						return;
					}
					emitted = downloaded;
					emit_progress(&app, Progress { downloaded, total });
				}
			},
			|| {},
		)
		.await;

	// `retained` puts the update back on the way out unless this disarms it, so
	// the error path needs nothing of its own.
	result.map_err(install_failed)?;
	retained.installed();
	Ok(())
}

/// The version running now, for the settings row to show before any check.
///
/// Sourced from the binary rather than from `package.json`, which carries a
/// placeholder, or from a literal in the frontend, which would be a second copy
/// free to disagree with what is actually installed. The canonical value is
/// `package.version` in `src-tauri/Cargo.toml`; `tauri.conf.json` omits `version`
/// and inherits it.
/// `async` with nothing to await, because every command in this app is: one
/// uniform contract across the whole IPC surface, and `tests/commands.rs` asserts
/// that shape while parsing the signatures.
#[tauri::command]
pub async fn get_app_version(app: AppHandle) -> String {
	app.package_info().version.to_string()
}

// --- state helpers -----------------------------------------------------------

/// Locks through a poison rather than panicking, the same view `store::lock` and
/// `capture::lock` take: what is behind this lock is one owned value, and a panic
/// elsewhere cannot leave it in a shape that makes reading it dangerous.
/// Propagating the poison would turn one panic into an updater that can never run
/// again.
fn pending(app: &AppHandle) -> std::sync::MutexGuard<'_, Option<Update>> {
	let state = app.state::<PendingUpdate>();
	// The guard borrows the `Mutex`, which lives in managed state for the life of
	// the app rather than in the `State` wrapper, so it outlives this frame.
	let state: &PendingUpdate = state.inner();
	state
		.0
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn store_pending(app: &AppHandle, update: Update) {
	*pending(app) = Some(update);
}

fn take_pending(app: &AppHandle) -> Option<Update> {
	pending(app).take()
}

/// The approved `Update`, out of [`PendingUpdate`] for the length of an install
/// and put back unless the install actually consumed it.
///
/// Restoring on the error path alone is not enough, which is the whole reason
/// this is a guard rather than two calls. Tauri may **drop** a command future —
/// a WebView reload part-way through a multi-minute download is the realistic
/// way that happens — and a plain `take` would then strand the approved update
/// in a local that is about to be dropped. The pending slot would be empty with
/// nothing having failed, so nothing would put it back, and the settings row
/// would keep offering an install that Rust can no longer perform. This is the
/// discipline [`InFlight`] applies to the gate, applied to the value.
///
/// Defence in depth rather than the only protection: the row also offers a
/// "check again" action out of the error state, so a stranded update is
/// recoverable by hand either way.
struct Retained {
	app: AppHandle,
	/// `None` only after [`Self::installed`] has consumed the guard, or inside
	/// `Drop` — neither of which is observable from a method call below.
	update: Option<Update>,
}

impl Retained {
	fn take(app: &AppHandle) -> Option<Self> {
		take_pending(app).map(|update| Self {
			app: app.clone(),
			update: Some(update),
		})
	}

	fn update(&self) -> &Update {
		self.update
			.as_ref()
			.expect("the retained update is present until the guard is consumed or dropped")
	}

	/// Caps the artifact download, which the plugin otherwise leaves untimed.
	fn bound_download(&mut self, timeout: std::time::Duration) {
		if let Some(update) = self.update.as_mut() {
			update.timeout = Some(timeout);
		}
	}

	/// Disarms the put-back: the update is installed and must not be offered
	/// again. Unreachable on Windows, where the plugin exits the process from
	/// inside the install rather than returning — but the guard's contract should
	/// not depend on that, or a platform where it does return would keep offering
	/// an install that has already happened.
	fn installed(mut self) {
		self.update = None;
	}
}

impl Drop for Retained {
	fn drop(&mut self) {
		if let Some(update) = self.update.take() {
			store_pending(&self.app, update);
		}
	}
}

fn emit_progress(app: &AppHandle, progress: Progress) {
	if let Err(err) = app.emit(PROGRESS_EVENT, progress) {
		crate::diagnostics::log_error(&format!(
			"[copper] updater: could not report download progress: {err}"
		));
	}
}

// --- teardown ----------------------------------------------------------------

/// Runs just before the plugin replaces this process with the installer.
///
/// Two things about this hook are easy to get wrong and expensive to get wrong.
///
/// It **replaces** Tauri's own teardown rather than adding to it:
/// `updater_builder()` pre-installs a default hook that calls
/// `cleanup_before_exit()`, and the setter stores through `Option::replace`. A
/// custom hook that forgets the call silently drops Tauri's teardown, so
/// `cleanup_before_exit()` is last here and nothing touches a Tauri API after it,
/// which its own documentation requires.
///
/// And it is an *updater-path* hook, not a general exit hook. It does not run on
/// a crash, on a Task Manager kill, or on an uninstall started from Windows.
/// Copper's crash-safety comes from `editor::scavenge` at startup, not from here.
fn before_exit(app: &AppHandle) {
	crate::teardown(app);
	app.cleanup_before_exit();
}

// --- error wording -----------------------------------------------------------

/// The plugin's errors are written for a developer — "Could not fetch a valid
/// release JSON from the remote" — so each is given the one piece of context the
/// user actually needs, which is which half of the operation failed. Classifying
/// further would mean guessing at causes the error type does not distinguish.
fn check_failed(error: tauri_plugin_updater::Error) -> String {
	format!("Copper couldn't check for updates: {error}")
}

fn install_failed(error: tauri_plugin_updater::Error) -> String {
	format!("Copper couldn't install the update: {error}")
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The gate's whole contract: one holder, non-blocking refusal, reusable
	/// afterwards. Tested on the type rather than through a command because the
	/// commands need a running app and this is the part with the invariant in it.
	#[test]
	fn gate_admits_one_holder_at_a_time() {
		let gate = UpdateGate::default();

		assert!(gate.try_acquire(), "an idle gate admits the first caller");
		assert!(
			!gate.try_acquire(),
			"a second caller is refused rather than queued"
		);

		gate.release();
		assert!(gate.try_acquire(), "releasing makes the gate reusable");
	}

	/// A release without a matching acquire — which `Drop` cannot cause, but a
	/// future refactor could — leaves the gate open rather than wedged.
	#[test]
	fn gate_release_is_idempotent() {
		let gate = UpdateGate::default();
		gate.release();
		gate.release();
		assert!(gate.try_acquire());
	}

	#[test]
	fn update_info_serialises_camel_case() {
		let info = UpdateInfo {
			version: "0.1.1".into(),
			current_version: "0.1.0".into(),
			notes: Some("Fixes the thing.".into()),
			date: Some("2026-08-05".into()),
		};

		let json = serde_json::to_value(&info).expect("UpdateInfo serialises");
		assert_eq!(json["version"], "0.1.1");
		assert_eq!(json["currentVersion"], "0.1.0");
		assert_eq!(json["notes"], "Fixes the thing.");
		assert_eq!(json["date"], "2026-08-05");
	}

	/// The absent halves are `null`, not missing keys: the frontend narrows on
	/// `total === null` to choose an indeterminate indicator, and a missing key
	/// would arrive as `undefined` and take the wrong branch.
	#[test]
	fn optional_fields_serialise_as_null() {
		let info = UpdateInfo {
			version: "0.1.1".into(),
			current_version: "0.1.0".into(),
			notes: None,
			date: None,
		};
		let json = serde_json::to_value(&info).expect("UpdateInfo serialises");
		assert!(json["notes"].is_null());
		assert!(json["date"].is_null());

		let progress = serde_json::to_value(Progress {
			downloaded: 4096,
			total: None,
		})
		.expect("Progress serialises");
		assert_eq!(progress["downloaded"], 4096);
		assert!(progress["total"].is_null());
	}

	#[test]
	fn progress_carries_the_total_when_the_server_sent_one() {
		let progress = serde_json::to_value(Progress {
			downloaded: 4096,
			total: Some(8192),
		})
		.expect("Progress serialises");
		assert_eq!(progress["total"], 8192);
	}
}
