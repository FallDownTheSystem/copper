//! The clipboard payloads, as Rust.
//!
//! A port of the former `src/lib/noteMarkdown.ts`, function for function, and
//! byte-equal to it on that file's whole test corpus (`tests/markdown.rs`). It
//! lives here rather than in either front end because two implementations of one
//! clipboard format is two formats, and the only way "the same notes produce the
//! same text" stays true is for there to be one renderer.
//!
//! **Both front ends now reach it.** The CLI calls these functions directly;
//! the app calls them through `render_notes_markdown` (task-024), which is also
//! what deleted the TypeScript original.
//!
//! Three renderers, not one, and the split is the TypeScript file's:
//! [`copy_markdown`] and [`list_markdown`] are body-only and each has a recorded
//! contract the section-aware renderer would break; [`section_markdown`] is the
//! one shared by every "copy as Markdown" scope. Which notes reach them is the
//! caller's question — this module only formats what it is handed, which is what
//! makes the scopes byte-identical for the same input by construction.
//!
//! **Attachments are omitted from all three.** A note's `file` is a
//! content-addressed name inside a sidecar directory beside the `.copper`, so a
//! link to it means nothing on another machine, and `name` is the user's original
//! filename and not unique. Naming them in prose would put text into the document
//! that was never in a note.

/// `Copy`: the raw Markdown bodies, joined by a blank line.
///
/// Byte-for-byte the join `ops::merge_notes` performs, which is why there is no
/// second body-merging helper beside it.
pub fn copy_markdown(bodies: &[&str]) -> String {
	bodies.join("\n\n")
}

/// `Copy as List`: a flat plain bulleted list, for pasting into a prompt with no
/// cleanup.
///
/// Never `[ ]`/`[x]` checkbox syntax and never headings, whatever the notes' own
/// done state or grouping. That contract is why [`section_markdown`] is a
/// separate function rather than this one growing a mode.
pub fn list_markdown(bodies: &[&str]) -> String {
	bodies
		.iter()
		.map(|body| item(body, "- "))
		.collect::<Vec<_>>()
		.join("\n")
}

/// A section and the notes of it that are in scope, in document order.
///
/// `notes` is an owned `Vec` of `(done, body)` rather than a borrowed slice of a
/// note type: the renderer needs exactly two fields, and taking `model::Note`
/// would tie the clipboard format to the on-disk one — so a field added to the
/// document would change this signature for no reason.
pub struct MarkdownSection<'a> {
	pub name: &'a str,
	pub notes: Vec<(bool, &'a str)>,
}

/// Sections as ATX headings, notes as task-list items carrying their done state.
///
/// A body's *characters* are embedded verbatim — it is already Markdown and
/// escaping it would corrupt every fence and table in the space — but where they
/// are placed inside the item is not free: see [`item`].
///
/// A section holding nothing renders as its heading alone, which is what keeps an
/// empty section visible in a document-wide copy.
pub fn section_markdown(sections: &[MarkdownSection<'_>]) -> String {
	sections
		.iter()
		.map(|section| {
			let mut lines = vec![format!("# {}", section.name)];
			lines.extend(
				section
					.notes
					.iter()
					.map(|&(done, body)| item(body, marker(done))),
			);
			lines.join("\n")
		})
		.collect::<Vec<_>>()
		.join("\n\n")
}

fn marker(done: bool) -> &'static str {
	if done {
		"- [x] "
	} else {
		"- [ ] "
	}
}

/// One note as a Markdown list item.
///
/// The compact form puts the first line after the marker and indents every
/// continuation line two spaces so it stays inside the item, leaving a blank line
/// genuinely blank rather than two spaces of trailing whitespace.
///
/// **A body that opens a block construct does not fit that form**, because
/// Markdown's block syntax is line-anchored: pushed behind `- [ ] ` a fence, a
/// heading, a blockquote, a nested list or an indented code block stops being one
/// and becomes paragraph text. Measured with the project's own markdown-it, the
/// compact form turns a ```` ```js ```` into an empty `<pre>` with the code
/// leaked out beside it, and — worse, because it consumes the item itself — turns
/// a body whose second line is `===` into `<h1>[ ] Title</h1>`.
///
/// So such a body goes on continuation lines under a bare marker, separated by a
/// blank line. The blank line is not decoration: without it the marker's own
/// `[ ]` joins the first body line into one paragraph, and the two *retroactive*
/// constructs — a setext underline and a table delimiter row — then swallow the
/// checkbox along with it.
fn item(body: &str, marker: &str) -> String {
	let lines = split_lines(body);
	let first = lines.first().copied().unwrap_or("");
	let indented: Vec<String> = lines
		.iter()
		.map(|line| {
			if line.is_empty() {
				String::new()
			} else {
				format!("  {line}")
			}
		})
		.collect();

	if opens_a_block(first) || redefines_the_line_above(lines.get(1).copied()) {
		let mut out = vec![marker.trim_end().to_string(), String::new()];
		out.extend(indented);
		out.join("\n")
	} else {
		let mut out = vec![format!("{marker}{first}")];
		out.extend(indented.into_iter().skip(1));
		out.join("\n")
	}
}

/// The TypeScript's `body.split(/\r?\n/)`, which is **not** `str::lines()`.
///
/// The two disagree on a trailing newline: `"a\n"` is `["a", ""]` to the regex
/// and `["a"]` to `lines()`, so a body ending in a blank line would lose it here
/// and the Rust renderer would stop being byte-equal to the one the app ships.
/// They also disagree on the empty string — `[""]` against no elements at all.
///
/// Splitting on `\n` and stripping the `\r` that *preceded* one is the exact
/// equivalent. The `\r` matters on its own account: a body pasted from a Windows
/// app carries CRLF, and a stray carriage return at the end of every line is
/// invisible on the clipboard and wrong everywhere it lands.
///
/// Only segments that were followed by a `\n` lose their `\r`, which is what the
/// regex says and is not the same as stripping every trailing one. A body that
/// ends in a bare carriage return keeps it — nothing was a line break there, so
/// removing it would be editing the note rather than formatting it.
fn split_lines(body: &str) -> Vec<&str> {
	let mut lines: Vec<&str> = body.split('\n').collect();
	// `split` always yields at least one segment, so there is always a last one,
	// and it is the only one no `\n` followed.
	let last = lines.len() - 1;
	for line in lines.iter_mut().take(last) {
		*line = line.strip_suffix('\r').unwrap_or(line);
	}
	lines
}

/// Constructs that have to begin a line to mean anything.
///
/// Over-triggering costs three characters of compactness; under-triggering
/// silently corrupts a note. Written to err the first way, exactly as the
/// TypeScript is.
///
/// Hand-rolled prefix scans rather than a `regex` dependency: every one of these
/// is a fixed-width scan over ASCII punctuation, and `cargo tree -p copper-cli`
/// is an acceptance criterion.
fn opens_a_block(line: &str) -> bool {
	let indent = line.bytes().take_while(|&byte| byte == b' ').count();

	// Indented code, `^ {4,}\S` — the one opener whose prefix is four spaces or
	// more, which is also why it can return outright: at that indent every check
	// below has already failed its own `^ {0,3}`.
	if indent >= 4 {
		return line[indent..]
			.chars()
			.next()
			.is_some_and(|ch| !crate::js::is_whitespace(ch));
	}

	let rest = &line[indent..];
	rest.starts_with("```")            // fenced code
		|| rest.starts_with("~~~")       // fenced code
		|| is_atx_heading(rest)          // # heading
		|| rest.starts_with('>')         // blockquote
		|| is_list_item(rest)            // - + * or 1. 1)
		|| is_thematic_break(rest)       // --- *** ___
		|| rest.starts_with('<') // HTML block
}

/// `#{1,6}(?:\s|$)`.
///
/// Seven hashes is not a heading, and neither is `###x`: the regex can give back
/// hashes on backtracking but every shorter match then meets another `#`, which
/// is neither whitespace nor the end.
fn is_atx_heading(rest: &str) -> bool {
	let hashes = rest.bytes().take_while(|&byte| byte == b'#').count();
	if hashes == 0 || hashes > 6 {
		return false;
	}
	followed_by_space_or_end(rest, hashes)
}

/// `(?:[-+*]|\d{1,9}[.)])(?:\s|$)`.
fn is_list_item(rest: &str) -> bool {
	let bytes = rest.as_bytes();
	let after = match bytes.first() {
		Some(b'-' | b'+' | b'*') => 1,
		Some(byte) if byte.is_ascii_digit() => {
			let digits = bytes.iter().take_while(|byte| byte.is_ascii_digit()).count();
			// Ten digits is not a list marker at any shorter match either: giving one
			// back leaves the regex looking at a digit where it needs `.` or `)`.
			if digits > 9 {
				return false;
			}
			match bytes.get(digits) {
				Some(b'.' | b')') => digits + 1,
				_ => return false,
			}
		}
		_ => return false,
	};
	followed_by_space_or_end(rest, after)
}

/// The `(?:\s|$)` both markers end with.
fn followed_by_space_or_end(rest: &str, at: usize) -> bool {
	rest[at..]
		.chars()
		.next()
		.is_none_or(crate::js::is_whitespace)
}

/// `(?:[-*_][ \t]*){3,}$`.
///
/// Three or more markers, each optionally followed by spaces or tabs, filling the
/// rest of the line. The markers need not be the same character — `- * _` counts
/// — which is looser than CommonMark and is what the TypeScript does.
fn is_thematic_break(rest: &str) -> bool {
	let mut markers = 0;
	let mut remainder = rest;
	loop {
		let mut chars = remainder.chars();
		match chars.next() {
			Some('-' | '*' | '_') => {
				markers += 1;
				let after = chars.as_str();
				let spaces = after
					.bytes()
					.take_while(|&byte| byte == b' ' || byte == b'\t')
					.count();
				remainder = &after[spaces..];
			}
			Some(_) => return false,
			None => return markers >= 3,
		}
	}
}

/// A line that changes what the line *above* it means: a setext underline turns a
/// paragraph into a heading, and a GFM delimiter row turns it into a table
/// header.
///
/// These are why the first line alone is not enough to decide. A table's own
/// opener — a leading `|` — needs no case here, because every GFM table has a
/// delimiter row on its second line and that is what this catches.
fn redefines_the_line_above(line: Option<&str>) -> bool {
	let Some(line) = line else { return false };
	if is_setext_underline(line) {
		return true;
	}
	// `^[\s|:-]+$` plus both of the two characters that make it a delimiter row
	// rather than, say, a line of spaces.
	!line.is_empty()
		&& line
			.chars()
			.all(|ch| crate::js::is_whitespace(ch) || matches!(ch, '|' | ':' | '-'))
		&& line.contains('|')
		&& line.contains('-')
}

/// `^ {0,3}(?:=+|-+)[ \t]*$`.
fn is_setext_underline(line: &str) -> bool {
	let indent = line.bytes().take_while(|&byte| byte == b' ').count();
	if indent > 3 {
		return false;
	}
	let rest = &line[indent..];
	let marker = match rest.as_bytes().first() {
		Some(&byte @ (b'=' | b'-')) => byte,
		_ => return false,
	};
	let run = rest.bytes().take_while(|&byte| byte == marker).count();
	rest[run..]
		.bytes()
		.all(|byte| byte == b' ' || byte == b'\t')
}
