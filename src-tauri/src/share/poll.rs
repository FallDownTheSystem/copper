//! The one thread that collects delivered notes, and the rules it follows.
//!
//! The shape is `editor.rs`'s idle sweeper: a `static` holding a stop flag, a
//! `Condvar` and a join handle; an idempotent start; a stop that sets the flag,
//! notifies and **joins**. Two differences, both deliberate.
//!
//! **While the feature is disabled — or not yet fully configured — the wait has
//! no timeout at all.** A switched-off share costs not one wake-up per minute;
//! it costs none, until something calls [`wake`]. `set_share_config` and
//! `generate_share_secret` both do, and because that wait is indefinite the wake
//! is a **latch** rather than a bare notify: see [`PollFlags`].
//!
//! **The drain checks the stop flag between messages.** Teardown joins this
//! thread, and a backlog of twenty messages must not hold the process open for
//! the whole drain; the join is bounded by one in-flight request instead.
//!
//! # The order that makes delivery safe
//!
//! For each message: fetch, open, parse, **commit to disk**, persist the cursor,
//! then delete from the relay. Progress is recorded before the acknowledgement,
//! so a failed delete leaves a message that expires on its own rather than being
//! delivered twice. A crash between the commit and the persist re-delivers once
//! — the accepted at-least-once window, and the direction every ambiguous case
//! in this feature resolves towards, because `Ctrl+Z` fixes a duplicate and
//! nothing fixes a note that never arrived.
//!
//! # The hole rule
//!
//! A 404 means "not here **yet**" far more often than it means "gone". KV
//! propagates keys independently and caches negative lookups, so a reader can
//! legitimately see a fresh head pointer and then fail to fetch the message it
//! points at. Skipping on the first miss would turn that ordinary transient into
//! permanent, silent loss. The reader records the miss and retries the same
//! sequence every tick; only after [`HOLE_GRACE`] does it advance past it, and
//! the skip is written to `lastError` naming the sequence, so a genuinely lost
//! message is *reported* rather than swallowed.
//!
//! # The poison rule
//!
//! A message that fetches but will not open or parse is retried
//! [`POISON_LIMIT`] times across ticks, then skipped — also reported. One
//! `pending` slot serves both rules because the drain is strictly sequential:
//! only the head of the line can ever be stuck.

use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use copper_core::store::error::{Result, StoreError};
use copper_core::store::format::{now_rfc3339, seconds_since};
use tauri::{AppHandle, Emitter, Manager};

use super::config::{self, Pending, StoredConfig};
use super::protocol::Payload;
use super::relay::{HttpRelay, Relay};
use super::state::ShareState;
use super::{advance, crypto, DRAIN_LIMIT, HOLE_GRACE, POISON_LIMIT, POLL_INTERVAL};

/// Emitted when `lastError` changes, so an already-open Settings view learns
/// that a poll failed without having to be re-opened.
pub const SHARE_CHANGED: &str = "share-changed";

/// Where one message's notes go once they have been decrypted and parsed.
///
/// A trait rather than a direct call so the drain loop below is testable: the
/// production implementation needs a store, an `AppHandle` and a real space on
/// disk, and none of that is what the hole and poison rules are about.
pub trait Deliver {
	/// Which document a message fetched now would be written into.
	///
	/// Read **before** the ciphertext is opened, so that the space a delivery is
	/// aimed at is the one that was open when the message arrived rather than
	/// whichever happens to be open by the time it has been decrypted, ingested
	/// and written. `append_received` re-checks this path under the store lock,
	/// so a switch anywhere in between is turned into a refusal — the message
	/// stays unacknowledged and the notes unwritten — instead of landing
	/// somewhere the user was not looking.
	fn destination(&self) -> Result<std::path::PathBuf>;

	/// Returns `Ok` **only** when the notes are committed to disk. The caller
	/// persists its cursor and acknowledges on `Ok` and on nothing else.
	fn deliver(&self, payload: &Payload, space: &std::path::Path) -> Result<()>;
}

// --- the thread --------------------------------------------------------------

/// The two flags the tick thread waits on.
///
/// `woken` is a **latch**, not a nicety. `Condvar::notify_all` wakes only a
/// thread that is already waiting, so a bare notify racing the moment before the
/// wait is simply lost — and with the feature disabled that wait has no timeout,
/// so enabling share would leave the poller asleep for the life of the process.
/// Setting a flag under the same mutex the wait uses is what makes the signal
/// impossible to miss.
struct PollFlags {
	stop: bool,
	woken: bool,
}

struct Poller {
	flags: Mutex<PollFlags>,
	wake: Condvar,
	thread: Mutex<Option<JoinHandle<()>>>,
}

static POLLER: Poller = Poller {
	flags: Mutex::new(PollFlags {
		stop: false,
		woken: false,
	}),
	wake: Condvar::new(),
	thread: Mutex::new(None),
};

/// Poison is recovered from rather than propagated, as in `editor.rs`: the
/// guarded values are a `bool` and a `JoinHandle`, so a panicking holder leaves
/// no invariant broken.
fn recover<T>(result: std::sync::LockResult<T>) -> T {
	result.unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the poller has been asked to stop. Read between drained messages.
pub fn stopped() -> bool {
	recover(POLLER.flags.lock()).stop
}

/// Waits for the next tick, or for a wake, and reports whether to stop.
///
/// `ticking` decides which wait this is. A feature that is switched off **or not
/// yet fully configured** waits with no timeout at all, so it consumes nothing
/// until [`wake`] is called; one that is ready to poll waits [`POLL_INTERVAL`].
///
/// A pending wake is consumed here rather than slept through: `wake()` sets the
/// latch under this same mutex, so a notify that arrived while the caller was
/// between reading its configuration and getting here is still seen. A spurious
/// wake costs one early poll, which is one KV read.
fn wait_for_tick(ticking: bool) -> bool {
	let mut flags = recover(POLLER.flags.lock());
	if flags.stop {
		return true;
	}
	if flags.woken {
		flags.woken = false;
		return false;
	}

	let mut flags = if ticking {
		recover(POLLER.wake.wait_timeout(flags, POLL_INTERVAL)).0
	} else {
		recover(POLLER.wake.wait(flags))
	};
	flags.woken = false;
	flags.stop
}

/// Rouses the thread, so a change of settings takes effect within a second
/// rather than within a minute.
pub fn wake() {
	recover(POLLER.flags.lock()).woken = true;
	POLLER.wake.notify_all();
}

/// Starts the one poll thread, and returns at once. Idempotent.
pub fn start_poller(app: &AppHandle) {
	let mut slot: MutexGuard<'_, Option<JoinHandle<()>>> = recover(POLLER.thread.lock());
	if slot.is_some() {
		return;
	}

	let app = app.clone();
	let spawned = std::thread::Builder::new()
		.name("copper-share".into())
		.spawn(move || {
			loop {
				// Read before the wait, so the *first* wait after a config change is
				// the right kind: enabling the feature wakes the thread, and it must
				// then start ticking rather than going back to sleep indefinitely.
				//
				// **Enabled *and* resolvable**, not merely enabled. The requirement is
				// that the feature costs nothing — not one timer wake-up — until the
				// user has supplied a relay URL, a token and a pairing secret, and a
				// switch flipped on above three empty fields is not that. Filling the
				// last field calls `wake()`, which is what starts the ticking.
				let ticking = super::state().is_some_and(|state| {
					let (config, _) = state.snapshot();
					config.enabled && config::resolve(&config).is_ok()
				});
				if wait_for_tick(ticking) {
					break;
				}
				run_one_tick(&app);
			}
		});

	match spawned {
		Ok(handle) => *slot = Some(handle),
		// Degrades rather than propagating, like everything else `setup()` starts
		// below the store bootstrap: a thread that will not spawn must cost the user
		// this one feature, not the app.
		Err(err) => crate::diagnostics::log_error(&format!(
			"[copper] share: the poll thread could not be started, so nothing will be received this \
			 session: {err}"
		)),
	}
}

/// Stops the poller and **waits for it**, so teardown does not run beside a
/// delivery in flight. Idempotent.
pub fn stop_poller() {
	recover(POLLER.flags.lock()).stop = true;
	POLLER.wake.notify_all();
	let handle = recover(POLLER.thread.lock()).take();
	if let Some(handle) = handle {
		// The poller takes neither lock this thread holds — `stop` was released
		// above, and it never touches `thread` — so the join cannot deadlock.
		let _ = handle.join();
	}
}

/// One tick against the real relay, with its outcome recorded.
fn run_one_tick(app: &AppHandle) {
	let Some(state) = super::state() else {
		return;
	};
	// **One snapshot for the relay and for the tick.** Taking a second one below
	// would let a configuration change land in between, so the tick could talk to
	// the old host and write its answer back against the new configuration's
	// generation — a cursor for a mailbox the reply never described.
	let started = state.snapshot();
	let (config, _) = &started;
	if !config.enabled {
		return;
	}
	let Ok(ready) = config::resolve(config) else {
		// Not an error to report: an unconfigured share is the state the user is in
		// while they are still filling the fields in.
		return;
	};

	let relay = HttpRelay::new(&ready.relay_url, &ready.token);
	let outcome = tick_from(&relay, state, &AppDelivery { app: app.clone() }, started.clone());

	let report = match outcome {
		Ok(()) => None,
		Err(err) => Some(err.message()),
	};
	// Fenced on the generation this tick started with. A timeout from the old
	// relay must not repopulate `lastError` after the user has just corrected the
	// URL — with the new setup incomplete there may be no later poll to clear it,
	// so the stale sentence would stand under the Share section indefinitely.
	match state.report_if_current(started.1, report) {
		// Only on a change, so an unreachable relay does not wake the Settings view
		// once a minute forever.
		Ok(true) => {
			if let Err(err) = app.emit(SHARE_CHANGED, ()) {
				crate::diagnostics::log_error(&format!("[copper] share: could not emit: {err}"));
			}
		}
		Ok(false) => {}
		Err(err) => {
			crate::diagnostics::log_error(&format!(
				"[copper] share: could not record the poll outcome: {err}"
			));
		}
	}
}

/// The production [`Deliver`]: ingest the blobs, then write the notes.
struct AppDelivery {
	app: AppHandle,
}

impl Deliver for AppDelivery {
	fn destination(&self) -> Result<std::path::PathBuf> {
		active_space(&self.app)
	}

	fn deliver(&self, payload: &Payload, space: &std::path::Path) -> Result<()> {
		super::deliver_once(&self.app, payload, space)
	}
}

// --- the tick ----------------------------------------------------------------

/// Collects whatever is waiting, at most [`DRAIN_LIMIT`] messages.
///
/// Generic over the relay and the delivery so the whole of it is testable
/// against an in-memory fake. Returns the first failure worth reporting; a
/// disabled or unconfigured share returns `Ok` and does nothing.
pub fn tick<R: Relay, D: Deliver>(relay: &R, state: &ShareState, deliver: &D) -> Result<()> {
	let started = state.snapshot();
	tick_from(relay, state, deliver, started)
}

/// [`tick`] against a snapshot the caller already holds.
///
/// The production caller builds its `HttpRelay` from that same snapshot, so the
/// host it talks to and the mailboxes it writes cursors for cannot come from two
/// different configurations.
pub fn tick_from<R: Relay, D: Deliver>(
	relay: &R,
	state: &ShareState,
	deliver: &D,
	started: (StoredConfig, u64),
) -> Result<()> {
	let (config, generation) = started;
	if !config.enabled {
		return Ok(());
	}
	let Ok(ready) = config::resolve(&config) else {
		return Ok(());
	};

	let mut generation = generation;
	let mut next_read = config.next_read;
	let mut pending = config.pending.clone();

	// --- re-sync, only when the reader's counter is unknown -------------------
	//
	// Losing `share.json` must not lose deliverable messages, so the reader
	// re-syncs from the relay's acknowledged cursor. Because that cursor is
	// written only *after* a message is committed locally, re-syncing from it can
	// re-deliver an already-committed message whose ack write failed — but it
	// cannot skip one that was never consumed, which is the direction that
	// matters.
	//
	// The **sender's** counter is deliberately not re-synced here. It belongs to
	// `share_send_notes`, which reads the peer's head itself when its counter is
	// unknown: doing it there means a send made before the first poll re-syncs
	// too, and it keeps this poll's cold cost at two reads rather than three.
	if next_read.is_none() {
		let from = match relay.acked(&ready.own)? {
			Some(acked) => advance(acked)?,
			None => 0,
		};
		next_read = Some(from);
		if !persist(state, &mut generation, |config| config.next_read = Some(from))? {
			return Ok(());
		}
	}

	// --- the one read an idle poll costs -------------------------------------
	let mut next_read = next_read.unwrap_or(0);
	let Some(head) = relay.head(&ready.own)? else {
		return Ok(());
	};
	if head < next_read {
		return Ok(());
	}

	// --- the drain ------------------------------------------------------------
	for _ in 0..DRAIN_LIMIT {
		// Between messages, not around the whole loop: teardown waits for this
		// thread, and the bound on that wait should be one request rather than one
		// drain.
		if stopped() || next_read > head {
			break;
		}
		let seq = next_read;
		// Before the fetch, so the whole of decrypt-ingest-write is aimed at one
		// document. See `Deliver::destination`.
		let destination = deliver.destination()?;

		let Some(wire) = relay.fetch(&ready.own, seq)? else {
			// --- the hole rule ---
			//
			// The stored timestamp is reused only when the pending entry is itself a
			// run of misses — `failures == 0`. A message that fetched and failed to
			// open on earlier ticks left a `pending` for the *poison* rule, with a
			// timestamp that may be older than the grace window; inheriting it would
			// skip this 404 on its very first sighting, which is the one thing this
			// rule exists to prevent.
			let miss = match pending
				.as_ref()
				.filter(|held| held.seq == seq && held.failures == 0)
			{
				Some(held) => held.clone(),
				None => Pending {
					seq,
					first_miss_at: now_rfc3339(),
					failures: 0,
				},
			};
			// `None` from an unparseable timestamp reads as "not yet", so a
			// hand-edited file can only ever lengthen the wait.
			let waited = seconds_since(&miss.first_miss_at).unwrap_or(0);
			if waited < HOLE_GRACE.as_secs() {
				persist(state, &mut generation, |config| config.pending = Some(miss))?;
				return Ok(());
			}

			// The grace has run out. Advance past it, and say so — a message this
			// design gives up on is reported, never discarded quietly.
			let skipped = seq;
			next_read = advance(seq)?;
			persist(state, &mut generation, |config| {
				config.next_read = Some(next_read);
				config.pending = None;
			})?;
			return Err(StoreError::NotFound(format!(
				"a note sent to this device (message {skipped}) never arrived at the relay and has \
				 been skipped; ask the other device to send it again"
			)));
		};

		// The message is here, so whatever hole was recorded for it is over. Left
		// standing, its timestamp would be inherited by a *later* 404 on the same
		// sequence — a delivery that fails after this point leaves the cursor where
		// it is, and the next tick could then find the message gone and skip it on
		// its first miss against a stopwatch that started long ago.
		if pending.as_ref().is_some_and(|held| held.seq == seq && held.failures == 0) {
			pending = None;
			persist(state, &mut generation, |config| config.pending = None)?;
		}

		let opened = crypto::open(&ready.keys, &ready.own, seq, &wire)
			.and_then(|plaintext| super::protocol::parse_payload(&plaintext));
		let payload = match opened {
			Ok(payload) => payload,
			Err(err) => {
				// --- the poison rule ---
				let failures = pending
					.as_ref()
					.filter(|held| held.seq == seq)
					.map_or(0, |held| held.failures)
					.saturating_add(1);

				if failures < POISON_LIMIT {
					let miss = Pending {
						seq,
						first_miss_at: now_rfc3339(),
						failures,
					};
					persist(state, &mut generation, |config| config.pending = Some(miss))?;
					return Err(err);
				}

				let skipped = seq;
				next_read = advance(seq)?;
				persist(state, &mut generation, |config| {
					config.next_read = Some(next_read);
					config.pending = None;
				})?;
				// Deleted as well as skipped: it is provably unreadable by this device,
				// so leaving it would only spend the relay's storage until its TTL. A
				// failed delete is ignored — the TTL is the backstop.
				let _ = relay.ack(&ready.own, skipped);
				return Err(StoreError::Invalid(format!(
					"a note sent to this device (message {skipped}) could not be read after \
					 {POISON_LIMIT} attempts and has been skipped: {}",
					err.message()
				)));
			}
		};

		// The commit point. Anything that fails here leaves the message in place,
		// the cursor where it was, and the tick over — so the next tick tries the
		// same message again.
		deliver.deliver(&payload, &destination)?;

		next_read = advance(seq)?;
		pending = None;
		if !persist(state, &mut generation, |config| {
			config.next_read = Some(next_read);
			config.pending = None;
		})? {
			// The configuration changed mid-drain. The notes are already on disk, so
			// stopping here is at worst one re-delivery.
			return Ok(());
		}

		// After the cursor, never before it. A failed delete leaves a message that
		// expires on its own; a delete that succeeded before the cursor was written
		// would leave a message the reader still believes it has to fetch, and the
		// hole rule would then stall for fifteen minutes over a message that was
		// correctly consumed.
		relay.ack(&ready.own, seq)?;
	}

	Ok(())
}

/// Writes through the generation check and keeps the generation current.
///
/// `false` means the configuration changed while this tick was on the network,
/// which is an ordinary outcome: the tick's result describes a configuration
/// that no longer exists, so it is dropped rather than written back.
///
/// The new generation comes back **from the write itself** rather than from a
/// fresh read. A read here would pick up whatever the counter happened to be a
/// moment later, so a config change landing in the gap would be mistaken for
/// this tick's own write and the next `persist` would sail through a check it
/// should have failed.
fn persist(
	state: &ShareState,
	generation: &mut u64,
	edit: impl FnOnce(&mut StoredConfig),
) -> Result<bool> {
	match state.apply_if_current(*generation, edit)? {
		Some(produced) => {
			*generation = produced;
			Ok(true)
		}
		None => Ok(false),
	}
}

/// Reads the space this delivery must land in. Public for `share::deliver_once`.
///
/// The guard is taken and dropped here. It is not the check that matters —
/// `append_received` re-checks the path under its own lock — this only says
/// which space the blobs should be ingested beside.
pub(super) fn active_space(app: &AppHandle) -> Result<std::path::PathBuf> {
	let shared = app.state::<copper_core::store::SharedStore>();
	// Bound rather than left as a temporary in the tail expression: a temporary
	// there outlives `shared`, which does not compile.
	let guard = copper_core::store::lock(shared.inner());
	guard.require_active_path()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::share::config::{ShareConfigPatch, ShareRole};
	use crate::share::crypto::Keys;
	use crate::share::protocol::{build_payload, PayloadNote};
	use crate::share::relay::SendAck;
	use std::collections::HashMap;
	use std::sync::atomic::{AtomicU32, Ordering};

	// --- the fake relay -------------------------------------------------------

	#[derive(Default)]
	struct Fake {
		messages: Mutex<HashMap<(String, u64), Vec<u8>>>,
		heads: Mutex<HashMap<String, u64>>,
		acks: Mutex<HashMap<String, u64>>,
		head_calls: AtomicU32,
		acked_calls: AtomicU32,
		fetch_calls: AtomicU32,
		ack_calls: AtomicU32,
		/// When set, every `fetch` fails with it instead of answering.
		fetch_error: Mutex<Option<String>>,
	}

	impl Fake {
		fn put(&self, mailbox: &str, seq: u64, body: Vec<u8>) {
			self.messages.lock().unwrap().insert((mailbox.into(), seq), body);
			let mut heads = self.heads.lock().unwrap();
			let head = heads.entry(mailbox.into()).or_insert(seq);
			*head = (*head).max(seq);
		}

		/// A head pointer with no message under it — the transient KV state the
		/// hole rule exists for.
		fn announce(&self, mailbox: &str, seq: u64) {
			self.heads.lock().unwrap().insert(mailbox.into(), seq);
		}
	}

	impl Relay for Fake {
		fn head(&self, mailbox: &str) -> Result<Option<u64>> {
			self.head_calls.fetch_add(1, Ordering::SeqCst);
			Ok(self.heads.lock().unwrap().get(mailbox).copied())
		}

		fn acked(&self, mailbox: &str) -> Result<Option<u64>> {
			self.acked_calls.fetch_add(1, Ordering::SeqCst);
			Ok(self.acks.lock().unwrap().get(mailbox).copied())
		}

		fn fetch(&self, mailbox: &str, seq: u64) -> Result<Option<Vec<u8>>> {
			self.fetch_calls.fetch_add(1, Ordering::SeqCst);
			if let Some(message) = self.fetch_error.lock().unwrap().clone() {
				return Err(StoreError::Unavailable(message));
			}
			Ok(self.messages.lock().unwrap().get(&(mailbox.into(), seq)).cloned())
		}

		fn send(&self, mailbox: &str, seq: u64, body: &[u8]) -> Result<SendAck> {
			self.put(mailbox, seq, body.to_vec());
			Ok(SendAck::Delivered)
		}

		fn ack(&self, mailbox: &str, seq: u64) -> Result<()> {
			self.ack_calls.fetch_add(1, Ordering::SeqCst);
			self.messages.lock().unwrap().remove(&(mailbox.into(), seq));
			self.acks.lock().unwrap().insert(mailbox.into(), seq);
			Ok(())
		}
	}

	// --- the fake delivery ----------------------------------------------------

	#[derive(Default)]
	struct Landed {
		bodies: Mutex<Vec<String>>,
		fail_with: Mutex<Option<String>>,
	}

	impl Deliver for Landed {
		fn destination(&self) -> Result<std::path::PathBuf> {
			Ok(std::path::PathBuf::from("C:\\notes.copper"))
		}

		fn deliver(&self, payload: &Payload, _space: &std::path::Path) -> Result<()> {
			if let Some(message) = self.fail_with.lock().unwrap().clone() {
				return Err(StoreError::Unavailable(message));
			}
			let mut bodies = self.bodies.lock().unwrap();
			for note in &payload.notes {
				bodies.push(note.body.clone());
			}
			Ok(())
		}
	}

	// --- the harness ----------------------------------------------------------

	struct Harness {
		_dir: tempfile::TempDir,
		state: ShareState,
		relay: Fake,
		landed: Landed,
		keys: Keys,
		own: String,
	}

	fn harness() -> Harness {
		let dir = tempfile::tempdir().unwrap();
		let state = ShareState::load(dir.path());
		let secret = crypto::generate_secret().unwrap();
		state
			.mutate(|config| {
				config::patch(
					config,
					ShareConfigPatch {
						enabled: Some(true),
						relay_url: Some("https://relay.workers.dev".into()),
						role: Some(ShareRole::First),
						token: Some(Some("token".into())),
						secret: Some(Some(secret.clone())),
					},
				);
			})
			.unwrap();

		let keys = crypto::derive(&crypto::decode_secret(&secret).unwrap());
		let own = keys.mailbox_1.clone();
		Harness {
			_dir: dir,
			state,
			relay: Fake::default(),
			landed: Landed::default(),
			keys,
			own,
		}
	}

	impl Harness {
		fn tick(&self) -> Result<()> {
			tick(&self.relay, &self.state, &self.landed)
		}

		/// Seals a one-note message and puts it in the reader's own mailbox.
		fn waiting(&self, seq: u64, body: &str) {
			let plaintext = build_payload(&[PayloadNote {
				body: body.into(),
				attachments: Vec::new(),
			}])
			.unwrap();
			let sealed = crypto::seal(&self.keys, &self.own, seq, &plaintext).unwrap();
			self.relay.put(&self.own, seq, sealed);
		}

		fn delivered(&self) -> Vec<String> {
			self.landed.bodies.lock().unwrap().clone()
		}

		fn config(&self) -> StoredConfig {
			self.state.snapshot().0
		}
	}

	// --- the tests ------------------------------------------------------------

	/// A21. Once both cursors are known, an empty mailbox costs one KV read and
	/// nothing else — which is the whole budget argument for this design.
	#[test]
	fn an_empty_mailbox_costs_one_head_call_and_no_fetch() {
		let harness = harness();
		// The first tick learns both cursors; the second is the steady state.
		harness.tick().unwrap();
		harness.relay.head_calls.store(0, Ordering::SeqCst);

		harness.tick().unwrap();

		assert_eq!(harness.relay.head_calls.load(Ordering::SeqCst), 1);
		assert_eq!(harness.relay.fetch_calls.load(Ordering::SeqCst), 0);
		assert_eq!(harness.relay.ack_calls.load(Ordering::SeqCst), 0);
	}

	#[test]
	fn one_waiting_message_is_delivered_then_acknowledged() {
		let harness = harness();
		harness.waiting(0, "from the laptop");

		harness.tick().unwrap();

		assert_eq!(harness.delivered(), ["from the laptop"]);
		assert_eq!(harness.config().next_read, Some(1));
		assert_eq!(harness.config().pending, None);
		assert_eq!(harness.relay.ack_calls.load(Ordering::SeqCst), 1);
		assert_eq!(harness.relay.acks.lock().unwrap().get(&harness.own), Some(&0));
	}

	#[test]
	fn several_waiting_messages_are_drained_in_order() {
		let harness = harness();
		for (seq, body) in [(0, "one"), (1, "two"), (2, "three")] {
			harness.waiting(seq, body);
		}

		harness.tick().unwrap();

		assert_eq!(harness.delivered(), ["one", "two", "three"]);
		assert_eq!(harness.config().next_read, Some(3));
	}

	/// The failure the hole rule exists for: a head pointer that has propagated
	/// and a message that has not. Skipping here would be silent, permanent loss.
	#[test]
	fn a_first_miss_does_not_advance_and_records_the_pending_message() {
		let harness = harness();
		harness.relay.announce(&harness.own, 0);

		harness.tick().unwrap();

		assert!(harness.delivered().is_empty());
		assert_eq!(harness.config().next_read, Some(0), "a first miss advanced the cursor");
		let pending = harness.config().pending.expect("the miss was not recorded");
		assert_eq!(pending.seq, 0);
		assert_eq!(pending.failures, 0);
	}

	/// And the message arriving late is delivered normally — the pending slot is
	/// not a quarantine.
	#[test]
	fn a_message_that_arrives_late_is_delivered_and_clears_the_pending_slot() {
		let harness = harness();
		harness.relay.announce(&harness.own, 0);
		harness.tick().unwrap();

		harness.waiting(0, "arrived at last");
		harness.tick().unwrap();

		assert_eq!(harness.delivered(), ["arrived at last"]);
		assert_eq!(harness.config().pending, None);
	}

	#[test]
	fn a_miss_older_than_the_grace_advances_and_is_reported() {
		let harness = harness();
		harness.relay.announce(&harness.own, 0);
		harness.tick().unwrap();

		// Backdate the first miss past the grace window, which is what a
		// fifteen-minute wait looks like from the next tick's point of view.
		harness
			.state
			.mutate(|config| {
				config.pending = Some(Pending {
					seq: 0,
					first_miss_at: "2020-01-01T00:00:00Z".into(),
					failures: 0,
				});
			})
			.unwrap();

		let err = harness.tick().unwrap_err();

		assert_eq!(harness.config().next_read, Some(1), "the skip did not advance");
		assert_eq!(harness.config().pending, None);
		assert!(err.message().contains("message 0"), "{}", err.message());
	}

	/// An unparseable timestamp must read as "not yet" — a hand-edited file can
	/// lengthen a wait, never shorten one.
	#[test]
	fn an_unreadable_first_miss_timestamp_does_not_shorten_the_grace() {
		let harness = harness();
		harness.relay.announce(&harness.own, 0);
		harness.tick().unwrap();
		harness
			.state
			.mutate(|config| {
				config.pending = Some(Pending {
					seq: 0,
					first_miss_at: "not a timestamp".into(),
					failures: 0,
				});
			})
			.unwrap();

		harness.tick().unwrap();

		assert_eq!(harness.config().next_read, Some(0), "a bad timestamp let the skip through");
	}

	#[test]
	fn an_unopenable_message_stops_the_tick_and_is_skipped_on_the_third_attempt() {
		let harness = harness();
		harness.relay.put(&harness.own, 0, vec![7u8; 64]);

		for attempt in 1..POISON_LIMIT {
			assert!(harness.tick().is_err(), "attempt {attempt} did not report");
			assert_eq!(harness.config().next_read, Some(0), "attempt {attempt} advanced early");
			assert_eq!(harness.config().pending.unwrap().failures, attempt);
		}

		let err = harness.tick().unwrap_err();
		assert_eq!(harness.config().next_read, Some(1));
		assert_eq!(harness.config().pending, None);
		assert!(err.message().contains("message 0"), "{}", err.message());
		assert!(harness.delivered().is_empty());
	}

	#[test]
	fn a_transport_error_mid_drain_leaves_the_cursor_at_the_unconsumed_message() {
		let harness = harness();
		harness.waiting(0, "one");
		harness.waiting(1, "two");
		*harness.relay.fetch_error.lock().unwrap() = Some("the relay could not be reached".into());

		assert!(harness.tick().is_err());

		assert!(harness.delivered().is_empty());
		assert_eq!(harness.config().next_read, Some(0));
		assert_eq!(harness.config().pending, None, "a transport error is not a miss");
	}

	/// A store failure — a space switch mid-delivery — leaves the message
	/// unacknowledged and the cursor where it was, so the next tick retries it.
	#[test]
	fn a_failed_delivery_acknowledges_nothing_and_does_not_advance() {
		let harness = harness();
		harness.waiting(0, "one");
		*harness.landed.fail_with.lock().unwrap() = Some("the open space changed".into());

		assert!(harness.tick().is_err());

		assert_eq!(harness.config().next_read, Some(0));
		assert_eq!(harness.relay.ack_calls.load(Ordering::SeqCst), 0);
	}

	#[test]
	fn a_backlog_stops_at_the_drain_limit_and_resumes_next_tick() {
		let harness = harness();
		for seq in 0..50 {
			harness.waiting(seq, &format!("note {seq}"));
		}

		harness.tick().unwrap();
		assert_eq!(harness.delivered().len(), DRAIN_LIMIT as usize);
		assert_eq!(harness.config().next_read, Some(u64::from(DRAIN_LIMIT)));

		harness.tick().unwrap();
		assert_eq!(harness.delivered().len(), DRAIN_LIMIT as usize * 2);
	}

	/// A30. Losing `share.json` must not lose deliverable messages.
	#[test]
	fn a_null_cursor_re_syncs_from_the_acknowledged_cursor() {
		let harness = harness();
		harness.relay.acks.lock().unwrap().insert(harness.own.clone(), 41);
		harness.state.mutate(config::reset_counters).unwrap();

		harness.tick().unwrap();

		assert_eq!(harness.config().next_read, Some(42));
	}

	#[test]
	fn a_null_cursor_with_no_acknowledged_cursor_re_syncs_to_zero() {
		let harness = harness();
		harness.state.mutate(config::reset_counters).unwrap();

		harness.tick().unwrap();

		assert_eq!(harness.config().next_read, Some(0));
	}

	/// The **sender's** counter is not the poller's business. `share_send_notes`
	/// recovers it from the peer's head when it is unknown, which is what makes a
	/// send before the first poll safe; doing it here as well would spend a third
	/// KV read on the one tick that can least afford it.
	#[test]
	fn the_poller_leaves_the_senders_counter_alone() {
		let harness = harness();
		let peer = harness.keys.mailbox_2.clone();
		harness.relay.heads.lock().unwrap().insert(peer, 8);
		harness.state.mutate(config::reset_counters).unwrap();

		harness.tick().unwrap();

		assert_eq!(harness.config().next_seq, None, "the poller re-synced the sender's counter");
		assert_eq!(harness.config().next_read, Some(0), "the reader's counter was not re-synced");
	}

	/// A21: a cold poll costs the head read plus **one** extra to learn the
	/// reader's cursor, and nothing more.
	#[test]
	fn a_cold_poll_costs_two_reads() {
		let harness = harness();
		harness.state.mutate(config::reset_counters).unwrap();

		harness.tick().unwrap();

		assert_eq!(
			harness.relay.head_calls.load(Ordering::SeqCst) + harness.relay.ack_calls.load(Ordering::SeqCst),
			1,
			"the cold poll made more than one head-shaped call besides the ack cursor"
		);
		assert_eq!(harness.relay.acked_calls.load(Ordering::SeqCst), 1);
	}

	/// A4 of the "off by default" requirement: a disabled share makes no request
	/// at all, not even the head read.
	#[test]
	fn a_disabled_share_makes_no_relay_call() {
		let harness = harness();
		harness.waiting(0, "waiting");
		harness.state.mutate(|config| config.enabled = false).unwrap();

		harness.tick().unwrap();

		assert_eq!(harness.relay.head_calls.load(Ordering::SeqCst), 0);
		assert_eq!(harness.relay.fetch_calls.load(Ordering::SeqCst), 0);
		assert!(harness.delivered().is_empty());
	}

	#[test]
	fn an_unconfigured_share_makes_no_relay_call() {
		let harness = harness();
		harness
			.state
			.mutate(|config| config.secret = String::new())
			.unwrap();

		harness.tick().unwrap();

		assert_eq!(harness.relay.head_calls.load(Ordering::SeqCst), 0);
	}

	/// The transition the two rules used to share a timestamp across. A message
	/// that failed to open on earlier ticks left a `pending` whose `firstMissAt`
	/// can be older than the whole grace window; if it then 404s — it expired, or
	/// the sender deleted it — inheriting that timestamp would skip it on its very
	/// first miss, which is the one thing the hole rule exists to prevent.
	#[test]
	fn a_404_after_poison_attempts_starts_its_own_grace_window() {
		let harness = harness();
		harness.relay.put(&harness.own, 0, vec![7u8; 64]);
		assert!(harness.tick().is_err(), "the unopenable message did not report");

		// Backdate the poison entry past the grace window and take the message away,
		// which is what an expiry looks like from the reader's side.
		harness
			.state
			.mutate(|config| {
				config.pending = Some(Pending {
					seq: 0,
					first_miss_at: "2020-01-01T00:00:00Z".into(),
					failures: 1,
				});
			})
			.unwrap();
		harness.relay.messages.lock().unwrap().remove(&(harness.own.clone(), 0));

		harness.tick().unwrap();

		assert_eq!(harness.config().next_read, Some(0), "the first miss skipped immediately");
		let pending = harness.config().pending.expect("the miss was not recorded");
		assert_eq!(pending.failures, 0, "the hole rule inherited the poison counter");
		assert_ne!(
			pending.first_miss_at, "2020-01-01T00:00:00Z",
			"the hole rule inherited the poison timestamp"
		);
	}

	/// A message sealed for the *other* mailbox must not open here, because the
	/// mailbox is bound into the AEAD's associated data.
	#[test]
	fn a_message_sealed_for_the_peers_mailbox_is_poison_rather_than_delivered() {
		let harness = harness();
		let plaintext = build_payload(&[PayloadNote {
			body: "not for this device".into(),
			attachments: Vec::new(),
		}])
		.unwrap();
		let sealed = crypto::seal(&harness.keys, &harness.keys.mailbox_2, 0, &plaintext).unwrap();
		harness.relay.put(&harness.own, 0, sealed);

		assert!(harness.tick().is_err());
		assert!(harness.delivered().is_empty());
	}
}
