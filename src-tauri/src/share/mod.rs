//! Device share: one note, one way, once.
//!
//! Two Windows machines, both the same person's, both running Copper. Select a
//! note, choose **Send to my other device**, and it appears on the other machine
//! — whether or not that machine is awake, and whether or not the two are on one
//! network.
//!
//! **Not a sync engine.** No shared document, no conflict resolution, no
//! continuous state. A relay the user deploys to their own free Cloudflare
//! account acts as a mailbox: the sender encrypts locally and posts the
//! ciphertext to the other device's mailbox, the reader polls its own, decrypts,
//! writes the notes into the open space and deletes the message. The relay only
//! ever holds ciphertext, and every stored message expires on its own even if
//! nobody collects it.
//!
//! **Delivery is at-least-once, never zero.** A note is committed to disk before
//! it is deleted from the relay, so a crash in the window between the two
//! delivers it twice. Every ambiguous case in this module resolves towards the
//! duplicate, because `Ctrl+Z` fixes one and nothing fixes a dropped note.
//!
//! **The whole feature is synchronous.** No `.await`, no Tokio task, no async
//! runtime: one named OS thread on the `Condvar` tick pattern `editor.rs`'s idle
//! sweeper already uses, and `ureq` for the HTTP, which has no runtime to
//! conflict with. While the feature is switched off the thread waits with no
//! timeout at all, so a disabled share costs not one wake-up.
//!
//! The module map:
//!
//! - [`crypto`] — HKDF derivation, `seal` and `open`. Pure, no I/O.
//! - [`protocol`] — what a sealed message carries, and its size ceiling.
//! - [`config`] — the shapes in `share.json`, and `resolve`.
//! - [`state`] — the one in-process owner of `share.json`.
//! - [`relay`] — the `Relay` trait and its `ureq` implementation.
//! - [`poll`] — the tick thread, the drain loop, the hole and poison rules.
//! - [`commands`] — the five `#[tauri::command]` wrappers.

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use copper_core::store::error::{Result, StoreError};
use tauri::AppHandle;

pub mod commands;
pub mod config;
pub mod crypto;
pub mod poll;
pub mod protocol;
pub mod relay;
pub mod state;

pub use poll::{start_poller, stop_poller, wake};

use protocol::Payload;
use state::ShareState;

/// How often the reader asks its mailbox whether anything is waiting.
///
/// One KV read per tick against a 100,000/day budget: two devices around the
/// clock is about 2,880. A shorter interval buys latency the eventual-consistency
/// window (Cloudflare documents "60 seconds or more") would swallow anyway.
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Seven days. A message nobody collects disappears on its own.
pub const MESSAGE_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;

/// Thirty days, longer than any ciphertext the cursors can point at — so a live
/// cursor never points into an expired range, and a null cursor reliably means
/// "this mailbox is empty and the counter may restart at 0".
pub const CURSOR_TTL_SECONDS: u64 = 30 * 24 * 60 * 60;

/// The most messages one tick will drain before yielding.
///
/// A cap rather than "until the mailbox is empty": teardown joins this thread,
/// and a backlog of two hundred messages must not hold the process open for the
/// whole drain. The remainder is taken on the next tick.
pub const DRAIN_LIMIT: u32 = 20;

/// How long a missing message is retried before the reader gives up on it.
///
/// Fifteen times Cloudflare's documented "60 seconds or more" for KV
/// convergence. A 404 is "not here *yet*" far more often than it is "gone", and
/// skipping on the first miss would turn an ordinary transient into permanent
/// silent loss. When the grace does run out, the skip is reported rather than
/// swallowed.
pub const HOLE_GRACE: Duration = Duration::from_secs(15 * 60);

/// How many ticks a message that fetches but will not open is retried.
pub const POISON_LIMIT: u32 = 3;

/// The floor between two sends.
///
/// KV allows roughly one write per second to a single key, and every send writes
/// the head key. Two notes sent back to back would otherwise race that limit.
pub const MIN_SEND_INTERVAL: Duration = Duration::from_millis(1100);

/// For the requests that carry a counter and nothing else.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// For the two requests that can carry 20 MiB.
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);

/// The next sequence after `seq`, refusing rather than saturating.
///
/// `checked_add`, not `saturating_add`: saturating at `u64::MAX` would leave a
/// cursor pointing at the slot it had just used. For the reader that means
/// fetching, delivering and acknowledging the same message on every tick for
/// ever; for the sender it means handing the next send a slot whose message the
/// reader has not consumed. The number is unreachable in practice — it is a
/// hundred thousand years of sending one note a nanosecond — which is exactly
/// why it must fail loudly rather than quietly do the wrong thing.
pub fn advance(seq: u64) -> Result<u64> {
	seq.checked_add(1).ok_or_else(|| {
		StoreError::Invalid(
			"this pairing has run out of message numbers; generate a new pairing secret".into(),
		)
	})
}

// --- the process-wide state --------------------------------------------------

/// The one [`ShareState`] this process has.
///
/// A `static` rather than Tauri managed state, because the poll thread reaches
/// it too and it has no `AppHandle` to resolve through at the moment it wakes.
/// Initialised by [`init`] from `setup()`; a command that somehow arrives first
/// falls back to `default_config_dir()`, so the IPC surface never depends on the
/// ordering inside `setup()`.
static SHARE: OnceLock<Arc<ShareState>> = OnceLock::new();

/// Points the share state at a directory. Called once, from `setup()`.
pub fn init(dir: &Path) {
	let _ = SHARE.set(Arc::new(ShareState::load(dir)));
}

/// The share state, or `None` when there is no roaming profile to store it in.
pub fn state() -> Option<&'static Arc<ShareState>> {
	if SHARE.get().is_none() {
		let dir = copper_core::store::settings::default_config_dir()?;
		init(&dir);
	}
	SHARE.get()
}

/// The share state, or the error a command reports.
pub fn require_state() -> Result<&'static Arc<ShareState>> {
	state().ok_or_else(|| {
		StoreError::Unavailable(
			"Copper cannot find a place to store the share settings on this machine".into(),
		)
	})
}

// --- delivery ----------------------------------------------------------------

/// Turns one decrypted message into notes on disk.
///
/// The order is the whole of it: **the blobs are written before the document
/// is**, matching `attachments::ingest`'s own rule, so a failure leaves orphan
/// blobs for the sweep rather than a document referencing files that are not
/// there. That same path is then handed to
/// [`copper_core::store::append_received`], which re-checks it **under the store
/// lock** — a space switch between here and that lock would otherwise file the
/// notes in a different document from their attachments.
///
/// `space_path` is read by the caller **before the ciphertext was opened**,
/// which is what makes every switch detectable rather than only the ones landing
/// after the decrypt. A switch turns the check below into a refusal, so the
/// message stays unacknowledged and is delivered on a later tick.
///
/// Returns `Ok` only once the notes are committed. The poller acknowledges the
/// message on `Ok` and on nothing else.
pub fn deliver_once(app: &AppHandle, payload: &Payload, space_path: &Path) -> Result<()> {
	let mut notes = Vec::with_capacity(payload.notes.len());
	for note in &payload.notes {
		let mut attachments = Vec::with_capacity(note.attachments.len());
		for (name, bytes) in &note.attachments {
			attachments.push(crate::attachments::ingest(space_path, bytes, name)?);
		}
		notes.push(copper_core::store::ReceivedNote {
			body: note.body.clone(),
			attachments,
		});
	}

	let shared = tauri::Manager::state::<copper_core::store::SharedStore>(app);
	copper_core::store::append_received(&shared, space_path, &notes)
}
