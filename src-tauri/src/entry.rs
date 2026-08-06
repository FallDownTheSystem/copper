//! What a submitted string *means*, decided above the store.
//!
//! Task-003 §1 makes an invariant of it: `body` is opaque Markdown and **the
//! store never parses it**. That invariant is why `merge_notes`, `edit_note` and
//! the `$EDITOR` write-back are all safe, and putting this rule inside
//! `ops::add_note` would break it for every caller at once. So the
//! classification lives here, in a module the store's write pipeline does not
//! reach, and only `submit_entry` consults it.
//!
//! Pure: no store, no `AppHandle`, no IO. Which is what makes the whole
//! recognition table below a table-driven test with no fixtures.
//!
//! **The rule is deliberately narrow.** A captured selection very often begins
//! with a Markdown heading, and treating "first line is a heading" as a
//! directive would silently swallow the first line of a large fraction of real
//! notes and create junk sections named after them. So a body is a directive
//! only when, after trimming, the **entire** body is a single `# Name` line.
//!
//! The capture path does not come here at all (Open Question 1, answered
//! 2026-08-05): a captured selection whose body is exactly `# Name` is saved as
//! an ordinary note. Inline section creation exists only in the composer.

use std::borrow::Cow;

/// Open Question 3, answered 2026-08-05: 80 characters after normalisation,
/// truncated rather than rejected.
pub const SECTION_NAME_MAX: usize = 80;

/// What the composer submitted, once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry<'a> {
	Section {
		name: String,
	},
	/// `Cow` because only the `\#` escape rewrites the body; every other note
	/// crosses this module untouched.
	Note {
		body: Cow<'a, str>,
	},
}

/// Collapses internal whitespace runs, trims, and caps the length.
///
/// Public because `ops::add_section_and_activate` resolves duplicate names
/// against the *same* normalisation — a second copy of this rule in the store is
/// how the directive and the switcher would drift into disagreeing about which
/// names collide. It normalises a section *name*; it does not look at a body, so
/// the store's opacity invariant is untouched.
pub fn normalise_name(name: &str) -> String {
	let collapsed = name.split_whitespace().collect::<Vec<_>>().join(" ");
	if collapsed.chars().count() <= SECTION_NAME_MAX {
		return collapsed;
	}
	// Counted in `char`s, not bytes: a byte slice would panic on a multi-byte
	// boundary, and the cap is a legibility limit rather than a storage one.
	// Trimmed again because the cut can land immediately after a space.
	collapsed
		.chars()
		.take(SECTION_NAME_MAX)
		.collect::<String>()
		.trim_end()
		.to_string()
}

/// The whole rule, in one function.
pub fn classify(body: &str) -> Entry<'_> {
	// The documented escape hatch, and the only body rewriting this module
	// performs. Tested against the raw body rather than the trimmed one: it is a
	// literal "I meant this character" prefix, so it has to be the first thing the
	// user typed.
	if let Some(rest) = body.strip_prefix("\\#") {
		return Entry::Note {
			body: Cow::Owned(format!("#{rest}")),
		};
	}

	match section_name(body) {
		Some(name) => Entry::Section { name },
		None => Entry::Note {
			body: Cow::Borrowed(body),
		},
	}
}

/// `^#[ \t]+\S.*$` over the **whole** trimmed body, hand-written because this
/// crate takes no regex dependency.
fn section_name(body: &str) -> Option<String> {
	// `str::trim` is Unicode whitespace, so this also absorbs the CRLF a paste can
	// leave on the end.
	let rest = body.trim().strip_prefix('#')?;
	// The space is required, which is Markdown's own ATX rule and what gives
	// `#hashtag` a safe path. It also rejects `##`+, since the second `#` is not a
	// space — that is how a user writes a literal heading note.
	if !rest.starts_with([' ', '\t']) {
		return None;
	}

	let name = rest.trim_start_matches([' ', '\t']);
	// Multi-line bodies are never directives. `\r` counts as a line ending, so a
	// CRLF document's `# Name\r\n\r\nbody` is caught here rather than becoming a
	// section named `Name\r\r\nbody`.
	if name.contains(['\n', '\r']) {
		return None;
	}

	let name = normalise_name(name);
	// Unreachable through the checks above, which already require a non-whitespace
	// character. Kept because the emptiness rule is the caller's contract, not a
	// consequence of how the match happens to be spelled.
	(!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn note_body(body: &str) -> String {
		match classify(body) {
			Entry::Note { body } => body.into_owned(),
			Entry::Section { name } => panic!("{body:?} was classified as the section {name:?}"),
		}
	}

	fn section_of(body: &str) -> String {
		match classify(body) {
			Entry::Section { name } => name,
			Entry::Note { .. } => panic!("{body:?} was classified as a note"),
		}
	}

	/// Every row of the Specification's consequence list, plus the cases that
	/// only appear once real text is pasted in.
	#[test]
	fn the_recognition_table_holds() {
		let sections = [
			// The feature itself.
			("# Research", "Research"),
			// Leading and trailing whitespace, including the blank lines a paste
			// leaves behind, are trimmed before the match.
			("   # Research  ", "Research"),
			("\n\n# Research\n\n", "Research"),
			("# Research\r\n", "Research"),
			("\t# Research\t", "Research"),
			// More than one space after the `#` is still one directive.
			("#    Research", "Research"),
			("#\tResearch", "Research"),
			// Internal whitespace runs collapse; the name is trimmed.
			("# Deep    Research", "Deep Research"),
			("# Deep\tResearch", "Deep Research"),
			// A trailing `#` run is *not* stripped. Closed ATX headings are rare in
			// hand typing and stripping them would surprise more than it helps.
			("# Research #", "Research #"),
			("# Research ###", "Research ###"),
			// Interior `#` characters are ordinary text.
			("# C# notes", "C# notes"),
			// A backslash anywhere but the very front is just a character.
			("# a\\#b", "a\\#b"),
		];
		for (body, name) in sections {
			assert_eq!(section_of(body), name, "{body:?} should be a section directive");
		}

		let notes = [
			// Multi-line bodies are never directives.
			"# Research\n\nSome note text",
			"# Research\nmore",
			"# Research\r\n\r\nSome note text",
			// A body that is `# Name` followed by more heading lines is still a note.
			"# Research\n# Later",
			// Only a single `#` counts.
			"## Research",
			"### Research",
			// The space is required.
			"#Research",
			"#hashtag",
			// Nothing after the `#`.
			"#",
			"#   ",
			"#\t",
			"# ",
			// Not a heading at all.
			"Research",
			"",
			"   ",
			"A note that mentions # Research inside it",
		];
		for body in notes {
			assert_eq!(note_body(body), body, "{body:?} should be an ordinary note");
		}
	}

	/// The escape hatch, and the only body this module rewrites.
	#[test]
	fn a_leading_backslash_escapes_the_directive_and_is_consumed() {
		assert_eq!(note_body("\\# Research"), "# Research");
		// Consumed by position, not by whether what follows would have matched.
		assert_eq!(note_body("\\#Research"), "#Research");
		assert_eq!(note_body("\\# Research\n\nmore"), "# Research\n\nmore");
		// Only at the very front: anywhere else it is an ordinary character, and a
		// body that merely *contains* the sequence keeps it.
		assert_eq!(note_body(" \\# Research"), " \\# Research");
		assert_eq!(note_body("see \\# Research"), "see \\# Research");
		// A doubled backslash escapes nothing extra — one is taken, one stays.
		assert_eq!(note_body("\\\\# Research"), "\\\\# Research");
	}

	/// An ordinary note crosses this module without allocating, which is the
	/// whole reason the variant carries a `Cow`.
	#[test]
	fn an_unescaped_note_body_is_borrowed_rather_than_copied() {
		let body = "an ordinary note";
		match classify(body) {
			Entry::Note { body: Cow::Borrowed(borrowed) } => assert!(std::ptr::eq(borrowed, body)),
			other => panic!("expected a borrowed note, got {other:?}"),
		}
	}

	#[test]
	fn a_name_is_capped_rather_than_rejected() {
		let long = "x".repeat(500);
		let name = section_of(&format!("# {long}"));
		assert_eq!(name.chars().count(), SECTION_NAME_MAX);
		assert_eq!(name, "x".repeat(SECTION_NAME_MAX));

		// The cap counts characters, so a multi-byte name is not cut mid-codepoint.
		let wide = "é".repeat(200);
		let name = section_of(&format!("# {wide}"));
		assert_eq!(name.chars().count(), SECTION_NAME_MAX);

		// A cut that lands on a space leaves no trailing space behind.
		let words = format!("# {}", "ab ".repeat(60));
		let name = section_of(&words);
		assert_eq!(name, name.trim_end());
		assert!(name.chars().count() <= SECTION_NAME_MAX);
	}

	/// Normalisation is idempotent and case-preserving: it decides which names
	/// *collide*, never how a name is displayed.
	#[test]
	fn normalise_name_collapses_without_folding_case() {
		assert_eq!(normalise_name("  Deep   Research \n"), "Deep Research");
		assert_eq!(normalise_name("Deep Research"), "Deep Research");
		assert_eq!(normalise_name(&normalise_name("  a   b  ")), "a b");
		assert_eq!(normalise_name("ReSeArCh"), "ReSeArCh");
		assert_eq!(normalise_name("   "), "");
	}
}
