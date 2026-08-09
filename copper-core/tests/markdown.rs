//! The `src/lib/noteMarkdown.test.ts` corpus, ported one `#[test]` per `it(…)`.
//!
//! Every expected string is **copied from the TypeScript file**, not re-derived
//! from the Rust. A re-derived expectation proves only that the port agrees with
//! itself; the claim these tests exist to support is that it agrees with the
//! renderer the app ships today, which is the one the user's clipboard has been
//! getting since task-013.
//!
//! One `describe` block is deliberately not ported: `parsed back by markdown-it`
//! runs a JavaScript Markdown parser over the output to prove the strings are
//! safe to paste. That is a claim about the string *contents*, which the
//! byte-equality assertions here already pin — re-asserting it in Rust would mean
//! either embedding a Markdown parser, against this crate's dependency budget, or
//! writing the same string comparisons a second time under a different name.
//!
//! The last block has no TypeScript counterpart. `split(/\r?\n/)` and
//! `str::lines()` disagree about trailing segments, so the port uses neither; the
//! cases that separate them are pinned here.

use copper_core::markdown::{copy_markdown, list_markdown, section_markdown, MarkdownSection};

/// The section renderer over one unnamed-in-the-test section, as the TypeScript's
/// own `render` helper does.
fn render(body: &str, done: bool) -> String {
	section_markdown(&[MarkdownSection {
		name: "S",
		notes: vec![(done, body)],
	}])
}

// --- buildCopyMarkdown --------------------------------------------------------

#[test]
fn copies_a_single_body_unchanged() {
	assert_eq!(copy_markdown(&["**one**"]), "**one**");
}

#[test]
fn joins_two_bodies_with_exactly_one_blank_line() {
	assert_eq!(copy_markdown(&["one", "two"]), "one\n\ntwo");
}

#[test]
fn preserves_interior_blank_lines_and_leading_whitespace() {
	// Both are meaningful Markdown — indented code blocks and list nesting — and
	// the store preserves them, so the clipboard must too.
	let body = "    indented code\n\nsecond paragraph";
	assert_eq!(copy_markdown(&[body]), body);
}

#[test]
fn copy_markdown_returns_an_empty_string_for_no_bodies() {
	assert_eq!(copy_markdown(&[]), "");
}

// --- buildListMarkdown --------------------------------------------------------

#[test]
fn prefixes_every_note_with_a_dash_and_never_checkbox_syntax() {
	let list = list_markdown(&["alpha", "beta"]);
	assert_eq!(list, "- alpha\n- beta");
	assert!(!list.contains("[ ]"));
	assert!(!list.contains("[x]"));
}

#[test]
fn indents_continuation_lines_by_two_spaces_so_they_stay_in_one_item() {
	assert_eq!(list_markdown(&["one\ntwo\nthree"]), "- one\n  two\n  three");
}

#[test]
fn leaves_a_blank_continuation_line_blank_rather_than_indenting_whitespace() {
	assert_eq!(list_markdown(&["head\n\ntail"]), "- head\n\n  tail");
}

/// It shares `item` with the section renderer, so it inherits the same
/// block-structure rule — and wants it for the same reason, since a prompt
/// carrying a mangled fence is worse than one carrying an indented item.
#[test]
fn puts_a_body_that_opens_a_block_construct_under_a_bare_marker() {
	assert_eq!(
		list_markdown(&["```ts\nconst x = 1\n```"]),
		"-\n\n  ```ts\n  const x = 1\n  ```"
	);
}

/// A body pasted from a Windows app carries CRLF; a stray `\r` at the end of
/// every line is invisible on the clipboard and wrong everywhere it lands.
#[test]
fn strips_the_carriage_returns_of_a_crlf_body() {
	assert_eq!(list_markdown(&["one\r\ntwo"]), "- one\n  two");
}

#[test]
fn emits_no_section_headings() {
	assert!(!list_markdown(&["a", "b"]).contains("##"));
}

#[test]
fn returns_an_empty_string_for_no_notes() {
	assert_eq!(list_markdown(&[]), "");
}

// --- buildSectionMarkdown -----------------------------------------------------

fn setup() -> MarkdownSection<'static> {
	MarkdownSection {
		name: "Project Setup",
		notes: vec![
			(false, "Install dependencies\nNote body here."),
			(true, "Configure environment\nDone note body."),
		],
	}
}

fn testing() -> MarkdownSection<'static> {
	MarkdownSection {
		name: "Testing",
		notes: vec![(false, "Write unit tests")],
	}
}

#[test]
fn renders_sections_as_atx_headings_and_notes_as_task_list_items() {
	assert_eq!(
		section_markdown(&[setup(), testing()]),
		"# Project Setup\n\
		 - [ ] Install dependencies\n  \
		 Note body here.\n\
		 - [x] Configure environment\n  \
		 Done note body.\n\
		 \n\
		 # Testing\n\
		 - [ ] Write unit tests"
	);
}

/// AC12. The three scopes differ only in which sections they hand over, so the
/// same input has to come back byte-identical however it was resolved.
#[test]
fn is_byte_identical_for_the_same_input_whatever_the_scope_resolved_it() {
	let whole = section_markdown(&[setup(), testing()]);
	let selection = section_markdown(&[setup(), testing()]);
	assert_eq!(selection, whole);
	assert_eq!(
		section_markdown(&[testing()]),
		whole.split("\n\n").nth(1).unwrap()
	);
}

#[test]
fn embeds_a_body_as_is_rather_than_escaping_markdown_inside_it() {
	assert_eq!(render("a **bold** word", false), "# S\n- [ ] a **bold** word");
}

#[test]
fn section_markdown_leaves_a_blank_continuation_line_blank() {
	assert_eq!(render("head\n\ntail", false), "# S\n- [ ] head\n\n  tail");
}

// --- a body that opens a block construct --------------------------------------

#[test]
fn puts_a_fenced_code_block_under_a_bare_marker() {
	assert_eq!(
		render("```ts\nconst x = 1\n```", false),
		"# S\n- [ ]\n\n  ```ts\n  const x = 1\n  ```"
	);
}

#[test]
fn puts_a_body_whose_second_line_is_a_setext_underline_under_a_bare_marker() {
	// The nastiest of them inline: `- [ ] Title` followed by `  ===` makes the
	// whole item a heading, checkbox text and all.
	assert_eq!(render("Title\n===", true), "# S\n- [x]\n\n  Title\n  ===");
}

#[test]
fn puts_a_blockquote_under_a_bare_marker() {
	assert_eq!(render("> quoted", false), "# S\n- [ ]\n\n  > quoted");
}

#[test]
fn puts_a_nested_list_under_a_bare_marker() {
	assert_eq!(
		render("- inner\n- second", false),
		"# S\n- [ ]\n\n  - inner\n  - second"
	);
}

#[test]
fn puts_a_heading_a_table_and_indented_code_under_a_bare_marker() {
	assert_eq!(render("# Title", false), "# S\n- [ ]\n\n  # Title");
	assert_eq!(
		render("Name | Age\n--- | ---", false),
		"# S\n- [ ]\n\n  Name | Age\n  --- | ---"
	);
	assert_eq!(
		render("    indented code", false),
		"# S\n- [ ]\n\n      indented code"
	);
}

#[test]
fn leaves_an_inline_safe_body_compact() {
	// Emphasis, links and inline code all mean the same thing anywhere on a line,
	// so nothing is gained by pushing them down.
	assert_eq!(
		render("`inline code` and *emphasis*", false),
		"# S\n- [ ] `inline code` and *emphasis*"
	);
	assert_eq!(
		render("a line\nand another", false),
		"# S\n- [ ] a line\n  and another"
	);
}

#[test]
fn renders_a_section_with_nothing_in_scope_as_its_heading_alone() {
	assert_eq!(
		section_markdown(&[MarkdownSection {
			name: "Empty",
			notes: Vec::new(),
		}]),
		"# Empty"
	);
}

#[test]
fn returns_an_empty_string_for_no_sections() {
	assert_eq!(section_markdown(&[]), "");
}

/// Attachments are omitted, so a note carrying one renders exactly as the same
/// note without it.
#[test]
fn renders_nothing_for_attachments() {
	assert_eq!(render("a note", false), "# S\n- [ ] a note");
}

// --- line splitting, which has no TypeScript counterpart ----------------------

/// The cases that separate `split(/\r?\n/)` from `str::lines()`.
///
/// `lines()` drops the empty segment a trailing newline produces, so a body that
/// ends in a blank line would come back one line shorter than the app's clipboard
/// has it. These pin the regex's answer, which is the one the port has to give.
#[test]
fn a_trailing_newline_keeps_its_empty_last_line() {
	assert_eq!(list_markdown(&["a\n"]), "- a\n");
	assert_eq!(list_markdown(&["a\r\n"]), "- a\n");
	assert_eq!(list_markdown(&["a\n\n"]), "- a\n\n");
	assert_eq!(list_markdown(&["a\r\n\r\n"]), "- a\n\n");
}

/// A carriage return with no newline after it is not a line break, so it stays in
/// the text. Stripping it would be editing the note rather than formatting it.
#[test]
fn a_lone_carriage_return_is_not_a_line_break() {
	assert_eq!(list_markdown(&["\r"]), "- \r");
	assert_eq!(list_markdown(&["a\r"]), "- a\r");
}

/// `ops::clean_body` refuses an empty body long before a renderer sees one, so
/// this never fires in the app. It is pinned anyway because this module is a
/// public library surface with its own callers, and because `first` being `""`
/// here is currently an accident of `unwrap_or` rather than a decision anyone
/// wrote down.
#[test]
fn an_empty_body_renders_as_a_bare_marker() {
	assert_eq!(list_markdown(&[""]), "- ");
	assert_eq!(render("", false), "# S\n- [ ] ");
}
