//! `copper attachment export` — a note's files, copied out under their original
//! names.
//!
//! Reads only. It never touches the sidecar beyond opening blobs through
//! `resolve_existing`, and it **never calls `attachments::sweep`** — sweep moves
//! unreferenced blobs to quarantine, and a CLI cannot know whether a running
//! app's undo stack is holding snapshots that still reference them.

use std::path::Path;

use copper_core::attachments;
use copper_core::store::error::{io_err, Result};
use copper_core::store::{path_string, Store};

use crate::output::{ExportRow, FailedExport, Report};
use crate::resolve;

pub fn run(store: &Store, reference: &str, out: Option<&Path>) -> Result<Report> {
	let space_path = store.require_active_path()?;
	let space = store.active_space()?;
	let note = resolve::note(&space, reference)?;

	let target = match out {
		Some(dir) => resolve::absolute(dir)?,
		None => resolve::working_dir()?,
	};
	if !note.attachments.is_empty() {
		std::fs::create_dir_all(&target).map_err(|err| io_err(&target, "create", &err))?;
	}

	let mut exported = Vec::new();
	let mut failed = Vec::new();

	for attachment in &note.attachments {
		// One attachment's failure must not sink the rest: the command exports
		// everything on a note in one call, and a single missing blob is a reason
		// to report that blob, not to abandon the other nine files the user asked
		// for. The exit code below still says the command did not fully succeed.
		match attachments::export::export_one(
			&space_path,
			&target,
			&attachment.file,
			&attachment.name,
		) {
			Ok(written) => exported.push(ExportRow {
				name: attachment.name.clone(),
				path: path_string(&written),
				bytes: attachment.bytes,
			}),
			Err(err) => failed.push(FailedExport {
				name: attachment.name.clone(),
				message: err.message(),
			}),
		}
	}

	Ok(Report::Export { exported, failed })
}
