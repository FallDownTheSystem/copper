//! `copper search` — a thin wrapper over `copper_core::search::search_notes`.
//!
//! Thin on purpose. The matching, the filters and the ordering all belong to the
//! core so the app and the CLI cannot disagree about what a query means; the only
//! thing this file decides is what a result row looks like.

use copper_core::search::search_notes;
use copper_core::store::error::Result;
use copper_core::store::Store;

use crate::cli::SearchArgs;
use crate::output::{Report, SearchRow};
use crate::resolve;

pub fn run(store: &Store, args: &SearchArgs) -> Result<Report> {
	let space = store.active_space()?;
	let section = resolve::optional_section(&space, args.section.as_deref())?;

	let results = search_notes(
		&space,
		&args.query,
		section.as_deref(),
		args.state.wanted(),
		args.exact,
	)
	.into_iter()
	.take(args.limit.unwrap_or(usize::MAX))
	.map(|note| SearchRow {
		id: note.id.clone(),
		section: note.section.clone(),
		body: note.body.clone(),
	})
	.collect();

	Ok(Report::Search {
		query: args.query.clone(),
		exact: args.exact,
		results,
	})
}
