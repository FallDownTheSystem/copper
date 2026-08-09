//! Collecting the blobs no note references any more.
//!
//! It is on this side of the crate boundary because every failure it has is
//! logged rather than propagated, and the log is this crate's `diagnostics` —
//! `OutputDebugStringW` in a release build. The layout it works over
//! (`assets_dir`, [`COLLECTED_DIR`], [`ORPHAN_GRACE`]) belongs to
//! [`copper_core::attachments`], which is where the sidecar is defined.

use std::path::Path;
use std::time::SystemTime;

use copper_core::attachments::{assets_dir, COLLECTED_DIR, ORPHAN_GRACE};
use copper_core::store::model::Space;

use crate::diagnostics;

/// Every `file` the document references, whether or not it is a valid name.
///
/// Invalid entries are included deliberately: a blob whose name a corrupt entry
/// happens to spell is still referenced by the user's intent, and the sweep
/// deleting it would turn a repairable typo into lost bytes.
fn referenced(doc: &Space) -> std::collections::HashSet<&str> {
	doc.notes
		.iter()
		.flat_map(|note| note.attachments.iter())
		.map(|attachment| attachment.file.as_str())
		.collect()
}

/// The outcome of getting rid of one blob, spelled out in full because this
/// module's own `Result` is the store's.
type Disposed = std::result::Result<(), String>;

/// Moves a collected blob into [`COLLECTED_DIR`], which destroys nothing.
///
/// **Why this is a rename and not the Recycle Bin.** Task-015 shipped the bin
/// first and the mechanism was wrong. `IFileOperation` with `FOF_ALLOWUNDO`
/// **permanently deletes the file and returns `S_OK`** when the shell cannot
/// recycle — a bin over its quota, a bin switched off, a FAT32 stick, a network
/// share — and `GetAnyOperationsAborted` stays `FALSE`, so no error ever reaches
/// the caller and no failure branch can run. Measured on this Windows build,
/// through the raw COM interface and through the `trash` crate alike. That
/// inverts the entire point: the people the recovery window existed for were
/// exactly the people whose bytes it destroyed without saying so.
///
/// A rename has no such mode. It moves a directory entry within one volume and
/// either succeeds or returns an error, and every error takes the log-and-leave
/// branch in [`sweep_with`]. The destination is a child of the blob's own
/// directory, so it is the same volume by construction and the move can never
/// degrade into a copy-and-delete across a boundary.
///
/// A name already in the quarantine is overwritten, and that is safe because the
/// names are content addresses: the same sixteen hex characters mean the same
/// bytes, so the file being replaced is a copy of the file replacing it. The
/// exception is a deliberately engineered 64-bit prefix collision — and there
/// the loss is one unreferenced orphan overwriting another, both already
/// collected, which does not justify a second directory level to prevent.
fn quarantine(path: &Path) -> Disposed {
	let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
		return Err("it has no name inside a directory".to_string());
	};
	let collected = dir.join(COLLECTED_DIR);
	// On demand rather than at startup: a space that never orphans a blob never
	// grows the directory, and an empty one would only invite the question.
	std::fs::create_dir_all(&collected).map_err(|err| err.to_string())?;
	std::fs::rename(path, collected.join(name)).map_err(|err| err.to_string())
}

/// Moves blobs no note references, and that nothing has touched for
/// [`ORPHAN_GRACE`], into [`COLLECTED_DIR`].
///
/// Best-effort throughout: every failure is logged and skipped, nothing is
/// propagated, and a missing directory is the ordinary case for a space that
/// has never had an attachment.
///
/// **It runs on the thread that asked for it, not off it.**
/// `spaces::sweep_detached` detaches the *snapshot*, not the execution: it
/// clones the path and document out of the store and drops the guard before the
/// walk, per task-006's two-lock discipline, so a directory walk never stalls a
/// capture. The walk itself is synchronous, and on a slow volume it does hold up
/// the switch that started it. That is the pre-existing arrangement and this
/// mechanism does not change its cost — a same-volume rename is the same order
/// of work as the `remove_file` it replaced — and the one sweep where the wait
/// would have been felt, at startup, is already on a thread of its own.
///
/// **What a failure costs, stated plainly.** A blob whose move fails is logged
/// and left where it is for the next sweep to retry. The log is
/// `OutputDebugStringW` in a release build, so without DebugView attached nobody
/// sees it: the retry is silent. And [`COLLECTED_DIR`] never shrinks on its own
/// — it grows for the life of the space until somebody empties it by hand.
/// Auto-purge was considered for task-015 and deliberately not built; the lever
/// is a person deleting the directory, which is safe at any moment because
/// nothing in Copper ever reads from it.
pub fn sweep(space_path: &Path, doc: &Space) {
	sweep_with(space_path, doc, quarantine);
}

/// [`sweep`], with the disposal step as a parameter.
///
/// The seam is for the unit tests in this file, which need to drive a
/// *particular* disposal — one that records what it was handed, or one that
/// fails on demand, which is the only way the failure branch below is reachable
/// at all. The production path is safe to run in a test as it stands, since a
/// rename inside a `tempfile::tempdir()` touches nothing outside it, so the
/// tests that only care about the outcome call [`sweep`] itself.
fn sweep_with(space_path: &Path, doc: &Space, mut dispose: impl FnMut(&Path) -> Disposed) {
	let dir = assets_dir(space_path);
	let entries = match std::fs::read_dir(&dir) {
		Ok(entries) => entries,
		// Never had an attachment, or the directory went away with the drive.
		Err(_) => return,
	};

	let live = referenced(doc);
	let now = SystemTime::now();
	for entry in entries.flatten() {
		let name = entry.file_name();
		let Some(name) = name.to_str() else { continue };
		if live.contains(name) {
			continue;
		}
		// A subdirectory, or a temp file some other process is mid-write on. The
		// metadata read failing is itself a reason to leave it alone.
		let Ok(metadata) = entry.metadata() else { continue };
		if !metadata.is_file() {
			continue;
		}
		let old_enough = metadata
			.modified()
			.ok()
			.and_then(|modified| now.duration_since(modified).ok())
			.is_some_and(|age| age >= ORPHAN_GRACE);
		if !old_enough {
			continue;
		}
		let path = entry.path();
		if let Err(err) = dispose(&path) {
			diagnostics::log_error(&format!(
				"[copper] could not move the unreferenced attachment {} into {COLLECTED_DIR}, so it \
				 stays where it is until the next sweep: {err}",
				path.display()
			));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use std::time::Duration;

	use copper_core::store::model::{Attachment, Note, Section};

	fn space_with(files: &[&str]) -> Space {
		Space {
			id: "spc_00000001".into(),
			name: "test".into(),
			active_section: "sec_a".into(),
			sections: vec![Section {
				id: "sec_a".into(),
				name: "Notes".into(),
				order: 0,
			}],
			notes: vec![Note {
				id: "nte_1".into(),
				section: "sec_a".into(),
				order: 0,
				done: false,
				body: "a note".into(),
				attachments: files
					.iter()
					.map(|file| Attachment {
						id: "att_1".into(),
						file: (*file).to_string(),
						name: (*file).to_string(),
						mime: "image/png".into(),
						bytes: 1,
						width: None,
						height: None,
					})
					.collect(),
				created: "2026-08-04T14:12:33Z".into(),
				updated: "2026-08-04T14:12:33Z".into(),
			}],
		}
	}

	/// Backdates a file so the grace window has demonstrably elapsed, rather
	/// than making the test sleep for a day.
	fn age(path: &Path) {
		let old = SystemTime::now() - ORPHAN_GRACE - Duration::from_secs(60);
		let file = std::fs::File::options().write(true).open(path).unwrap();
		file.set_modified(old).unwrap();
	}

	/// A path's own name, which is what every assertion below is really about.
	fn file_name(path: &Path) -> String {
		path.file_name().unwrap_or_default().to_string_lossy().into_owned()
	}

	/// AC13, both halves: an old orphan goes and a young one stays. Task-015
	/// adds where it goes, and that is the assertion that matters most — the
	/// bytes are still there afterwards, under their own name, byte for byte.
	///
	/// Runs the production [`sweep`], not the seam: the mechanism is a rename
	/// inside the fixture's own `tempfile::tempdir()`, so there is nothing left
	/// to protect the machine running the tests from.
	#[test]
	fn the_sweep_collects_old_orphans_and_leaves_referenced_and_young_ones() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let assets = assets_dir(&space);
		std::fs::create_dir_all(&assets).unwrap();
		for name in ["kept.png", "old-orphan.png", "young-orphan.png"] {
			std::fs::write(assets.join(name), name.as_bytes()).unwrap();
		}
		age(&assets.join("kept.png"));
		age(&assets.join("old-orphan.png"));

		sweep(&space, &space_with(&["kept.png"]));

		assert!(assets.join("kept.png").exists(), "a referenced blob was collected");
		assert!(
			assets.join("young-orphan.png").exists(),
			"an orphan inside the grace window was collected"
		);
		assert!(
			!assets.join("old-orphan.png").exists(),
			"an old orphan survived the sweep"
		);
		// The point of the whole task: collected is not destroyed.
		assert_eq!(
			std::fs::read(assets.join(COLLECTED_DIR).join("old-orphan.png")).unwrap(),
			b"old-orphan.png",
			"the collected blob was destroyed rather than moved aside"
		);
	}

	/// Task-015 AC3. A blob whose move fails stays exactly where it is — nothing
	/// falls back to deleting it — and the failure does not stop the sweep from
	/// reaching the orphans behind it.
	///
	/// **Every** disposal fails, which is what makes this deterministic. Failing
	/// only the first entry would leave the assertion depending on `read_dir`
	/// order, which is not defined and is not the same on every filesystem.
	#[test]
	fn a_failed_move_leaves_every_blob_alone_and_the_sweep_carries_on() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let assets = assets_dir(&space);
		std::fs::create_dir_all(&assets).unwrap();
		for name in ["a-orphan.png", "b-orphan.png"] {
			std::fs::write(assets.join(name), b"x").unwrap();
			age(&assets.join(name));
		}

		let mut seen = Vec::new();
		sweep_with(&space, &space_with(&[]), |path| {
			seen.push(file_name(path));
			Err("the volume is read-only".to_string())
		});

		seen.sort();
		assert_eq!(
			seen,
			["a-orphan.png", "b-orphan.png"],
			"the sweep stopped at the first failure instead of carrying on"
		);
		assert!(assets.join("a-orphan.png").exists(), "a blob was deleted after a failure");
		assert!(assets.join("b-orphan.png").exists(), "a blob was deleted after a failure");
	}

	/// The same content being collected twice is the ordinary case — one
	/// screenshot attached, orphaned, re-attached and orphaned again — and it
	/// must not fail. Content addressing is what makes overwriting safe: the
	/// name already in the quarantine denotes these exact bytes.
	#[test]
	fn collecting_a_name_already_in_the_quarantine_succeeds() {
		let dir = tempfile::tempdir().unwrap();
		let assets = dir.path().join("notes.copper.assets");
		std::fs::create_dir_all(assets.join(COLLECTED_DIR)).unwrap();
		std::fs::write(assets.join(COLLECTED_DIR).join("dupe.png"), b"same").unwrap();
		std::fs::write(assets.join("dupe.png"), b"same").unwrap();

		quarantine(&assets.join("dupe.png")).expect("a repeat collection must not fail");

		assert!(!assets.join("dupe.png").exists());
		assert_eq!(
			std::fs::read(assets.join(COLLECTED_DIR).join("dupe.png")).unwrap(),
			b"same"
		);
	}

	/// The quarantine is not itself swept. It is a directory, so the `is_file`
	/// gate skips it — but a regression there would move `.collected` inside
	/// itself on every startup, so it is worth an assertion of its own.
	#[test]
	fn a_second_sweep_leaves_the_quarantine_alone() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let assets = assets_dir(&space);
		std::fs::create_dir_all(&assets).unwrap();
		std::fs::write(assets.join("orphan.png"), b"x").unwrap();
		age(&assets.join("orphan.png"));

		sweep(&space, &space_with(&[]));
		sweep(&space, &space_with(&[]));

		assert!(assets.join(COLLECTED_DIR).join("orphan.png").is_file());
		assert!(
			!assets.join(COLLECTED_DIR).join(COLLECTED_DIR).exists(),
			"the quarantine swept itself"
		);
	}

	#[test]
	fn sweeping_a_space_with_no_assets_directory_is_a_no_op() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		sweep(&space, &space_with(&[]));
		assert!(!assets_dir(&space).exists());
	}

	/// A corrupt `file` value protects the blob it names rather than condemning
	/// it: the entry is repairable by hand, the bytes are not recoverable.
	#[test]
	fn a_blob_named_by_an_invalid_entry_is_not_collected() {
		let dir = tempfile::tempdir().unwrap();
		let space = dir.path().join("notes.copper");
		let assets = assets_dir(&space);
		std::fs::create_dir_all(&assets).unwrap();
		std::fs::write(assets.join("kept.png"), b"x").unwrap();
		age(&assets.join("kept.png"));

		// The document spells it as an escaping path, which `resolve` refuses —
		// but the intent still names these bytes.
		sweep(&space, &space_with(&[r"..\kept.png", "kept.png"]));

		assert!(assets.join("kept.png").exists());
		assert!(
			!assets.join(COLLECTED_DIR).exists(),
			"a protected blob was collected anyway"
		);
	}
}
