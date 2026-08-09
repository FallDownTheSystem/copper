//! `copper section …`
//!
//! Every mutation goes through `Store::mutate`, which is the app's own
//! compare-and-swap write. Section references are resolved to ids against the
//! document *before* the mutation is built, because `ops` takes ids only.

use copper_core::store::error::Result;
use copper_core::store::{ops, Store};

use crate::cli::SectionCommand;
use crate::output::{Report, SectionRow};
use crate::resolve;

pub fn run(store: &mut Store, command: &SectionCommand) -> Result<Report> {
	match command {
		SectionCommand::List => list(store),
		SectionCommand::Add { name } => add(store, name),
		SectionCommand::Rename { reference, name } => rename(store, reference, name),
		SectionCommand::Delete { reference } => delete(store, reference),
	}
}

fn list(store: &Store) -> Result<Report> {
	let space = store.active_space()?;
	let rows = space
		.sections
		.iter()
		.map(|section| SectionRow {
			id: section.id.clone(),
			name: section.name.clone(),
			order: section.order,
			active: space.active_section == section.id,
			notes: space
				.notes
				.iter()
				.filter(|note| note.section == section.id)
				.count(),
		})
		.collect();
	Ok(Report::Sections(rows))
}

fn add(store: &mut Store, name: &str) -> Result<Report> {
	let (id, _) = store.mutate(|space| ops::add_section(space, name))?;
	Ok(Report::Id(id))
}

/// The reference is resolved once, against the document as it is now, and the
/// resulting id is what the closure captures.
///
/// That matters on the conflict path: `mutate` re-applies its closure to a
/// freshly re-read document, and a closure that re-resolved a *name* each time
/// could rename a different section on the second attempt than it picked on the
/// first. An id cannot drift that way.
fn rename(store: &mut Store, reference: &str, name: &str) -> Result<Report> {
	let space = store.active_space()?;
	let id = resolve::section(&space, reference)?.to_string();
	store.mutate(|space| ops::rename_section(space, &id, name))?;
	Ok(Report::Id(id))
}

fn delete(store: &mut Store, reference: &str) -> Result<Report> {
	let space = store.active_space()?;
	let id = resolve::section(&space, reference)?.to_string();
	store.mutate(|space| ops::delete_section(space, &id))?;
	Ok(Report::Id(id))
}
