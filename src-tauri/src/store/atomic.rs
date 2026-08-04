//! The only place the store writes bytes to disk.
//!
//! Write-to-temp-then-rename, with the two halves deliberately separable. The
//! split is not tidiness: `mutate` has to put its write-conflict comparison
//! *between* them (spec 2.1), so that the window between "checked the file" and
//! "replaced the file" is the rename call rather than the whole serialise-write-
//! fsync sequence. Fusing the phases would widen an accepted race by orders of
//! magnitude.
//!
//! Two limits are worth stating so the guarantee is not read as stronger than
//! it is. The rename is atomic but **not durable** (spec 2.9a): `sync_all` on
//! the temp file puts the *content* on disk before the rename, which is what
//! makes a partial file unobservable, but `persist` does not fsync the
//! containing directory and Windows exposes no equivalent. And atomicity holds
//! on local NTFS; on a network filesystem it cannot be guaranteed.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use tempfile::NamedTempFile;

use super::error::{io_err, Result, StoreError};

/// `ERROR_SHARING_VIOLATION`. What *opening* a file someone else holds without
/// sharing returns — antivirus, the search indexer, OneDrive, git's own handles.
const ERROR_SHARING_VIOLATION: i32 = 32;

/// `ERROR_ACCESS_DENIED`.
///
/// Spec 2.2 named only error 32 and listed "permission denied" among the
/// permanent failures, which turned out to be half right and was found by the
/// test for A9.22. Renaming over a destination that another process holds open
/// without `FILE_SHARE_DELETE` does not fail with 32 — `MoveFileExW` reports
/// **5**, `ERROR_ACCESS_DENIED`, the same code a genuinely read-only file gives.
/// Treating 5 as permanent therefore meant the single most common transient
/// case, an antivirus scanner holding the space file for a few hundred
/// milliseconds, lost the write outright.
///
/// It is treated as transient **at the rename step only**, and the distinction
/// is principled rather than a shrug: reaching the rename means a temp file was
/// already created, written and fsynced in that same directory, which proves the
/// directory is writable. What remains is far more likely to be a transient
/// handle on the destination than a permissions problem. The cost when it really
/// is permanent — a read-only destination — is half a second before returning
/// the same error, which is a good trade against losing a capture.
const ERROR_ACCESS_DENIED: i32 = 5;

/// Four sleeps between five attempts, summing to 500 ms (spec 2.2).
pub const BACKOFF_DELAYS: [u64; 4] = [25, 75, 150, 250];

/// What one try of a retryable operation produced.
pub enum Attempt<T> {
	Done(T),
	/// Worth trying again shortly — a sharing violation, or the brief window in
	/// which git has unlinked a file and not yet finished writing its
	/// replacement (spec 2.3b).
	Transient(StoreError),
	/// A read-only file, a missing directory, denied permission: retrying only
	/// wastes half a second before returning the same error.
	Failed(StoreError),
}

/// Runs `attempt` up to five times over roughly 500 ms, stopping early on
/// success or on a permanent failure.
pub fn with_backoff<T>(mut attempt: impl FnMut() -> Attempt<T>) -> Result<T> {
	for delay in BACKOFF_DELAYS {
		match attempt() {
			Attempt::Done(value) => return Ok(value),
			Attempt::Failed(err) => return Err(err),
			Attempt::Transient(_) => std::thread::sleep(Duration::from_millis(delay)),
		}
	}
	match attempt() {
		Attempt::Done(value) => Ok(value),
		Attempt::Failed(err) | Attempt::Transient(err) => Err(err),
	}
}

/// Whether a failed rename is worth another try shortly.
pub fn is_transient_commit_failure(err: &std::io::Error) -> bool {
	matches!(
		err.raw_os_error(),
		Some(ERROR_SHARING_VIOLATION | ERROR_ACCESS_DENIED)
	)
}

/// A temp file whose content is already on disk, waiting to be renamed.
#[derive(Debug)]
pub struct Prepared {
	file: NamedTempFile,
}

/// A failed rename, handing the prepared file back so a retry does not have to
/// serialise and fsync the same bytes again.
#[derive(Debug)]
pub struct CommitFailure {
	pub prepared: Prepared,
	pub error: std::io::Error,
}

/// Writes `text` into a temp file in `dir` and flushes it to the platter.
///
/// `dir` must be the destination's own directory: `persist` is a rename, and a
/// rename cannot cross volumes.
///
/// A failure here is not retried by callers, and the omission is deliberate
/// rather than an oversight — creating a fresh randomly-named file cannot
/// realistically lose a sharing race, so every failure at this step is the
/// permanent kind (the directory does not exist, or is not writable).
pub fn prepare(dir: &Path, text: &str) -> Result<Prepared> {
	let mut file = NamedTempFile::new_in(dir)
		.map_err(|err| io_err(dir, "create a temporary file in", &err))?;
	file.write_all(text.as_bytes())
		.map_err(|err| io_err(file.path(), "write", &err))?;
	file.as_file()
		.sync_all()
		.map_err(|err| io_err(file.path(), "flush", &err))?;
	Ok(Prepared { file })
}

impl Prepared {
	/// Replaces `path`, atomically. Any previous content is gone.
	pub fn commit(self, path: &Path) -> std::result::Result<(), CommitFailure> {
		self.file
			.persist(path)
			.map(|_| ())
			.map_err(|err| CommitFailure {
				prepared: Prepared { file: err.file },
				error: err.error,
			})
	}

	/// Creates `path`, refusing if anything is already there.
	///
	/// This is what makes `create_space` safe against a file that appears between
	/// an `exists()` check and the write (A9.30): the refusal comes from the
	/// filesystem's own create-exclusive, so there is no window to lose.
	pub fn commit_new(self, path: &Path) -> std::result::Result<(), CommitFailure> {
		self.file
			.persist_noclobber(path)
			.map(|_| ())
			.map_err(|err| CommitFailure {
				prepared: Prepared { file: err.file },
				error: err.error,
			})
	}
}

/// prepare + commit in one call, retrying transient sharing violations.
///
/// For single-writer destinations only — `settings.json` and the initial
/// creation of a space document. A space document being *edited* must not use
/// this: the backoff between attempts is hundreds of milliseconds, ample for an
/// external writer to land, and this loop would blindly overwrite it. That path
/// repeats its conflict comparison before every attempt instead (spec 2.2a),
/// which is why `mutate` drives `with_backoff` itself.
pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
	let dir = parent_dir(path)?;
	let mut held: Option<Prepared> = None;
	with_backoff(|| {
		let prepared = match held.take() {
			Some(prepared) => prepared,
			None => match prepare(dir, text) {
				Ok(prepared) => prepared,
				Err(err) => return Attempt::Failed(err),
			},
		};
		match prepared.commit(path) {
			Ok(()) => Attempt::Done(()),
			Err(failure) => {
				let transient = is_transient_commit_failure(&failure.error);
				let err = io_err(path, "write", &failure.error);
				if transient {
					held = Some(failure.prepared);
					Attempt::Transient(err)
				} else {
					Attempt::Failed(err)
				}
			}
		}
	})
}

/// The directory a file lives in, which is where its temp file must go.
pub fn parent_dir(path: &Path) -> Result<&Path> {
	path.parent()
		.filter(|parent| !parent.as_os_str().is_empty())
		.ok_or_else(|| StoreError::Invalid(format!("{} has no parent directory", path.display())))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::time::Instant;

	fn entries(dir: &Path) -> Vec<String> {
		let mut names: Vec<String> = std::fs::read_dir(dir)
			.unwrap()
			.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
			.collect();
		names.sort();
		names
	}

	#[test]
	fn writes_the_content_and_leaves_no_temp_file() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("space.copper");

		write_atomic(&path, "hello\n").unwrap();

		assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
		assert_eq!(entries(dir.path()), ["space.copper"]);
	}

	#[test]
	fn replaces_existing_content() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("space.copper");

		write_atomic(&path, "first\n").unwrap();
		write_atomic(&path, "second\n").unwrap();

		assert_eq!(std::fs::read_to_string(&path).unwrap(), "second\n");
		assert_eq!(entries(dir.path()), ["space.copper"]);
	}

	#[test]
	fn commit_new_refuses_an_existing_destination() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("space.copper");
		std::fs::write(&path, "existing\n").unwrap();

		let prepared = prepare(dir.path(), "replacement\n").unwrap();
		assert!(prepared.commit_new(&path).is_err());

		assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing\n");
		assert_eq!(entries(dir.path()), ["space.copper"]);
	}

	#[test]
	fn commit_new_creates_a_missing_destination() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("space.copper");

		prepare(dir.path(), "fresh\n").unwrap().commit_new(&path).unwrap();

		assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh\n");
		assert_eq!(entries(dir.path()), ["space.copper"]);
	}

	#[test]
	fn a_failed_write_leaves_the_previous_content_intact() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("space.copper");
		write_atomic(&path, "original\n").unwrap();

		// A directory is never a valid rename destination, so this fails at the
		// commit — after the temp file has been written and synced.
		let blocked = dir.path().join("blocked");
		std::fs::create_dir(&blocked).unwrap();
		assert!(write_atomic(&blocked, "nope\n").is_err());

		assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\n");
		assert_eq!(entries(dir.path()), ["blocked", "space.copper"]);
	}

	#[test]
	fn a_missing_directory_fails_immediately_rather_than_backing_off() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("no-such-dir").join("space.copper");

		let started = Instant::now();
		let err = write_atomic(&path, "nope\n").unwrap_err();

		assert_eq!(err.kind(), "not-found");
		assert!(
			started.elapsed() < Duration::from_millis(200),
			"a permanent failure was retried: took {:?}",
			started.elapsed()
		);
	}

	#[test]
	fn backoff_stops_early_on_success() {
		let mut calls = 0;
		let result: Result<u8> = with_backoff(|| {
			calls += 1;
			if calls < 3 {
				Attempt::Transient(StoreError::Io("busy".into()))
			} else {
				Attempt::Done(7)
			}
		});
		assert_eq!(result.unwrap(), 7);
		assert_eq!(calls, 3);
	}

	#[test]
	fn backoff_gives_up_after_five_attempts() {
		let mut calls = 0;
		let result: Result<u8> = with_backoff(|| {
			calls += 1;
			Attempt::Transient(StoreError::Io("still busy".into()))
		});
		assert!(result.is_err());
		assert_eq!(calls, BACKOFF_DELAYS.len() + 1);
	}

	#[test]
	fn a_path_without_a_parent_is_rejected() {
		assert!(parent_dir(Path::new("space.copper")).is_err());
	}
}
