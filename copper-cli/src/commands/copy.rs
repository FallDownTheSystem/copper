//! `copper copy` — notes to stdout, and optionally to the clipboard.
//!
//! Two axes that happen to share a word. `--format` chooses what the rendered
//! *content* looks like; the global `--json` chooses whether the whole response
//! is wrapped in an envelope. They compose: `--format json --json` nests the
//! array under `"text"` rather than double-encoding it as a string.
//!
//! The clipboard always receives the **raw** rendering, never the envelope. What
//! the user pastes is what `--format` produced, whatever the CLI printed around
//! it.

use copper_core::markdown::{copy_markdown, list_markdown, section_markdown, MarkdownSection};
use copper_core::search::search_notes;
use copper_core::store::error::{Result, StoreError};
use copper_core::store::model::{Note, Space};
use copper_core::store::Store;
use serde_json::{json, Value};

use crate::cli::{CopyArgs, CopyFormat};
use crate::output::Report;
use crate::resolve;

pub fn run(store: &Store, args: &CopyArgs) -> Result<Report> {
	let space = store.active_space()?;
	let selected = select(&space, args)?;
	let rendered = render(&space, &selected, args.format);

	// **The clipboard is not touched here.** Spec 6 wants the rendering on stdout
	// *first*, so a clipboard that is locked by another process delays nothing and
	// costs the user nothing. Only `main` knows when stdout has been written, so
	// only `main` can order the two — see its `clipboard` step.
	Ok(Report::Copy {
		format: args.format.name(),
		text: rendered,
		clipboard_wanted: args.clipboard,
		clipboard: false,
		clipboard_error: None,
	})
}

/// The four selectors, each resolving to notes in **canonical document order**.
///
/// The ids form is reordered rather than kept as typed. `copper copy c3d4 a1b2`
/// produces the same text as `copper copy a1b2 c3d4`, which is what makes the
/// rendering a function of *which* notes were chosen rather than of the order
/// they were named — the same property the app's three copy scopes have.
fn select<'a>(space: &'a Space, args: &CopyArgs) -> Result<Vec<&'a Note>> {
	if !args.ids.is_empty() {
		let wanted = resolve::note_ids(space, &args.ids)?;
		return Ok(space
			.notes
			.iter()
			.filter(|note| wanted.iter().any(|id| *id == note.id))
			.collect());
	}

	if let Some(reference) = &args.section {
		let id = resolve::section(space, reference)?;
		return Ok(space.notes.iter().filter(|note| note.section == id).collect());
	}

	if args.all {
		return Ok(space.notes.iter().collect());
	}

	if let Some(query) = &args.query {
		return Ok(search_notes(space, query, None, None, args.exact));
	}

	// Unreachable: clap's `selection` group is `required(true).multiple(false)`.
	// Stated as an error rather than an `unreachable!` because `panic = "abort"`
	// is on in release and a wrong answer is better delivered as exit code 2.
	Err(StoreError::Invalid(
		"copy needs exactly one of <ID…>, --section, --all or --query".into(),
	))
}

fn render(space: &Space, selected: &[&Note], format: CopyFormat) -> Value {
	match format {
		CopyFormat::Bodies => {
			let bodies: Vec<&str> = selected.iter().map(|note| note.body.as_str()).collect();
			Value::String(copy_markdown(&bodies))
		}
		CopyFormat::List => {
			let bodies: Vec<&str> = selected.iter().map(|note| note.body.as_str()).collect();
			Value::String(list_markdown(&bodies))
		}
		CopyFormat::Markdown => Value::String(section_markdown(&grouped(space, selected))),
		CopyFormat::Json => Value::Array(
			selected
				.iter()
				.map(|note| json!({ "id": note.id, "done": note.done, "body": note.body }))
				.collect(),
		),
	}
}

/// The selected notes under their own sections, in canonical section order.
///
/// A section with none of the selection in it is left out entirely. That differs
/// from the renderer's own rule — it will happily print a heading with nothing
/// under it — and the difference is the caller's to make: an empty heading is
/// meaningful in a whole-document copy, where it says "this section exists and is
/// empty", and is noise in a copy of three notes.
fn grouped<'a>(space: &'a Space, selected: &[&'a Note]) -> Vec<MarkdownSection<'a>> {
	space
		.sections
		.iter()
		.filter_map(|section| {
			let notes: Vec<(bool, &str)> = selected
				.iter()
				.filter(|note| note.section == section.id)
				.map(|note| (note.done, note.body.as_str()))
				.collect();
			(!notes.is_empty()).then(|| MarkdownSection {
				name: section.name.as_str(),
				notes,
			})
		})
		.collect()
}

/// One `set_text` and out.
///
/// `src-tauri`'s clipboard modules are deliberately not reused. They open a
/// message-only window to own the clipboard across `EmptyClipboard` races from a
/// long-lived process, and write several synchronised formats for rich paste
/// targets. A CLI process places one plain-text payload and exits before anything
/// else can contend for it.
///
/// `pub` because `main` owns the ordering against stdout.
pub fn place_on_clipboard(text: &str) -> Result<()> {
	let mut clipboard = arboard::Clipboard::new()
		.map_err(|err| StoreError::Io(format!("could not open the clipboard: {err}")))?;
	clipboard
		.set_text(text.to_string())
		.map_err(|err| StoreError::Io(format!("could not write to the clipboard: {err}")))
}
