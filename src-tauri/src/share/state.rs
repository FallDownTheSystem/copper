//! The single in-process owner of `share.json`.
//!
//! `atomic::write_atomic` prevents a *torn* file and nothing else — its own doc
//! comment says it is for single-writer destinations. `share.json` has five
//! would-be writers: the poll thread advancing `nextRead`, a send reserving a
//! sequence, a config patch from the Settings view, secret generation, and the
//! poller recording `lastError`. Two of them each reading, editing and writing
//! the whole document would lose one of the edits with the file intact — which
//! is worse than a torn write, because nothing detects it.
//!
//! So every write goes through [`ShareState`], which holds the loaded config
//! under a mutex and saves it while still holding that mutex.
//!
//! **No lock is ever held across a network call.** Network work takes a
//! [`ShareState::snapshot`] plus a generation number, releases the lock, does its
//! request, and writes its result back through [`ShareState::apply_if_current`],
//! which refuses if the configuration changed while it was out. Without that, a
//! user changing the pairing secret mid-poll would have the old mailbox's cursor
//! written back over the reset one.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use copper_core::store::error::Result;

use super::config::{self, ShareConfig, StoredConfig};
use super::MIN_SEND_INTERVAL;

pub struct ShareState {
	config: Mutex<StoredConfig>,
	/// Bumped on every successful mutation. The whole of [`Self::apply_if_current`].
	generation: AtomicU64,
	/// When the previous send finished, so the next one can space itself out.
	/// Also serialises sends: one at a time, in this process.
	sending: Mutex<Option<Instant>>,
	dir: PathBuf,
}

impl ShareState {
	/// Loads `share.json` from `dir`.
	///
	/// The directory is a parameter for the same reason `config::load` takes one:
	/// the tests point it at a tempdir, and a `default_config_dir()` call buried
	/// inside would make that impossible.
	pub fn load(dir: &Path) -> Self {
		Self {
			config: Mutex::new(config::load(dir)),
			generation: AtomicU64::new(0),
			sending: Mutex::new(None),
			dir: dir.to_path_buf(),
		}
	}

	/// Tolerates a poisoned mutex, like `store::lock` and `editor`'s `recover`: a
	/// panic while holding this guard leaves a plain data struct behind, and
	/// refusing to work afterwards would turn one failure into a permanently
	/// broken feature.
	fn locked(&self) -> MutexGuard<'_, StoredConfig> {
		self.config.lock().unwrap_or_else(|err| err.into_inner())
	}

	/// The configuration and the generation it belongs to.
	///
	/// **Both read under one acquisition of the lock**, which is the whole point.
	/// Cloning the config, releasing the guard, and *then* loading the counter
	/// would let a mutation land between the two and hand the caller an old
	/// configuration stamped with the new generation — a pair
	/// [`Self::apply_if_current`] would happily accept, defeating the check
	/// entirely. Every mutation below bumps the counter while holding this same
	/// guard, so the pair cannot be torn.
	pub fn snapshot(&self) -> (StoredConfig, u64) {
		let config = self.locked();
		(config.clone(), self.generation.load(Ordering::SeqCst))
	}

	/// The frontend-facing shape, read under the lock.
	pub fn public(&self) -> ShareConfig {
		config::public(&self.locked())
	}

	/// Edits, saves and bumps the generation, all under one lock.
	///
	/// The save happens **while the lock is held**, which is what makes the file
	/// agree with memory: releasing it between the edit and the write would let a
	/// second writer interleave and produce a file matching neither caller's
	/// intent. A failed save leaves the in-memory value changed and reports the
	/// error — the alternative, rolling back, would mean a full disk silently
	/// undoing a setting the user just watched take effect.
	pub fn mutate<T>(&self, edit: impl FnOnce(&mut StoredConfig) -> T) -> Result<T> {
		let mut config = self.locked();
		let value = edit(&mut config);
		self.generation.fetch_add(1, Ordering::SeqCst);
		config::save(&self.dir, &config)?;
		Ok(value)
	}

	/// Applies `edit` only if nothing has been written since `generation`.
	///
	/// `None` means it did not apply, and that is an ordinary outcome rather than
	/// a failure: the user changed the configuration while a request was in
	/// flight, so the request's result describes a configuration that no longer
	/// exists. `Some` carries **the generation the write produced**, so a caller
	/// making several writes in one pass can chain them exactly. Re-reading the
	/// counter afterwards would not do: a third party bumping it in between would
	/// hand the caller a generation describing somebody else's write.
	pub fn apply_if_current(
		&self,
		generation: u64,
		edit: impl FnOnce(&mut StoredConfig),
	) -> Result<Option<u64>> {
		let mut config = self.locked();
		if self.generation.load(Ordering::SeqCst) != generation {
			return Ok(None);
		}
		edit(&mut config);
		// `fetch_add` returns the *previous* value.
		let produced = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
		config::save(&self.dir, &config)?;
		Ok(Some(produced))
	}

	/// Records a failure, or clears one, and says whether the value changed.
	///
	/// The boolean is what decides whether a `share-changed` event is emitted, so
	/// an unchanged error does not wake the Settings view once a minute for ever.
	pub fn report(&self, error: Option<String>) -> Result<bool> {
		let mut config = self.locked();
		if config.last_error == error {
			return Ok(false);
		}
		config.last_error = error;
		self.generation.fetch_add(1, Ordering::SeqCst);
		config::save(&self.dir, &config)?;
		Ok(true)
	}

	/// [`Self::report`], unless the configuration has moved on since `generation`.
	///
	/// A poll's verdict is about the configuration it ran against. A timeout from
	/// the relay the user has just replaced must not be written under the new one:
	/// `set_share_config` clears `lastError` precisely so a corrected setting shows
	/// a clean slate, and if the new setup is still incomplete there is no later
	/// poll coming to clear a stale sentence.
	pub fn report_if_current(&self, generation: u64, error: Option<String>) -> Result<bool> {
		if self.generation.load(Ordering::SeqCst) != generation {
			return Ok(false);
		}
		self.report(error)
	}

	/// Serialises sends and spaces them out.
	///
	/// KV allows roughly one write per second to any single key, and every send
	/// writes the head key. Holding the returned guard is what makes two sends
	/// queue rather than race; the sleep happens **before** the guard is handed
	/// back, so the caller cannot forget it.
	///
	/// **The clock runs from when the previous send finished, not from when it
	/// started**, and the difference is the whole point. The head write is the
	/// *last* thing a send does, so timing from the start would let a slow 20 MiB
	/// upload be followed immediately by a fast one — two head writes a fraction
	/// of a second apart, which is exactly what this exists to prevent. The
	/// timestamp is therefore taken in `Drop`.
	///
	/// The sleep is inside the lock deliberately. It is at most
	/// [`MIN_SEND_INTERVAL`], it is bounded, and the alternative — releasing and
	/// re-taking — would let a third send slip in between and defeat the spacing.
	pub fn send_guard(&self) -> SendGuard<'_> {
		let last = self.sending.lock().unwrap_or_else(|err| err.into_inner());
		if let Some(previous) = *last {
			let elapsed = previous.elapsed();
			if elapsed < MIN_SEND_INTERVAL {
				std::thread::sleep(MIN_SEND_INTERVAL - elapsed);
			}
		}
		SendGuard { held: last }
	}
}

/// Held for the duration of one send. Dropping it lets the next one start.
pub struct SendGuard<'a> {
	held: MutexGuard<'a, Option<Instant>>,
}

impl Drop for SendGuard<'_> {
	fn drop(&mut self) {
		*self.held = Some(Instant::now());
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::share::config::{Pending, ShareRole};

	fn state() -> (tempfile::TempDir, ShareState) {
		let dir = tempfile::tempdir().unwrap();
		let state = ShareState::load(dir.path());
		(dir, state)
	}

	#[test]
	fn a_mutation_reaches_the_file_and_bumps_the_generation() {
		let (dir, state) = state();
		let (_, before) = state.snapshot();

		state.mutate(|config| config.next_read = Some(5)).unwrap();

		let (config, after) = state.snapshot();
		assert_eq!(config.next_read, Some(5));
		assert!(after > before, "the generation did not move");
		assert_eq!(config::load(dir.path()).next_read, Some(5));
	}

	#[test]
	fn apply_if_current_writes_when_nothing_intervened() {
		let (dir, state) = state();
		let (_, generation) = state.snapshot();

		let applied = state
			.apply_if_current(generation, |config| config.next_read = Some(3))
			.unwrap();

		assert_eq!(applied, Some(generation + 1), "the write did not report its own generation");
		assert_eq!(config::load(dir.path()).next_read, Some(3));
	}

	/// The case the generation counter exists for: a poll reads the cursor,
	/// goes to the network, and the user changes the pairing secret while it is
	/// out. Writing the old mailbox's cursor back would undo the reset.
	#[test]
	fn apply_if_current_writes_nothing_after_an_intervening_mutation() {
		let (dir, state) = state();
		let (_, generation) = state.snapshot();

		state.mutate(|config| config.role = ShareRole::Second).unwrap();

		let applied = state
			.apply_if_current(generation, |config| config.next_read = Some(99))
			.unwrap();

		assert_eq!(applied, None, "a stale write was applied");
		assert_eq!(state.snapshot().0.next_read, None);
		assert_eq!(config::load(dir.path()).next_read, None);
		assert_eq!(config::load(dir.path()).role, ShareRole::Second);
	}

	#[test]
	fn report_only_writes_when_the_message_changes() {
		let (dir, state) = state();

		assert!(state.report(Some("broken".into())).unwrap());
		assert!(!state.report(Some("broken".into())).unwrap(), "an unchanged error rewrote");
		assert!(state.report(None).unwrap());

		assert_eq!(config::load(dir.path()).last_error, None);
	}

	/// A failure the user has to be able to see must survive a setting they
	/// change while the poll that produced it was running.
	#[test]
	fn report_ignores_the_generation() {
		let (_dir, state) = state();
		let (_, generation) = state.snapshot();

		state.mutate(|config| config.enabled = true).unwrap();
		assert!(state.report(Some("still broken".into())).unwrap());
		assert_eq!(state.snapshot().0.last_error.as_deref(), Some("still broken"));
		assert_ne!(state.snapshot().1, generation);
	}

	#[test]
	fn the_public_shape_is_read_through_the_same_lock() {
		let (_dir, state) = state();
		state
			.mutate(|config| {
				config.token = "t".into();
				config.secret = "s".into();
			})
			.unwrap();

		let public = state.public();
		assert!(public.token_set);
		assert!(public.secret_set);
	}

	#[test]
	fn the_state_loads_what_is_already_on_disk() {
		let dir = tempfile::tempdir().unwrap();
		let stored = StoredConfig {
			enabled: true,
			next_seq: Some(11),
			pending: Some(Pending {
				seq: 11,
				first_miss_at: "2026-08-09T16:00:00Z".into(),
				failures: 1,
			}),
			..Default::default()
		};
		config::save(dir.path(), &stored).unwrap();

		let state = ShareState::load(dir.path());
		assert_eq!(state.snapshot().0, stored);
	}

	/// The guard is what stops two sends racing KV's one-write-per-second limit
	/// on the head key. The first is not delayed; the second is.
	#[test]
	fn the_send_guard_spaces_consecutive_sends_out() {
		let (_dir, state) = state();

		let started = Instant::now();
		drop(state.send_guard());
		assert!(started.elapsed() < MIN_SEND_INTERVAL, "the first send was delayed");

		let second = Instant::now();
		drop(state.send_guard());
		assert!(
			second.elapsed() >= MIN_SEND_INTERVAL - std::time::Duration::from_millis(50),
			"the second send was not spaced out: {:?}",
			second.elapsed()
		);
	}

	/// The clock runs from the *end* of the previous send. A long upload writes
	/// its head key when it finishes, so timing from the start would let a fast
	/// send follow it immediately and put two head writes a fraction of a second
	/// apart — the exact collision the interval exists to prevent.
	#[test]
	fn the_interval_is_measured_from_when_the_previous_send_finished() {
		let (_dir, state) = state();

		// A send that takes longer than the whole interval, all of it *inside* the
		// guard, as a 20 MiB upload does.
		let first = state.send_guard();
		std::thread::sleep(MIN_SEND_INTERVAL + std::time::Duration::from_millis(50));
		drop(first);

		let second = Instant::now();
		drop(state.send_guard());
		assert!(
			second.elapsed() >= MIN_SEND_INTERVAL - std::time::Duration::from_millis(50),
			"a slow send let the next one start immediately: {:?}",
			second.elapsed()
		);
	}
}
