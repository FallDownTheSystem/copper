//! Whether a recents entry can be opened right now, and the bounded executor
//! that finds out without blocking the menu.
//!
//! **Availability is probed, never persisted.** Nothing here reaches
//! `settings.json`, which is what makes "it comes back when the branch is
//! checked out again" work for free rather than needing a repair step.
//!
//! # Classifying from the error, not from a sequence of existence probes
//!
//! `Path::exists()` throws away exactly the information that separates "not
//! there" from "cannot reach" from "not allowed", and a pre-flight walk down the
//! path is unsound on Windows besides: a mapped drive's root can itself block,
//! and a UNC path has a server and a share rather than a local volume to test.
//! So there is one `metadata` call and the Win32 error code decides.
//!
//! Two refinements that look like details and are not:
//!
//! - **`metadata` succeeding does not prove the file is readable.** The read is
//!   its own step with its own classification, because collapsing them reports a
//!   locked or permission-denied file as a corrupt one — which sends the user to
//!   fix the wrong thing. `Invalid` is concluded only after a file has been read
//!   successfully and then failed to parse.
//! - **Anything not confidently a drive or network failure is `Unreadable`.**
//!   Perfect discrimination between a disconnected share, a DNS failure and an
//!   auth failure is not a goal; Windows does not reliably expose the difference,
//!   and the five user-facing categories are the contract, not the internal
//!   precision.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::lock;
use super::paths::{comparison_key, drive_letter};
use crate::store::format;

/// Exactly four probe threads, and the number is exact rather than "about four":
/// a dead path occupies one of them until Windows gives up, so the cap is what
/// stops a recents list with two dead shares in it from exhausting a pool.
const MAX_IN_FLIGHT: usize = 4;

/// A10's UI deadline, measured from **submission** and therefore inclusive of
/// time spent waiting for a thread. Not a syscall timeout — there is no such
/// thing for a blocked Win32 filesystem call — so it is what the *user* is
/// promised, not what the operating system is asked for.
const PROBE_DEADLINE: Duration = Duration::from_secs(2);

/// The event this task adds to the project's surface, and the only one.
///
/// It deliberately does not ride `settings-changed`: no setting changed, nothing
/// was written, and the frontend's `settings-changed` handler re-reads recents —
/// which would drive a refresh loop, every pass minting a new generation.
pub const AVAILABILITY_CHANGED: &str = "spaces-availability-changed";

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UnavailableReason {
	DriveUnavailable,
	Missing,
	NotAFile,
	Unreadable,
	Invalid,
}

/// Four states, and only the last carries a cause. `Unresponsive` is a transient
/// *state*, not a fifth cause: a probe that has not answered has concluded
/// nothing, and reporting "the drive isn't connected" about a slow-but-fine share
/// would be a lie the user then has to disprove.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Availability {
	Pending,
	Available,
	Unresponsive { message: String },
	Unavailable {
		reason: UnavailableReason,
		message: String,
	},
}

impl Availability {
	pub fn unresponsive() -> Self {
		Self::Unresponsive {
			message: "This location isn't responding.".to_string(),
		}
	}

	/// The wording per cause. One sentence each, distinguished rather than
	/// collapsed into a generic "could not open" — the same principle the capture
	/// failure display follows.
	pub fn unavailable(reason: UnavailableReason) -> Self {
		let message = match reason {
			UnavailableReason::DriveUnavailable => "The drive this project is on isn't connected.",
			UnavailableReason::Missing => "This file has been moved, renamed, or deleted.",
			UnavailableReason::NotAFile => "This path is a folder, not a project file.",
			// Deliberately does not name permissions: this is also the catch-all for
			// every unmapped I/O error, so blaming permissions would be a confident
			// guess that is usually wrong. `denied` below is the one case that knows.
			UnavailableReason::Unreadable => "This file can't be read.",
			UnavailableReason::Invalid => "This file isn't a valid Copper project.",
		};
		Self::Unavailable {
			reason,
			message: message.to_string(),
		}
	}

	/// Access denied specifically — the one `Unreadable` that may name its cause.
	fn denied() -> Self {
		Self::Unavailable {
			reason: UnavailableReason::Unreadable,
			message: "This file can't be read. You may not have permission to open it.".to_string(),
		}
	}

	/// The sentence to show, if there is one to show.
	pub fn message(&self) -> Option<&str> {
		match self {
			Self::Unresponsive { message } | Self::Unavailable { message, .. } => Some(message),
			Self::Pending | Self::Available => None,
		}
	}
}

// --- error-code classification ------------------------------------------------

// Win32 codes, named rather than left as bare numbers at the match arms.
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PATH_NOT_FOUND: i32 = 3;
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_INVALID_DRIVE: i32 = 15;
const ERROR_NOT_READY: i32 = 21;
const ERROR_REM_NOT_LIST: i32 = 51;
const ERROR_BAD_NETPATH: i32 = 53;
const ERROR_DEV_NOT_EXIST: i32 = 55;
const ERROR_NETNAME_DELETED: i32 = 64;
const ERROR_BAD_NET_NAME: i32 = 67;
const ERROR_DEVICE_UNREACHABLE: i32 = 321;
const ERROR_NO_MEDIA_IN_DRIVE: i32 = 1112;
const ERROR_DEVICE_NOT_CONNECTED: i32 = 1167;
const ERROR_CONNECTION_UNAVAIL: i32 = 1201;
const ERROR_NO_NETWORK: i32 = 1222;
const ERROR_NETWORK_UNREACHABLE: i32 = 1231;
const ERROR_HOST_UNREACHABLE: i32 = 1232;
const ERROR_PROTOCOL_UNREACHABLE: i32 = 1233;
const ERROR_HOST_DOWN: i32 = 1256;

/// Maps an I/O failure to one of the five causes.
///
/// The drive and network families are enumerated rather than sampled: a removable
/// drive with no media, a device that has gone away and most of the
/// network-unreachable range would all otherwise report as "can't be read", when
/// "the drive isn't connected" is both true and more useful.
pub fn classify(error: &std::io::Error) -> Availability {
	match error.raw_os_error() {
		Some(
			ERROR_NOT_READY
			| ERROR_INVALID_DRIVE
			| ERROR_REM_NOT_LIST
			| ERROR_BAD_NETPATH
			| ERROR_DEV_NOT_EXIST
			| ERROR_NETNAME_DELETED
			| ERROR_BAD_NET_NAME
			| ERROR_DEVICE_UNREACHABLE
			| ERROR_NO_MEDIA_IN_DRIVE
			| ERROR_DEVICE_NOT_CONNECTED
			| ERROR_CONNECTION_UNAVAIL
			| ERROR_NO_NETWORK
			| ERROR_NETWORK_UNREACHABLE
			| ERROR_HOST_UNREACHABLE
			| ERROR_PROTOCOL_UNREACHABLE
			| ERROR_HOST_DOWN,
		) => Availability::unavailable(UnavailableReason::DriveUnavailable),
		Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => {
			Availability::unavailable(UnavailableReason::Missing)
		}
		Some(ERROR_ACCESS_DENIED) => Availability::denied(),
		// The code is unmapped, or there is none at all because the failure came
		// from Rust rather than from Windows. Neither is confidently a drive.
		_ => {
			if error.kind() == std::io::ErrorKind::NotFound {
				Availability::unavailable(UnavailableReason::Missing)
			} else {
				Availability::unavailable(UnavailableReason::Unreadable)
			}
		}
	}
}

/// The filesystem as the probe sees it, so error codes can be simulated.
///
/// `is_dir` rather than a `Metadata`, because `std::fs::Metadata` cannot be
/// constructed by a test. Picking a drive letter such as `Q:` and assuming it is
/// absent is not a substitute — it may well exist on the machine running the
/// tests.
pub trait Filesystem: Send + Sync + 'static {
	fn is_dir(&self, path: &Path) -> std::io::Result<bool>;
	fn read(&self, path: &Path) -> std::io::Result<String>;
	/// Whether a local drive letter is currently present. The one place
	/// `DriveUnavailable` may be concluded with no I/O error behind it.
	fn drive_present(&self, letter: char) -> bool;
}

pub struct RealFs;

impl Filesystem for RealFs {
	fn is_dir(&self, path: &Path) -> std::io::Result<bool> {
		std::fs::metadata(path).map(|meta| meta.is_dir())
	}

	fn read(&self, path: &Path) -> std::io::Result<String> {
		std::fs::read_to_string(path)
	}

	/// A bitmask query, not a filesystem access: it answers instantly for a drive
	/// letter that has gone away, whereas `metadata` on a path beneath it reports
	/// `ERROR_PATH_NOT_FOUND` and would be classified as a deleted file.
	fn drive_present(&self, letter: char) -> bool {
		// SAFETY: no arguments, no pointers; the call reads a process-wide bitmask.
		let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
		let Some(index) = letter.to_ascii_uppercase().to_digit(36) else {
			return true;
		};
		// 'A' is digit 10 in base 36, and bit 0.
		let Some(bit) = index.checked_sub(10) else {
			return true;
		};
		mask & (1 << bit) != 0
	}
}

/// The probe itself: one classification, plus the document's own name when it
/// could be read.
///
/// The name matters beyond decoration — it is what the composer shows and what
/// the switcher lists. An unavailable entry falls back to the file stem, so a row
/// always shows something recognisable rather than a bare path.
pub fn probe(fs: &dyn Filesystem, path: &Path) -> (Availability, Option<String>) {
	if let Some(letter) = drive_letter(path) {
		if !fs.drive_present(letter) {
			return (
				Availability::unavailable(UnavailableReason::DriveUnavailable),
				None,
			);
		}
	}

	match fs.is_dir(path) {
		Err(err) => (classify(&err), None),
		Ok(true) => (
			Availability::unavailable(UnavailableReason::NotAFile),
			None,
		),
		Ok(false) => match fs.read(path) {
			Err(err) => (classify(&err), None),
			Ok(text) => match format::from_json(&text) {
				Ok(doc) => (Availability::Available, Some(doc.name)),
				Err(_) => (Availability::unavailable(UnavailableReason::Invalid), None),
			},
		},
	}
}

// --- the executor --------------------------------------------------------------

/// One entry's answer, stamped with the snapshot it was started for.
///
/// Serialised straight onto the wire as the `spaces-availability-changed`
/// payload, so the event's shape is this declaration rather than a second copy
/// of it written out at the emit site.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
	pub generation: u64,
	pub key: String,
	pub availability: Availability,
	pub name: Option<String>,
}

/// Where a finished probe goes. A trait so the executor can be tested with a
/// recorder instead of an `AppHandle`.
pub trait ResultSink: Send + Sync + 'static {
	fn deliver(&self, result: &ProbeResult);
}

#[derive(Clone, Debug)]
struct Job {
	key: String,
	path: PathBuf,
	generation: u64,
}

#[derive(Clone, Debug)]
struct Cached {
	availability: Availability,
	name: Option<String>,
	generation: u64,
}

#[derive(Default)]
struct State {
	generation: u64,
	/// Only the latest snapshot's entries, so the queue is bounded by the recents
	/// cap rather than by how many times the menu was opened.
	queue: VecDeque<Job>,
	in_flight: HashSet<String>,
	/// A refresh that arrived for a key whose attempt is still running. Without
	/// this, a timed-out-but-still-blocked probe suppresses every later refresh
	/// for that path — and A9's "comes back with no repair step" would stop being
	/// true precisely for the entries A9 is about.
	rerun: HashMap<String, Job>,
	members: HashSet<String>,
	cache: HashMap<String, Cached>,
	/// When the current snapshot's UI deadline falls due, and which snapshot it
	/// belongs to. There is only ever one: a newer submission supersedes the
	/// generation an older deadline would have expired, so replacing it is not a
	/// lost timer.
	due: Option<(Instant, u64)>,
	shutdown: bool,
	/// Test counters. Started probes and queued jobs are what the accumulation
	/// check asserts on — a process thread count cannot distinguish our leak from
	/// WebView2's churn.
	started: u64,
}

struct Shared {
	state: Mutex<State>,
	work: Condvar,
	/// Woken when a submission moves the pending deadline. Separate from `work` so
	/// the timer does not wake on every completed probe and the workers do not
	/// wake when a deadline is merely rescheduled.
	timer: Condvar,
	fs: Box<dyn Filesystem>,
	sink: Box<dyn ResultSink>,
	deadline: Duration,
}

pub struct Executor {
	shared: Arc<Shared>,
}

impl Executor {
	pub fn new(fs: Box<dyn Filesystem>, sink: Box<dyn ResultSink>) -> Self {
		Self::with_deadline(fs, sink, PROBE_DEADLINE)
	}

	/// The deadline is injectable so the timeout path can be tested in
	/// milliseconds rather than in two-second sleeps.
	pub fn with_deadline(
		fs: Box<dyn Filesystem>,
		sink: Box<dyn ResultSink>,
		deadline: Duration,
	) -> Self {
		let shared = Arc::new(Shared {
			state: Mutex::new(State::default()),
			work: Condvar::new(),
			timer: Condvar::new(),
			fs,
			sink,
			deadline,
		});
		for _ in 0..MAX_IN_FLIGHT {
			let worker = Arc::clone(&shared);
			// A fixed pool rather than one task per entry: the thread is what a
			// blocked filesystem call actually occupies, and it cannot be reclaimed
			// by a timeout — so the only way to bound the cost is to bound how many
			// exist.
			std::thread::spawn(move || run_worker(&worker));
		}
		// One timer for the process, rescheduled on each submission, rather than a
		// thread per submission: opening the menu is a cheap, repeatable action and
		// should not mint a thread that outlives the answer it was waiting for.
		let timer = Arc::clone(&shared);
		std::thread::spawn(move || run_timer(&timer));
		Self { shared }
	}

	/// Submits a snapshot and returns immediately. The **only** thing that starts
	/// probes — listing recents must never, or listing and probing would drive
	/// each other in a loop.
	pub fn submit(&self, entries: Vec<(String, PathBuf)>) -> u64 {
		let mut state = lock(&self.shared.state);
		state.generation += 1;
		let generation = state.generation;
		state.members.clear();
		state.queue.clear();

		// The membership set *is* the dedupe: `insert` answers "was this key already
		// in this snapshot", which is the question a second set would be asked. And
		// dedupe by key is required, because a recents list can legitimately hold two
		// spellings of one path — a hand-edited `%APPDATA%` entry beside the same
		// file opened through the picker — and they share one comparison key. Two
		// jobs for one key would be two probes of one file, and the second would
		// arrive as a duplicate result for a row that already had its answer.
		for (key, path) in entries {
			if !state.members.insert(key.clone()) {
				continue;
			}
			let job = Job {
				key: key.clone(),
				path,
				generation,
			};
			if state.in_flight.contains(&key) {
				// At most one attempt per key at a time; the newer request waits for
				// the running one to return rather than starting a second.
				state.rerun.insert(key, job);
			} else {
				state.queue.push_back(job);
			}
		}

		// The deadline is what makes the promise keepable when every thread is
		// blocked: an entry that never gets a thread still has an answer on time.
		state.due = Some((Instant::now() + self.shared.deadline, generation));
		drop(state);
		self.shared.work.notify_all();
		self.shared.timer.notify_all();

		generation
	}

	/// The cached answers for a set of keys, in the order asked. A pure read —
	/// nothing here starts work.
	pub fn cached(&self, keys: &[String]) -> Vec<(Availability, Option<String>)> {
		let state = lock(&self.shared.state);
		keys.iter()
			.map(|key| match state.cache.get(key) {
				Some(entry) => (entry.availability.clone(), entry.name.clone()),
				None => (Availability::Pending, None),
			})
			.collect()
	}

	/// Records an answer this layer learned by other means — an open that
	/// succeeded, or one that failed and was classified — so the switcher row
	/// agrees with what just happened without waiting for a probe.
	pub fn record(&self, key: &str, availability: Availability, name: Option<String>) {
		let mut state = lock(&self.shared.state);
		let generation = state.generation;
		state.cache.insert(
			key.to_string(),
			Cached {
				availability: availability.clone(),
				name: name.clone(),
				generation,
			},
		);
		drop(state);
		self.shared.sink.deliver(&ProbeResult {
			generation,
			key: key.to_string(),
			availability,
			name,
		});
	}

	/// Drops an entry's cached answer, so a removed and later re-added path is
	/// probed afresh rather than answered from a snapshot it was not in.
	pub fn forget(&self, key: &str) {
		let mut state = lock(&self.shared.state);
		state.cache.remove(key);
		state.members.remove(key);
	}

	#[cfg(test)]
	fn counts(&self) -> (u64, usize, usize) {
		let state = lock(&self.shared.state);
		(state.started, state.queue.len(), state.rerun.len())
	}
}

impl Drop for Executor {
	fn drop(&mut self) {
		let mut state = lock(&self.shared.state);
		state.shutdown = true;
		drop(state);
		self.shared.work.notify_all();
		self.shared.timer.notify_all();
	}
}

fn run_worker(shared: &Arc<Shared>) {
	loop {
		let job = {
			let mut state = lock(&shared.state);
			let job = loop {
				if state.shutdown {
					return;
				}
				// Stale work is dropped **before** it starts I/O, not after it returns:
				// a job for a superseded snapshot has no one to answer.
				match state.queue.pop_front() {
					Some(job)
						if job.generation == state.generation && state.members.contains(&job.key) =>
					{
						break job
					}
					Some(_) => continue,
					None => state = shared.work.wait(state).unwrap_or_else(|err| err.into_inner()),
				}
			};
			state.in_flight.insert(job.key.clone());
			state.started += 1;
			job
		};

		let (availability, name) = probe(shared.fs.as_ref(), &job.path);

		let mut state = lock(&shared.state);
		state.in_flight.remove(&job.key);
		// A refresh that arrived while this was blocked gets its turn now.
		if let Some(pending) = state.rerun.remove(&job.key) {
			state.queue.push_back(pending);
		}
		// Validation and the cache update happen under one lock acquisition, so a
		// result cannot be checked against one snapshot and applied to another.
		let fresh = job.generation == state.generation && state.members.contains(&job.key);
		if fresh {
			state.cache.insert(
				job.key.clone(),
				Cached {
					availability: availability.clone(),
					name: name.clone(),
					generation: job.generation,
				},
			);
		}
		drop(state);
		shared.work.notify_all();

		if fresh {
			shared.sink.deliver(&ProbeResult {
				generation: job.generation,
				key: job.key,
				availability,
				name,
			});
		}
	}
}

/// The one timer, waiting until whatever deadline is currently pending.
///
/// A submission moves the deadline rather than adding one, so a menu opened ten
/// times in a row leaves one thread waiting on one instant — not ten threads each
/// outliving the answer it was waiting for.
fn run_timer(shared: &Arc<Shared>) {
	let mut state = lock(&shared.state);
	loop {
		if state.shutdown {
			return;
		}
		let Some((at, generation)) = state.due else {
			state = shared.timer.wait(state).unwrap_or_else(|err| err.into_inner());
			continue;
		};

		let now = Instant::now();
		if now < at {
			// A spurious or early wake just re-reads `due`, which is the point of
			// looping on it rather than trusting the wait to mean "it is time".
			let (next, _) = shared
				.timer
				.wait_timeout(state, at - now)
				.unwrap_or_else(|err| err.into_inner());
			state = next;
			continue;
		}

		state.due = None;
		let owed = mark_unresponsive(&mut state, generation);
		drop(state);
		deliver(shared, generation, owed);
		state = lock(&shared.state);
	}
}

/// Everything in `generation` that has no answer *from* that generation by the
/// deadline resolves to `Unresponsive`. Returns the keys that were marked.
///
/// Note what this does not claim: an entry with four probes ahead of it that all
/// block forever resolves here rather than staying `Pending` indefinitely. The
/// residual limitation is real and is not implied away — if `MAX_IN_FLIGHT`
/// threads block and never return, no further probe can start, and recovery then
/// depends on Windows eventually unblocking them. Making that impossible needs
/// probe work in a killable helper process, which is a second process, its
/// lifecycle and its IPC in service of a menu that lists files.
fn mark_unresponsive(state: &mut State, generation: u64) -> Vec<String> {
	if state.generation != generation {
		return Vec::new();
	}
	let stale: Vec<String> = state
		.members
		.iter()
		.filter(|key| {
			state
				.cache
				.get(*key)
				.is_none_or(|entry| entry.generation < generation)
		})
		.cloned()
		.collect();

	for key in &stale {
		state.cache.insert(
			key.clone(),
			Cached {
				availability: Availability::unresponsive(),
				name: None,
				generation,
			},
		);
	}
	stale
}

/// Sends what the cache **currently** holds for each key, re-read under the lock
/// at delivery time.
///
/// Emitting cannot happen under the lock, so there is a gap between marking and
/// sending in which a real answer can land for one of these keys. Sending the
/// `Unresponsive` we decided on earlier would then overwrite a good result with a
/// stale timeout, and no further event would come to correct it. Re-reading means
/// the real answer is what goes out, and a key that has since been superseded
/// sends nothing at all.
fn deliver(shared: &Arc<Shared>, generation: u64, keys: Vec<String>) {
	if keys.is_empty() {
		return;
	}
	let results: Vec<ProbeResult> = {
		let state = lock(&shared.state);
		keys.into_iter()
			.filter_map(|key| {
				let entry = state.cache.get(&key)?;
				(entry.generation == generation && state.members.contains(&key)).then(|| ProbeResult {
					generation,
					key,
					availability: entry.availability.clone(),
					name: entry.name.clone(),
				})
			})
			.collect()
	};

	for result in &results {
		shared.sink.deliver(result);
	}
}

/// The snapshot to submit, derived from a recents list.
pub fn snapshot(recents: &[String]) -> Vec<(String, PathBuf)> {
	recents
		.iter()
		.map(|entry| {
			let path = PathBuf::from(entry);
			(comparison_key(&path), path)
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::mpsc::{channel, Sender};

	fn io(code: i32) -> std::io::Error {
		std::io::Error::from_raw_os_error(code)
	}

	fn reason(availability: &Availability) -> UnavailableReason {
		match availability {
			Availability::Unavailable { reason, .. } => *reason,
			other => panic!("expected an unavailable state, got {other:?}"),
		}
	}

	#[test]
	fn drive_and_network_codes_report_the_drive() {
		for code in [21, 53, 64, 67, 1231, 15, 55, 1112, 1167, 51, 321, 1201, 1222, 1232, 1233, 1256] {
			assert_eq!(
				reason(&classify(&io(code))),
				UnavailableReason::DriveUnavailable,
				"error {code} was not read as a drive failure"
			);
		}
	}

	#[test]
	fn missing_and_denied_are_distinguished_and_the_rest_degrade() {
		assert_eq!(reason(&classify(&io(2))), UnavailableReason::Missing);
		assert_eq!(reason(&classify(&io(3))), UnavailableReason::Missing);
		assert_eq!(reason(&classify(&io(5))), UnavailableReason::Unreadable);
		// Unmapped, so it must not be guessed into a drive failure.
		assert_eq!(reason(&classify(&io(1392))), UnavailableReason::Unreadable);
	}

	/// The one `Unreadable` that may name a cause, and the one that must not.
	#[test]
	fn only_access_denied_mentions_permission() {
		assert!(classify(&io(5)).message().unwrap().contains("permission"));
		assert!(!classify(&io(1392)).message().unwrap().contains("permission"));
	}

	#[test]
	fn every_cause_has_its_own_sentence() {
		let all = [
			UnavailableReason::DriveUnavailable,
			UnavailableReason::Missing,
			UnavailableReason::NotAFile,
			UnavailableReason::Unreadable,
			UnavailableReason::Invalid,
		];
		let mut seen: Vec<String> = all
			.iter()
			.map(|reason| Availability::unavailable(*reason).message().unwrap().to_string())
			.collect();
		seen.push(Availability::unresponsive().message().unwrap().to_string());
		let unique: HashSet<&String> = seen.iter().collect();
		assert_eq!(unique.len(), seen.len(), "two states share a sentence: {seen:?}");
		assert!(seen.iter().all(|sentence| !sentence.is_empty()));
	}

	#[test]
	fn the_wire_shape_carries_the_state_and_its_cause() {
		let json = serde_json::to_value(Availability::unavailable(UnavailableReason::DriveUnavailable))
			.unwrap();
		assert_eq!(json["state"], "unavailable");
		assert_eq!(json["reason"], "drive-unavailable");
		assert!(json["message"].as_str().unwrap().contains("drive"));
		assert_eq!(serde_json::to_value(Availability::Pending).unwrap()["state"], "pending");
	}

	/// The payload is the struct now, so the four field names the frontend mirrors
	/// are asserted here rather than at the emit site.
	#[test]
	fn a_result_serialises_as_the_availability_event_payload() {
		let json = serde_json::to_value(ProbeResult {
			generation: 7,
			key: r"D:\X\A.COPPER".to_string(),
			availability: Availability::Available,
			name: Some("work".to_string()),
		})
		.unwrap();

		assert_eq!(json["generation"], 7);
		assert_eq!(json["key"], r"D:\X\A.COPPER");
		assert_eq!(json["availability"]["state"], "available");
		assert_eq!(json["name"], "work");
		assert_eq!(json.as_object().unwrap().len(), 4, "the payload grew a field");

		let absent = serde_json::to_value(ProbeResult {
			generation: 1,
			key: "K".to_string(),
			availability: Availability::Pending,
			name: None,
		})
		.unwrap();
		assert!(absent["name"].is_null(), "an unread name must arrive as null");
	}

	// --- probing a real temp directory ---

	fn golden(name: &str) -> String {
		format!(
			"{{\n  \"id\": \"spc_00000001\",\n  \"name\": \"{name}\",\n  \"activeSection\": \
			 \"sec_00000001\",\n  \"sections\": [\n    {{\n      \"id\": \"sec_00000001\",\n      \
			 \"name\": \"Notes\",\n      \"order\": 0\n    }}\n  ],\n  \"notes\": []\n}}\n"
		)
	}

	#[test]
	fn a_valid_document_is_available_and_yields_its_name() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("a.copper");
		std::fs::write(&path, golden("development")).unwrap();

		let (availability, name) = probe(&RealFs, &path);

		assert_eq!(availability, Availability::Available);
		assert_eq!(name.as_deref(), Some("development"));
	}

	#[test]
	fn the_four_filesystem_causes_are_told_apart() {
		let dir = tempfile::tempdir().unwrap();

		let missing = dir.path().join("gone.copper");
		assert_eq!(reason(&probe(&RealFs, &missing).0), UnavailableReason::Missing);

		assert_eq!(reason(&probe(&RealFs, dir.path()).0), UnavailableReason::NotAFile);

		let invalid = dir.path().join("bad.copper");
		std::fs::write(&invalid, "not json").unwrap();
		assert_eq!(reason(&probe(&RealFs, &invalid).0), UnavailableReason::Invalid);
	}

	/// The cheap pre-check, which is the only place `DriveUnavailable` is
	/// concluded with no I/O error behind it. Injected rather than tested against
	/// a letter assumed absent — it may exist on the machine running this.
	#[test]
	fn an_absent_drive_letter_short_circuits() {
		struct NoDrives;
		impl Filesystem for NoDrives {
			fn is_dir(&self, _: &Path) -> std::io::Result<bool> {
				panic!("the filesystem was touched for an absent drive")
			}
			fn read(&self, _: &Path) -> std::io::Result<String> {
				unreachable!()
			}
			fn drive_present(&self, _: char) -> bool {
				false
			}
		}

		let (availability, _) = probe(&NoDrives, Path::new(r"Q:\notes\a.copper"));
		assert_eq!(reason(&availability), UnavailableReason::DriveUnavailable);
	}

	/// `metadata` succeeding does not prove the file is readable: a read failure
	/// must classify as unreadable, not as a corrupt document.
	#[test]
	fn a_read_failure_is_not_reported_as_a_corrupt_document() {
		struct Locked;
		impl Filesystem for Locked {
			fn is_dir(&self, _: &Path) -> std::io::Result<bool> {
				Ok(false)
			}
			fn read(&self, _: &Path) -> std::io::Result<String> {
				Err(io(5))
			}
			fn drive_present(&self, _: char) -> bool {
				true
			}
		}

		let (availability, _) = probe(&Locked, Path::new(r"C:\x\a.copper"));
		assert_eq!(reason(&availability), UnavailableReason::Unreadable);
	}

	// --- the executor ---

	struct Recorder(Sender<ProbeResult>);

	impl ResultSink for Recorder {
		fn deliver(&self, result: &ProbeResult) {
			let _ = self.0.send(result.clone());
		}
	}

	/// Answers instantly for anything under `fast\`, and blocks forever otherwise.
	struct Blocking {
		gate: Arc<(Mutex<bool>, Condvar)>,
	}

	impl Filesystem for Blocking {
		fn is_dir(&self, path: &Path) -> std::io::Result<bool> {
			if path.to_string_lossy().contains("fast") {
				return Ok(false);
			}
			let (mutex, condvar) = &*self.gate;
			let mut released = lock(mutex);
			while !*released {
				released = condvar.wait(released).unwrap_or_else(|err| err.into_inner());
			}
			Err(io(53))
		}
		fn read(&self, _: &Path) -> std::io::Result<String> {
			Ok(golden("fast"))
		}
		fn drive_present(&self, _: char) -> bool {
			true
		}
	}

	fn entries(paths: &[&str]) -> Vec<(String, PathBuf)> {
		paths
			.iter()
			.map(|path| (path.to_ascii_uppercase(), PathBuf::from(path)))
			.collect()
	}

	#[test]
	fn results_carry_their_snapshot_and_land_in_the_cache() {
		let (tx, rx) = channel();
		let executor = Executor::new(Box::new(RealFs), Box::new(Recorder(tx)));

		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("a.copper");
		std::fs::write(&path, golden("development")).unwrap();
		let key = comparison_key(&path);

		let generation = executor.submit(vec![(key.clone(), path)]);
		let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();

		assert_eq!(result.generation, generation);
		assert_eq!(result.key, key);
		assert_eq!(result.availability, Availability::Available);
		assert_eq!(executor.cached(&[key])[0].0, Availability::Available);
	}

	/// A10b. A result whose entry has since been removed is discarded rather than
	/// applied to whatever row now sits where it used to.
	#[test]
	fn a_result_for_a_superseded_snapshot_is_discarded() {
		let (tx, rx) = channel();
		let gate = Arc::new((Mutex::new(false), Condvar::new()));
		let executor = Executor::new(
			Box::new(Blocking {
				gate: Arc::clone(&gate),
			}),
			Box::new(Recorder(tx)),
		);

		executor.submit(entries(&[r"\\dead\share\a.copper"]));
		// The entry is gone from the next snapshot, so its in-flight probe has
		// nobody to answer.
		executor.submit(entries(&[r"C:\fast\b.copper"]));

		let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
		assert_eq!(first.key, r"C:\FAST\B.COPPER".to_string());

		*lock(&gate.0) = true;
		gate.1.notify_all();

		// The dead path's answer never arrives, because it was dropped.
		assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
		assert_eq!(executor.cached(&[r"\\DEAD\SHARE\A.COPPER".to_string()])[0].0, Availability::Pending);
	}

	/// A10. The deadline is measured from submission and includes queue time, so
	/// an entry that never gets a thread still has an answer — `Unresponsive`,
	/// which is a state and not a cause.
	#[test]
	fn a_probe_that_does_not_answer_in_time_becomes_unresponsive() {
		let (tx, rx) = channel();
		let gate = Arc::new((Mutex::new(false), Condvar::new()));
		let executor = Executor::with_deadline(
			Box::new(Blocking {
				gate: Arc::clone(&gate),
			}),
			Box::new(Recorder(tx)),
			Duration::from_millis(80),
		);

		executor.submit(entries(&[r"\\dead\share\a.copper"]));
		let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();

		assert_eq!(result.availability, Availability::unresponsive());
		assert!(
			!matches!(result.availability, Availability::Unavailable { .. }),
			"a timeout must not claim a cause"
		);

		*lock(&gate.0) = true;
		gate.1.notify_all();
	}

	/// A6. Two spellings of one path — a hand-edited `%APPDATA%` entry beside the
	/// same file opened through the picker — share one comparison key, and one key
	/// must mean one probe. Two jobs would be two reads of one file and a
	/// duplicate result for a row that already had its answer.
	#[test]
	fn a_repeated_key_in_one_snapshot_queues_a_single_job() {
		let (tx, _rx) = channel();
		let gate = Arc::new((Mutex::new(false), Condvar::new()));
		let executor = Executor::with_deadline(
			Box::new(Blocking {
				gate: Arc::clone(&gate),
			}),
			Box::new(Recorder(tx)),
			Duration::from_millis(50),
		);

		// One key, three entries, three different spellings of the path.
		let duplicated = vec![
			("\\\\DEAD\\SHARE\\A.COPPER".to_string(), PathBuf::from(r"\\dead\share\a.copper")),
			("\\\\DEAD\\SHARE\\A.COPPER".to_string(), PathBuf::from(r"\\DEAD\SHARE\A.COPPER")),
			("\\\\DEAD\\SHARE\\A.COPPER".to_string(), PathBuf::from(r"\\dead\share\.\a.copper")),
		];
		executor.submit(duplicated);
		std::thread::sleep(Duration::from_millis(30));

		let (started, queued, rerun) = executor.counts();
		assert_eq!(started, 1, "one key produced {started} probes");
		assert_eq!(queued, 0, "a duplicate key was queued behind the first");
		assert_eq!(rerun, 0, "a duplicate key was recorded as a rerun");

		*lock(&gate.0) = true;
		gate.1.notify_all();
	}

	/// A10b. Re-opening the menu with a dead path in the list must not accumulate
	/// work: at most `MAX_IN_FLIGHT` probes ever start, and at most one job per
	/// key is queued behind them.
	#[test]
	fn repeated_refreshes_do_not_accumulate_probes() {
		let (tx, _rx) = channel();
		let gate = Arc::new((Mutex::new(false), Condvar::new()));
		let executor = Executor::with_deadline(
			Box::new(Blocking {
				gate: Arc::clone(&gate),
			}),
			Box::new(Recorder(tx)),
			Duration::from_millis(50),
		);

		let dead = entries(&[
			r"\\dead\one\a.copper",
			r"\\dead\two\b.copper",
			r"\\dead\three\c.copper",
			r"\\dead\four\d.copper",
			r"\\dead\five\e.copper",
		]);
		for _ in 0..10 {
			executor.submit(dead.clone());
			std::thread::sleep(Duration::from_millis(10));
		}

		let (started, queued, rerun) = executor.counts();
		assert!(
			started <= MAX_IN_FLIGHT as u64,
			"{started} probes started with only {MAX_IN_FLIGHT} threads"
		);
		assert!(queued <= dead.len(), "the queue grew past one snapshot: {queued}");
		assert!(rerun <= dead.len(), "reruns accumulated: {rerun}");

		*lock(&gate.0) = true;
		gate.1.notify_all();
	}
}
