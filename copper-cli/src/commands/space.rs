//! `copper space …` — which spaces exist, and which one this CLI works on.
//!
//! **No command here opens a `Store`.** `list` reads `settings.json` through the
//! decode-only loader and probes each entry; `use`/`current`/`clear` touch only
//! the CLI's own state file; `create` writes one document through
//! `create_headless`. Nothing bootstraps, nothing reorders recents, nothing
//! creates a default space.

use std::path::Path;

use copper_core::spaces::availability::{self, Availability, RealFs};
use copper_core::store::error::{Result, StoreError};
use copper_core::store::{self, format, settings};

use crate::cli::SpaceCommand;
use crate::output::{Report, SpaceRow};
use crate::resolve;

pub fn run(command: &SpaceCommand) -> Result<Report> {
	match command {
		SpaceCommand::List => list(),
		SpaceCommand::Use { path } => select(path),
		SpaceCommand::Current => current(),
		SpaceCommand::Clear => clear(),
		SpaceCommand::Create { path, name } => create(path, name.as_deref()),
	}
}

/// The app's recents, each classified by a live probe.
///
/// Running this twice must leave `settings.json` byte-identical, which is why it
/// goes through `load_read_only` and never through `settings::load` or
/// `bootstrap_store`.
///
/// One accepted cost: `probe` can block on an unresponsive network path.
/// `GetLogicalDrives` short-circuits an absent *drive letter* instantly, but a
/// UNC path has no letter to check, so a dead share is answered by the
/// redirector's own timeout. The app avoids this by probing on a background
/// worker with a deadline; a CLI has nowhere to put that work and no UI to update
/// afterwards, so it waits.
fn list() -> Result<Report> {
	let loaded = settings::load_read_only(&resolve::settings_path()?);
	let active = loaded.settings.active_recent().map(str::to_string);

	let rows = loaded
		.settings
		.recents
		.iter()
		.map(|entry| {
			let path = Path::new(entry);
			let (availability, name) = availability::probe(&RealFs, path);
			SpaceRow {
				path: entry.clone(),
				active: active.as_deref() == Some(entry.as_str()),
				// The document's own `name`, or `null` when the probe could not read
				// one. Deliberately **not** a file-stem fallback: `name` means "what
				// this document calls itself", and a stem is a guess from the path a
				// consumer already has. The human listing does substitute the stem, so
				// a row still reads as something recognisable — that is a rendering
				// choice, and `output.rs` is where it belongs.
				name,
				availability,
			}
		})
		.collect();

	Ok(Report::Spaces(rows))
}

/// Points the CLI at a space, for every later invocation from any directory.
///
/// The path is probed before it is stored. Storing an unopenable path would move
/// the failure to whatever command the user ran next, with a message about that
/// command rather than about the typo — and the probe already has one sentence
/// per cause ready to say instead.
fn select(path: &Path) -> Result<Report> {
	let resolved = resolve::absolute(path)?;
	let probed = availability::probe(&RealFs, &resolved).0;
	if probed != Availability::Available {
		let why = probed.message().unwrap_or("this space cannot be opened");
		let described = format!("{}: {why}", resolved.display());
		return Err(match probed {
			Availability::Unavailable {
				reason: availability::UnavailableReason::Missing,
				..
			} => StoreError::NotFound(described),
			_ => StoreError::Unavailable(described),
		});
	}

	// Canonical where possible, so two spellings of one path do not read as two
	// selections. `probe` has just proved the file is there, so this rarely falls
	// back — but a path that resolves through a junction still should not be
	// stored verbatim.
	let stored = store::canonical(&resolved).unwrap_or(resolved);
	let text = store::path_string(&stored);
	resolve::save_state(&resolve::CliState {
		space: Some(text.clone()),
	})?;
	Ok(Report::Selection(Some(text)))
}

fn current() -> Result<Report> {
	Ok(Report::Selection(resolve::load_state().space))
}

fn clear() -> Result<Report> {
	resolve::clear_state()?;
	Ok(Report::Selection(None))
}

/// Creates a space and stops there.
///
/// It deliberately does **not** also select it: `space create` and `space use`
/// are two verbs in the spec, and a create that silently repointed the CLI would
/// make `copper space create backup.copper` change where the next `note add`
/// lands.
fn create(path: &Path, name: Option<&str>) -> Result<Report> {
	let resolved = resolve::absolute(path)?;
	let name = match name {
		Some(given) => given.to_string(),
		// The file stem, so `copper space create work.copper` needs no second
		// argument to produce a document called "work".
		None => resolved
			.file_stem()
			.map(|stem| stem.to_string_lossy().into_owned())
			.unwrap_or_default(),
	};

	store::create_headless(&resolved, &name)?;

	// Re-read rather than threading a return value out of `create_headless`: the
	// file is the source of truth for what was written, and its id was minted
	// inside the write.
	let written = format::parse_normalised(&store::atomic::read_with_backoff(&resolved)?)?;
	Ok(Report::Created {
		path: store::path_string(&resolved),
		id: written.id,
		name: written.name,
	})
}
