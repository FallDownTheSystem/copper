//! `copper note …`
//!
//! Two decisions here are inherited rather than chosen, and both are worth
//! naming because a reader could reasonably expect otherwise:
//!
//! - **`add` never classifies `# Name` as a section directive.** That is
//!   composer-only behaviour (`entry::classify`, reached from `submit_entry`);
//!   `append_capture` does not classify either, and this is the same kind of
//!   caller. A note whose first line is a heading is a note with a heading in it.
//! - **The insertion point is fixed, not read from settings.** `--top` overrides
//!   it. A programmatic caller wants the same answer on every machine, and the
//!   app's `insertionPoint` is a preference about the composer.

use copper_core::store::error::Result;
use copper_core::store::settings::InsertionPoint;
use copper_core::store::{ops, Store};

use crate::cli::{NoteAddArgs, NoteCommand, NoteEditArgs, NoteListArgs};
use crate::commands::{body, note_row};
use crate::output::Report;
use crate::resolve;

pub fn run(store: &mut Store, command: &NoteCommand) -> Result<Report> {
	match command {
		NoteCommand::List(args) => list(store, args),
		NoteCommand::Add(args) => add(store, args),
		NoteCommand::Edit(args) => edit(store, args),
		NoteCommand::Delete { ids } => delete(store, ids),
		NoteCommand::Move { ids, section } => move_to(store, ids, section),
		NoteCommand::Done { ids } => set_done(store, ids, true),
		NoteCommand::Undone { ids } => set_done(store, ids, false),
		NoteCommand::Merge { ids } => merge(store, ids),
	}
}

fn list(store: &Store, args: &NoteListArgs) -> Result<Report> {
	let space = store.active_space()?;
	let section = match &args.section {
		Some(reference) => Some(resolve::section(&space, reference)?.to_string()),
		None => None,
	};
	let wanted = args.state.wanted();

	let rows: Vec<_> = space
		.notes
		.iter()
		.filter(|note| section.as_deref().is_none_or(|id| note.section == id))
		.filter(|note| wanted.is_none_or(|done| note.done == done))
		.take(args.limit.unwrap_or(usize::MAX))
		.map(|note| note_row(&space, note))
		.collect();

	Ok(Report::Notes(rows))
}

fn add(store: &mut Store, args: &NoteAddArgs) -> Result<Report> {
	let text = body(&args.body, args.stdin)?;
	let section = match &args.section {
		Some(reference) => {
			let space = store.active_space()?;
			Some(resolve::section(&space, reference)?.to_string())
		}
		None => None,
	};
	let at = if args.top {
		InsertionPoint::Top
	} else {
		InsertionPoint::Bottom
	};

	let (id, _) = store.mutate(|space| ops::add_note(space, &text, section.as_deref(), &[], at))?;
	Ok(Report::Id(id))
}

/// `mutate_no_snapshot`, matching what the app does for a text edit.
///
/// The app's reason is that the composer has the browser's own undo; the CLI's is
/// that its undo stack dies with the process anyway. What the choice actually
/// buys here is on the *app's* side: a CLI edit that pushed a snapshot would
/// still clear the running app's history through the watcher, and pushing one
/// into a stack nobody can reach is pure cost.
fn edit(store: &mut Store, args: &NoteEditArgs) -> Result<Report> {
	let text = body(&args.body, args.stdin)?;
	let space = store.active_space()?;
	let id = resolve::note_id(&space, &args.id)?.to_string();
	store.mutate_no_snapshot(|space| ops::edit_note(space, &id, &text))?;
	Ok(Report::Id(id))
}

fn delete(store: &mut Store, references: &[String]) -> Result<Report> {
	let ids = resolved(store, references)?;
	store.mutate(|space| ops::delete_notes(space, &ids))?;
	Ok(Report::Ids(ids))
}

fn move_to(store: &mut Store, references: &[String], reference: &str) -> Result<Report> {
	let space = store.active_space()?;
	let section = resolve::section(&space, reference)?.to_string();
	let ids = resolve::note_ids(&space, references)?;
	store.mutate(|space| ops::move_notes(space, &ids, &section))?;
	Ok(Report::Ids(ids))
}

fn set_done(store: &mut Store, references: &[String], done: bool) -> Result<Report> {
	let ids = resolved(store, references)?;
	store.mutate(|space| ops::set_notes_done(space, &ids, done))?;
	Ok(Report::Ids(ids))
}

fn merge(store: &mut Store, references: &[String]) -> Result<Report> {
	let ids = resolved(store, references)?;
	store.mutate(|space| ops::merge_notes(space, &ids))?;
	Ok(Report::Ids(ids))
}

/// Every reference resolved against the document as it is now.
///
/// Resolution happens once, outside the closure, for the reason
/// `section::rename` gives: a prefix re-resolved against a re-read document
/// during the conflict retry could pick a different note than the first attempt
/// did.
fn resolved(store: &Store, references: &[String]) -> Result<Vec<String>> {
	let space = store.active_space()?;
	resolve::note_ids(&space, references)
}
