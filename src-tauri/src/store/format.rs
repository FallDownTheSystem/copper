//! The only module that decides what a `.copper` file looks like.
//!
//! Three responsibilities, kept together because they are one contract:
//! serialisation that is byte-stable and diffs well under git, identity
//! validation at load, and `normalise` — the idempotent repair-and-canonical-
//! order pass that runs after every operation and before every write.
//!
//! The split between `validate_identity` and `normalise` is the important one.
//! `normalise` repairs *ordering and references*, which are always losslessly
//! repairable. It cannot repair *identity*: two sections sharing an id make note
//! ownership ambiguous and two notes sharing an id make every id-addressed
//! operation ambiguous, and picking a winner would silently discard user data.
//! So identity is checked once at load and the document is refused (spec 1.5a),
//! which is recoverable by hand-editing the file — a silent repair is not.

use std::collections::{BTreeMap, HashSet};

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::ser::PrettyFormatter;

use super::error::{Result, StoreError};
use super::ids;
use super::model::{Note, Section, Space, DEFAULT_SECTION_NAME};

/// `2026-07-30T14:02:11Z` — RFC3339, UTC, second precision (spec 1.2).
pub fn now_rfc3339() -> String {
	Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Serialises to the exact bytes that go on disk: two-space indent, struct
/// declaration order, LF, one trailing newline.
///
/// `serde_json::to_string_pretty` already indents with two spaces, but the
/// formatter is constructed explicitly so the on-disk format cannot drift with
/// a dependency default. Neither form appends a trailing newline; we do.
pub fn to_git_json<T: Serialize>(value: &T) -> Result<String> {
	let mut buffer = Vec::new();
	let formatter = PrettyFormatter::with_indent(b"  ");
	let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
	value
		.serialize(&mut serializer)
		.map_err(|err| StoreError::Io(format!("could not serialise document: {err}")))?;
	let mut text = String::from_utf8(buffer)
		.map_err(|err| StoreError::Io(format!("serialised document is not UTF-8: {err}")))?;
	text.push('\n');
	Ok(text)
}

/// Parses a space document and rejects it if its identity is ambiguous.
///
/// Deliberately does **not** call `normalise`: startup must be able to hold the
/// text it read alongside a normalised in-memory document, and spec 7.4 forbids
/// rewriting a file merely because loading it tidied the ordering.
pub fn from_json(text: &str) -> Result<Space> {
	let space: Space = serde_json::from_str(text).map_err(|err| {
		StoreError::Parse(format!(
			"not a valid space document (line {}, column {}): {err}",
			err.line(),
			err.column()
		))
	})?;
	validate_identity(&space)?;
	Ok(space)
}

/// Rejects duplicate section ids and duplicate note ids, naming the offenders.
pub fn validate_identity(space: &Space) -> Result<()> {
	let duplicate_sections = duplicates(space.sections.iter().map(|section| section.id.as_str()));
	if !duplicate_sections.is_empty() {
		return Err(StoreError::Parse(format!(
			"duplicate section ids make note ownership ambiguous: {}",
			duplicate_sections.join(", ")
		)));
	}
	let duplicate_notes = duplicates(space.notes.iter().map(|note| note.id.as_str()));
	if !duplicate_notes.is_empty() {
		return Err(StoreError::Parse(format!(
			"duplicate note ids make every operation ambiguous: {}",
			duplicate_notes.join(", ")
		)));
	}
	Ok(())
}

/// The ids that appear more than once, in first-seen order and each named once.
fn duplicates<'a>(ids: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
	let mut seen = HashSet::new();
	let mut repeated = Vec::new();
	for id in ids {
		if !seen.insert(id) && !repeated.contains(&id) {
			repeated.push(id);
		}
	}
	repeated
}

/// Canonical order and load-time repair, as one idempotent pass.
///
/// **The step order is load-bearing and the steps are not independent.** Step 5
/// rebuilds `notes` from per-section groups, so it silently drops any note whose
/// `section` still names no existing section — which means step 4 must have run,
/// which means step 3 must have given it a valid `activeSection` to reassign
/// *to*, which means step 1 must have guaranteed a section exists at all.
///
/// The failure mode this ordering avoids is nasty because it hides: a pass that
/// drops a note reaches its fixed point immediately, so an idempotence assertion
/// alone still passes. The tests therefore assert the preservation postconditions
/// from spec 1.5 as well — note count and the multiset of ids and bodies are
/// unchanged, every reference resolves, and orders are contiguous from zero.
pub fn normalise(space: &mut Space) {
	// 1. A hand-emptied `sections` array still needs a capture target.
	if space.sections.is_empty() {
		space.sections.push(Section {
			id: ids::unique_id(ids::SECTION, |_| false),
			name: DEFAULT_SECTION_NAME.to_string(),
			order: 0,
		});
	}

	// 2. Sections into canonical order, then renumbered to their index. Ties on
	//    `order` break by id so the result does not depend on input order.
	space
		.sections
		.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
	for (index, section) in space.sections.iter_mut().enumerate() {
		section.order = index as i64;
	}

	// 3. Before step 4, never after: step 4 reassigns notes *to* `activeSection`,
	//    so a dangling one would send them to a section that does not exist and
	//    step 5 would then drop them.
	if !space.has_section(&space.active_section) {
		space.active_section = space.sections[0].id.clone();
	}

	// 4. Repaired, never dropped.
	let known: HashSet<&str> = space
		.sections
		.iter()
		.map(|section| section.id.as_str())
		.collect();
	let reassign: Vec<usize> = space
		.notes
		.iter()
		.enumerate()
		.filter(|(_, note)| !known.contains(note.section.as_str()))
		.map(|(index, _)| index)
		.collect();
	for index in reassign {
		space.notes[index].section = space.active_section.clone();
	}

	// 5. Regroup by section, sort within the group, renumber to the index within
	//    the group. Every note reaches a group because step 4 has run.
	let mut grouped: BTreeMap<usize, Vec<Note>> = BTreeMap::new();
	for note in space.notes.drain(..) {
		let position = space
			.sections
			.iter()
			.position(|section| section.id == note.section)
			.expect("step 4 gave every note an existing section");
		grouped.entry(position).or_default().push(note);
	}
	let mut ordered = Vec::with_capacity(grouped.values().map(Vec::len).sum());
	for (_, mut group) in grouped {
		group.sort_by(|a, b| {
			a.order
				.cmp(&b.order)
				.then_with(|| a.created.cmp(&b.created))
				.then_with(|| a.id.cmp(&b.id))
		});
		for (index, note) in group.iter_mut().enumerate() {
			note.order = index as i64;
		}
		ordered.append(&mut group);
	}
	space.notes = ordered;
}

/// Parse, then normalise. The pairing every caller that is *not* startup wants.
pub fn parse_normalised(text: &str) -> Result<Space> {
	let mut space = from_json(text)?;
	normalise(&mut space);
	Ok(space)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn note(id: &str, section: &str, order: i64, created: &str) -> Note {
		Note {
			id: id.to_string(),
			section: section.to_string(),
			order,
			done: false,
			body: format!("body of {id}"),
			created: created.to_string(),
			updated: created.to_string(),
		}
	}

	fn section(id: &str, name: &str, order: i64) -> Section {
		Section {
			id: id.to_string(),
			name: name.to_string(),
			order,
		}
	}

	fn space() -> Space {
		Space {
			id: "spc_7f3aa001".to_string(),
			name: "development".to_string(),
			active_section: "sec_a1000001".to_string(),
			sections: vec![
				section("sec_a1000001", "Research", 0),
				section("sec_b2000002", "Configuration Formats", 1),
			],
			notes: vec![
				note("nte_01000001", "sec_a1000001", 0, "2026-07-30T14:02:11Z"),
				note("nte_02000002", "sec_b2000002", 0, "2026-07-30T14:05:00Z"),
			],
		}
	}

	/// Every postcondition spec 1.5 states, asserted together. Idempotence alone
	/// cannot catch a `normalise` that drops notes, because a dropping pass
	/// reaches its fixed point immediately.
	fn assert_postconditions(before: &Space, after: &Space) {
		assert_eq!(
			before.notes.len(),
			after.notes.len(),
			"normalise changed the note count"
		);

		let mut before_ids: Vec<&str> = before.notes.iter().map(|n| n.id.as_str()).collect();
		let mut after_ids: Vec<&str> = after.notes.iter().map(|n| n.id.as_str()).collect();
		before_ids.sort_unstable();
		after_ids.sort_unstable();
		assert_eq!(before_ids, after_ids, "normalise changed the set of note ids");

		let mut before_bodies: Vec<&str> = before.notes.iter().map(|n| n.body.as_str()).collect();
		let mut after_bodies: Vec<&str> = after.notes.iter().map(|n| n.body.as_str()).collect();
		before_bodies.sort_unstable();
		after_bodies.sort_unstable();
		assert_eq!(before_bodies, after_bodies, "normalise changed a note body");

		assert!(
			after.has_section(&after.active_section),
			"activeSection does not name an existing section"
		);
		for note in &after.notes {
			assert!(
				after.has_section(&note.section),
				"note {} points at a section that does not exist",
				note.id
			);
		}

		for (index, section) in after.sections.iter().enumerate() {
			assert_eq!(section.order, index as i64, "section order is not contiguous");
		}
		for section in &after.sections {
			let group: Vec<&Note> = after
				.notes
				.iter()
				.filter(|note| note.section == section.id)
				.collect();
			for (index, note) in group.iter().enumerate() {
				assert_eq!(note.order, index as i64, "note order is not contiguous");
			}
		}
	}

	fn normalised(mut input: Space) -> Space {
		let before = input.clone();
		normalise(&mut input);
		assert_postconditions(&before, &input);
		let once = input.clone();
		normalise(&mut input);
		assert_eq!(once, input, "normalise is not idempotent");
		input
	}

	#[test]
	fn serialisation_is_byte_stable() {
		let first = to_git_json(&space()).unwrap();
		let second = to_git_json(&space()).unwrap();
		assert_eq!(first, second);
	}

	#[test]
	fn serialisation_has_no_crlf_and_no_trailing_spaces() {
		let text = to_git_json(&space()).unwrap();
		assert!(!text.contains('\r'), "output contains a carriage return");
		assert!(text.ends_with("}\n"), "output does not end in exactly one newline");
		assert!(!text.ends_with("\n\n"));
		for line in text.lines() {
			assert_eq!(line, line.trim_end(), "line has trailing whitespace: {line:?}");
		}
	}

	#[test]
	fn parse_then_serialise_is_the_identity() {
		let text = to_git_json(&space()).unwrap();
		let parsed = from_json(&text).unwrap();
		assert_eq!(to_git_json(&parsed).unwrap(), text);
	}

	#[test]
	fn keys_are_camel_case_in_declaration_order() {
		let text = to_git_json(&space()).unwrap();
		let keys: Vec<&str> = text
			.lines()
			.take(5)
			.filter_map(|line| line.trim().split('"').nth(1))
			.collect();
		assert_eq!(keys, ["id", "name", "activeSection", "sections"]);
	}

	#[test]
	fn dangling_active_section_and_dangling_note_both_survive() {
		// The case that breaks a naively ordered implementation: reassigning notes
		// before repairing `activeSection` sends them to a nonexistent section, and
		// the regroup step then drops them — idempotently.
		let mut input = space();
		input.active_section = "sec_gone".to_string();
		input.notes[1].section = "sec_also_gone".to_string();

		let result = normalised(input);
		assert_eq!(result.notes.len(), 2);
		assert_eq!(result.active_section, "sec_a1000001");
		assert_eq!(result.notes[1].section, "sec_a1000001");
	}

	#[test]
	fn empty_sections_with_notes_present_keeps_every_note() {
		let mut input = space();
		input.sections.clear();

		let result = normalised(input);
		assert_eq!(result.sections.len(), 1);
		assert_eq!(result.sections[0].name, DEFAULT_SECTION_NAME);
		assert_eq!(result.notes.len(), 2);
		assert!(result.notes.iter().all(|n| n.section == result.sections[0].id));
	}

	#[test]
	fn shuffled_orders_reach_canonical_order() {
		let mut input = space();
		input.sections[0].order = 40;
		input.sections[1].order = 3;
		input.notes[0].order = 99;

		let result = normalised(input);
		assert_eq!(result.sections[0].id, "sec_b2000002");
		assert_eq!(result.notes[0].section, "sec_b2000002");
	}

	#[test]
	fn negative_orders_are_repaired_rather_than_rejected() {
		let text = to_git_json(&space()).unwrap().replace("\"order\": 0", "\"order\": -1");
		let result = normalised(from_json(&text).unwrap());
		assert_eq!(result.notes.len(), 2);
	}

	#[test]
	fn notes_group_by_section_in_section_order() {
		let mut input = space();
		input.notes.push(note("nte_03000003", "sec_a1000001", 5, "2026-07-30T14:09:00Z"));

		let result = normalised(input);
		let sections: Vec<&str> = result.notes.iter().map(|n| n.section.as_str()).collect();
		assert_eq!(
			sections,
			["sec_a1000001", "sec_a1000001", "sec_b2000002"],
			"notes are not grouped in section order"
		);
	}

	#[test]
	fn ties_break_by_created_then_id() {
		let mut input = space();
		input.notes.clear();
		input.notes.push(note("nte_bbbbbbbb", "sec_a1000001", 0, "2026-07-30T14:00:00Z"));
		input.notes.push(note("nte_aaaaaaaa", "sec_a1000001", 0, "2026-07-30T14:00:00Z"));
		input.notes.push(note("nte_cccccccc", "sec_a1000001", 0, "2026-07-30T13:00:00Z"));

		let result = normalised(input);
		let ids: Vec<&str> = result.notes.iter().map(|n| n.id.as_str()).collect();
		assert_eq!(ids, ["nte_cccccccc", "nte_aaaaaaaa", "nte_bbbbbbbb"]);
	}

	#[test]
	fn duplicate_note_ids_are_rejected_and_named() {
		let mut input = space();
		input.notes[1].id = input.notes[0].id.clone();
		let text = to_git_json(&input).unwrap();

		let err = from_json(&text).unwrap_err();
		assert_eq!(err.kind(), "parse");
		assert!(err.message().contains("nte_01000001"), "{}", err.message());
		assert!(err.message().contains("note ids"));
	}

	#[test]
	fn duplicate_section_ids_are_rejected_and_named() {
		let mut input = space();
		input.sections[1].id = input.sections[0].id.clone();
		let text = to_git_json(&input).unwrap();

		let err = from_json(&text).unwrap_err();
		assert_eq!(err.kind(), "parse");
		assert!(err.message().contains("sec_a1000001"), "{}", err.message());
		assert!(err.message().contains("section ids"));
	}

	#[test]
	fn malformed_json_reports_line_and_column() {
		let err = from_json("{ not json").unwrap_err();
		assert_eq!(err.kind(), "parse");
		assert!(err.message().contains("line 1"));
	}

	#[test]
	fn a_malformed_timestamp_does_not_make_the_document_unloadable() {
		let text = to_git_json(&space())
			.unwrap()
			.replace("2026-07-30T14:02:11Z", "yesterday afternoon");
		let parsed = from_json(&text).unwrap();
		assert_eq!(parsed.notes[0].created, "yesterday afternoon");
	}

	/// Spec 1.6 / A9.4. Appending to a non-empty array can only touch the new
	/// note's lines plus the previous last note's closing brace, which
	/// necessarily gains a comma.
	#[test]
	fn appending_a_note_produces_a_minimal_diff() {
		let before_doc = normalised(space());
		let mut after_doc = before_doc.clone();
		// The real operation rather than a hand-built `Note`, so this measures the
		// diff a capture actually produces — including whatever `add_note` and
		// `normalise` do to the rest of the document on the way.
		let added_id =
			crate::store::ops::add_note(&mut after_doc, "a captured note", Some("sec_b2000002"))
				.unwrap();

		let before = to_git_json(&before_doc).unwrap();
		let after = to_git_json(&after_doc).unwrap();
		let before_lines: Vec<String> = before.lines().map(str::to_string).collect();
		let after_lines: Vec<String> = after.lines().map(str::to_string).collect();

		// Reconstructing the expected result is stricter than comparing common
		// prefixes and suffixes, and it does not depend on which of two identical
		// `}` lines a diff algorithm chooses to align.
		let closing = before_lines.len() - 3;
		assert_eq!(before_lines[closing].trim(), "}", "not the last note's closing brace");

		let added = &after_lines[closing + 1..after_lines.len() - 2];
		assert_eq!(added.len(), 9, "the added region is not exactly one note object");
		assert_eq!(added[0].trim(), "{");
		assert_eq!(added[8].trim(), "}");
		assert!(added.iter().any(|line| line.contains(&added_id)));

		let mut expected = before_lines.clone();
		// The one unavoidable change to an existing line, and the reason A9.4 has
		// to permit it: JSON gives the previous last element a comma.
		expected[closing] = format!("{},", expected[closing]);
		expected.splice(closing + 1..closing + 1, added.iter().cloned());

		assert_eq!(expected, after_lines, "something outside the new note changed");
	}

	/// Asserted separately because `[]` expands into a multi-line array, so the
	/// line arithmetic above does not describe it.
	#[test]
	fn appending_to_an_empty_notes_array_touches_only_that_array() {
		let mut before_doc = space();
		before_doc.notes.clear();
		let before_doc = normalised(before_doc);
		let mut after_doc = before_doc.clone();
		crate::store::ops::add_note(&mut after_doc, "the first note", None).unwrap();

		let before = to_git_json(&before_doc).unwrap();
		let after = to_git_json(&after_doc).unwrap();

		assert!(before.contains("\"notes\": []"));
		// Everything above the notes array is untouched.
		let before_head = before.split("\"notes\"").next().unwrap();
		let after_head = after.split("\"notes\"").next().unwrap();
		assert_eq!(before_head, after_head);
	}
}
