//! Every structural change a space can undergo, as plain functions over
//! `&mut Space`.
//!
//! Two rules hold across all of them and are what make the write pipeline
//! simple. **Validate completely before mutating** (spec 2.5) — a multi-id call
//! with one bad id changes nothing and never reaches disk. And **every op ends
//! with `normalise`**, so no caller can forget and no operation has to think
//! about `order` bookkeeping beyond placing a note in the right slot.
//!
//! Because they are `Fn`-shaped closures over `&mut Space` and nothing else,
//! re-applying one against a freshly re-read document costs nothing — which is
//! the entire mechanism of the write-conflict path in `mutate`.
//!
//! `updated` tracks note *content*, never note *position* (spec 5.6). A drag
//! that restamped every affected note would turn moving one note into a diff
//! across the whole section, for no information gained.

use std::collections::HashSet;

use crate::entry::normalise_name;

use super::error::{Result, StoreError};
use super::format::{normalise, now_rfc3339};
use super::ids;
use super::model::{Note, Section, Space};

// --- shared validation -------------------------------------------------------

/// Trailing whitespace goes; everything else is preserved verbatim (spec 5.7).
/// Leading whitespace and interior blank lines are meaningful Markdown —
/// indented code blocks, list nesting — so trimming both ends would corrupt
/// notes.
fn clean_body(body: &str) -> Result<String> {
	let trimmed = body.trim_end();
	if trimmed.is_empty() {
		// Never a silent delete: clearing a textarea must not be able to destroy a
		// note by accident (spec 5.1).
		return Err(StoreError::Invalid("a note cannot be empty".into()));
	}
	Ok(trimmed.to_string())
}

fn clean_name(name: &str) -> Result<String> {
	let trimmed = name.trim();
	if trimmed.is_empty() {
		return Err(StoreError::Invalid("a section needs a name".into()));
	}
	Ok(trimmed.to_string())
}

/// First occurrence wins, and an empty result is an error rather than a silent
/// no-op (spec 5.5) — the frontend has no legitimate empty-selection call, and
/// returning an unchanged document from an apparently successful mutation would
/// mislead the caller into thinking something happened.
fn dedupe_ids(ids: &[String]) -> Result<Vec<String>> {
	let mut seen = HashSet::new();
	let mut unique = Vec::with_capacity(ids.len());
	for id in ids {
		if seen.insert(id.as_str()) {
			unique.push(id.clone());
		}
	}
	if unique.is_empty() {
		return Err(StoreError::Invalid("no notes were selected".into()));
	}
	Ok(unique)
}

/// The one spelling of each. Four call sites raise the section message and two
/// raise the note one, and they have to stay the same sentence.
fn no_such_note(id: &str) -> StoreError {
	StoreError::NotFound(format!("no such note: {id}"))
}

fn no_such_section(id: &str) -> StoreError {
	StoreError::NotFound(format!("no such section: {id}"))
}

fn require_notes(space: &Space, ids: &[String]) -> Result<()> {
	let missing: Vec<&str> = ids
		.iter()
		.filter(|id| space.note(id).is_none())
		.map(String::as_str)
		.collect();
	if missing.is_empty() {
		Ok(())
	} else {
		Err(StoreError::NotFound(format!(
			"no such note: {}",
			missing.join(", ")
		)))
	}
}

fn require_section(space: &Space, id: &str) -> Result<()> {
	if space.has_section(id) {
		Ok(())
	} else {
		Err(no_such_section(id))
	}
}

/// An index arriving from the frontend, clamped into `0..=len`.
///
/// `i64` rather than `usize` so a negative value clamps instead of failing to
/// deserialise — the same reasoning that keeps `order` signed in the model.
fn clamp_index(index: i64, len: usize) -> usize {
	if index < 0 {
		0
	} else {
		(index as usize).min(len)
	}
}

/// The number of notes already in `section`, which is where the next one goes.
/// Correct because `normalise` leaves every group contiguous from zero.
fn group_len(space: &Space, section: &str) -> i64 {
	space.notes.iter().filter(|note| note.section == section).count() as i64
}

// --- notes -------------------------------------------------------------------

/// Appends a note to `section`, or to the active section when none is given.
pub fn add_note(space: &mut Space, body: &str, section: Option<&str>) -> Result<String> {
	let body = clean_body(body)?;
	let section = match section {
		Some(id) => {
			require_section(space, id)?;
			id.to_string()
		}
		None => space.active_section.clone(),
	};

	let id = ids::unique_id(ids::NOTE, |candidate| space.note(candidate).is_some());
	let now = now_rfc3339();
	space.notes.push(Note {
		id: id.clone(),
		order: group_len(space, &section),
		section,
		done: false,
		body,
		created: now.clone(),
		updated: now,
	});
	normalise(space);
	Ok(id)
}

pub fn edit_note(space: &mut Space, id: &str, body: &str) -> Result<()> {
	let body = clean_body(body)?;
	let now = now_rfc3339();
	let note = space
		.note_mut(id)
		.ok_or_else(|| no_such_note(id))?;
	note.body = body;
	note.updated = now;
	normalise(space);
	Ok(())
}

/// An explicit boolean rather than a toggle, so a multi-select whose notes
/// disagree has one unambiguous outcome (spec 5.1).
pub fn set_notes_done(space: &mut Space, ids: &[String], done: bool) -> Result<()> {
	let ids = dedupe_ids(ids)?;
	require_notes(space, &ids)?;

	let now = now_rfc3339();
	for id in &ids {
		let note = space.note_mut(id).expect("validated above");
		// Only a real change restamps `updated` (spec 5.6): marking an already-done
		// note done is not an edit.
		if note.done != done {
			note.done = done;
			note.updated = now.clone();
		}
	}
	normalise(space);
	Ok(())
}

pub fn delete_notes(space: &mut Space, ids: &[String]) -> Result<()> {
	let ids = dedupe_ids(ids)?;
	require_notes(space, &ids)?;

	let doomed: HashSet<&str> = ids.iter().map(String::as_str).collect();
	space.notes.retain(|note| !doomed.contains(note.id.as_str()));
	normalise(space);
	Ok(())
}

/// Moves one note to `index` within `section`, which may be a different section.
///
/// `index` is interpreted against the target list **after** the note has been
/// removed from it (spec 5.1). Without that rule a within-section move is
/// ambiguous: dragging a note down by one would mean two different destinations
/// depending on whether the index counted the note itself.
pub fn reorder_note(space: &mut Space, id: &str, section: &str, index: i64) -> Result<()> {
	let position = space
		.notes
		.iter()
		.position(|note| note.id == id)
		.ok_or_else(|| no_such_note(id))?;
	require_section(space, section)?;

	let mut moved = space.notes.remove(position);
	moved.section = section.to_string();

	let mut group: Vec<Note> = Vec::new();
	let mut rest: Vec<Note> = Vec::new();
	for note in space.notes.drain(..) {
		if note.section == section {
			group.push(note);
		} else {
			rest.push(note);
		}
	}
	group.insert(clamp_index(index, group.len()), moved);
	for (position, note) in group.iter_mut().enumerate() {
		note.order = position as i64;
	}

	rest.append(&mut group);
	space.notes = rest;
	// `updated` is deliberately untouched: this changed position, not content.
	normalise(space);
	Ok(())
}

/// Appends notes to `section`, keeping their relative order. A note already in
/// the target moves to the end.
pub fn move_notes(space: &mut Space, ids: &[String], section: &str) -> Result<()> {
	let ids = dedupe_ids(ids)?;
	require_notes(space, &ids)?;
	require_section(space, section)?;

	let selected: HashSet<&str> = ids.iter().map(String::as_str).collect();
	let mut moved: Vec<Note> = Vec::new();
	let mut rest: Vec<Note> = Vec::new();
	// Draining `space.notes` in place is what makes the result independent of the
	// order the caller listed the ids in: canonical order is the document's.
	for note in space.notes.drain(..) {
		if selected.contains(note.id.as_str()) {
			moved.push(note);
		} else {
			rest.push(note);
		}
	}

	// Renumber what stays before placing what arrives. Taking the group's *count*
	// as the first free order is wrong when the notes being moved came out of that
	// same group: the survivors keep their original orders, so the count collides
	// with the last of them and the tie-break by id decides the result instead.
	let mut next_order = 0;
	for note in rest.iter_mut().filter(|note| note.section == section) {
		note.order = next_order;
		next_order += 1;
	}
	for note in &mut moved {
		note.section = section.to_string();
		note.order = next_order;
		next_order += 1;
	}

	rest.append(&mut moved);
	space.notes = rest;
	normalise(space);
	Ok(())
}

/// Folds the selected notes into the canonically first one.
pub fn merge_notes(space: &mut Space, ids: &[String]) -> Result<()> {
	let ids = dedupe_ids(ids)?;
	if ids.len() < 2 {
		// After deduplication, so `merge_notes([A, A])` lands here: it names one
		// note, and merging a note with itself would rewrite its body to a
		// duplicate of itself joined by a blank line (spec 5.5).
		return Err(StoreError::Invalid(
			"merging needs at least two different notes".into(),
		));
	}
	require_notes(space, &ids)?;

	let selected: HashSet<&str> = ids.iter().map(String::as_str).collect();
	let positions: Vec<usize> = space
		.notes
		.iter()
		.enumerate()
		.filter(|(_, note)| selected.contains(note.id.as_str()))
		.map(|(position, _)| position)
		.collect();

	let body = positions
		.iter()
		.map(|&position| space.notes[position].body.as_str())
		.collect::<Vec<_>>()
		.join("\n\n");
	let done = positions.iter().all(|&position| space.notes[position].done);

	// The survivor is first in canonical order and keeps its id, section, order
	// and created — a merge produces an older note with more in it, not a new one.
	let survivor = space.notes[positions[0]].id.clone();
	let note = &mut space.notes[positions[0]];
	note.done = done;
	note.body = body;
	note.updated = now_rfc3339();

	space
		.notes
		.retain(|note| note.id == survivor || !selected.contains(note.id.as_str()));
	normalise(space);
	Ok(())
}

// --- sections ----------------------------------------------------------------

/// Appends a section and makes it active immediately (spec 5.3), so the next
/// capture lands where the user just looked.
pub fn add_section(space: &mut Space, name: &str) -> Result<String> {
	let name = clean_name(name)?;
	let id = ids::unique_id(ids::SECTION, |candidate| space.has_section(candidate));
	space.sections.push(Section {
		id: id.clone(),
		name,
		order: space.sections.len() as i64,
	});
	space.active_section = id.clone();
	normalise(space);
	Ok(id)
}

/// The section a `# Name` directive means, if the document already has one.
///
/// Case-insensitive over [`normalise_name`]d names, so `Research`, `research`
/// and `Deep   Research` all resolve the way the person typing them expects. The
/// *first* match in document order wins: a hand-edited file may hold two
/// sections that collide under this rule, and the directive has to name one of
/// them deterministically.
///
/// Read-only and public because `submit_entry` consults it **before** mutating,
/// to decide whether the operation is snapshotted (task-003 §4.3 excludes
/// activation from the undo stack, exactly as `set_active_section` is excluded).
pub fn section_by_name<'a>(space: &'a Space, name: &str) -> Option<&'a Section> {
	let wanted = normalise_name(name).to_lowercase();
	space
		.sections
		.iter()
		.find(|section| normalise_name(&section.name).to_lowercase() == wanted)
}

/// Creates the named section and makes it active, or activates the one that
/// already carries that name.
///
/// Both mutations happen inside **one** op rather than two sequential commands,
/// so the whole thing is a single entry on the snapshot stack and one `Ctrl+Z`
/// removes the section *and* restores the previously active one. Two commands
/// would push two snapshots and take two presses.
///
/// Returns `(section_id, created)`. `created: false` means a duplicate name
/// resolved to an existing section — a switch, not an error, and deliberately
/// not announced as one: creating a second `Research` would produce two visually
/// identical headers and make `Move to ▸` ambiguous.
///
/// Stays `Fn`-shaped like every other op, so `mutate` can re-apply it against a
/// freshly re-read document after a write conflict.
pub fn add_section_and_activate(space: &mut Space, name: &str) -> Result<(String, bool)> {
	// Normalised **here**, at the boundary, rather than trusted from the caller.
	// The name this stores and the name duplicates are matched against then come
	// out of the same expression, so a caller passing a raw 200-character
	// double-spaced string cannot store one form and be looked up by another.
	let name = clean_name(&normalise_name(name))?;

	// Through `set_active_section` for the same reason the create arm below goes
	// through `add_section`: activation is spec 4.3's own operation, and a second
	// place that assigns `active_section` is a second place for it to stop
	// agreeing with the one `set_active_section` performs.
	if let Some(id) = section_by_name(space, &name).map(|section| section.id.clone()) {
		set_active_section(space, &id)?;
		return Ok((id, false));
	}

	// `add_section` already appends *and* activates (spec 5.3), so the create arm
	// is exactly it — reimplementing the push here would be a second place for the
	// id-collision loop and the ordering to be got wrong.
	let id = add_section(space, &name)?;
	Ok((id, true))
}

pub fn rename_section(space: &mut Space, id: &str, name: &str) -> Result<()> {
	// Duplicate names are allowed — ids are identity (spec 5.4).
	let name = clean_name(name)?;
	let section = space
		.sections
		.iter_mut()
		.find(|section| section.id == id)
		.ok_or_else(|| no_such_section(id))?;
	section.name = name;
	normalise(space);
	Ok(())
}

/// Index interpreted against the list **after** removal, exactly as
/// `reorder_note`: moving B to index 2 in `[A, B, C, D]` yields `[A, C, B, D]`.
pub fn reorder_section(space: &mut Space, id: &str, index: i64) -> Result<()> {
	let position = space
		.section_index(id)
		.ok_or_else(|| no_such_section(id))?;

	let section = space.sections.remove(position);
	let target = clamp_index(index, space.sections.len());
	space.sections.insert(target, section);
	for (position, section) in space.sections.iter_mut().enumerate() {
		section.order = position as i64;
	}
	normalise(space);
	Ok(())
}

pub fn set_active_section(space: &mut Space, id: &str) -> Result<()> {
	require_section(space, id)?;
	space.active_section = id.to_string();
	normalise(space);
	Ok(())
}

/// Deletes a section **and the notes in it** (spec 5.3, resolving Q11 — this is
/// a deliberate deviation from the more common "move the notes elsewhere"; undo
/// covers the deletion).
///
/// Refusing to delete the last section is what guarantees a capture target
/// always exists, which is the same invariant `normalise`'s first step defends
/// from the other direction.
pub fn delete_section(space: &mut Space, id: &str) -> Result<()> {
	let position = space
		.section_index(id)
		.ok_or_else(|| no_such_section(id))?;
	if space.sections.len() == 1 {
		return Err(StoreError::Invalid(
			"the last section cannot be deleted — a space always needs somewhere to capture into"
				.into(),
		));
	}

	// The preceding section, or the following one when deleting the first.
	let successor = if position > 0 {
		space.sections[position - 1].id.clone()
	} else {
		space.sections[1].id.clone()
	};

	space.notes.retain(|note| note.section != id);
	space.sections.remove(position);
	if space.active_section == id {
		space.active_section = successor;
	}
	normalise(space);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn space() -> Space {
		let mut space = Space {
			id: "spc_00000001".into(),
			name: "test".into(),
			active_section: "sec_aaaaaaaa".into(),
			sections: vec![
				Section {
					id: "sec_aaaaaaaa".into(),
					name: "Alpha".into(),
					order: 0,
				},
				Section {
					id: "sec_bbbbbbbb".into(),
					name: "Beta".into(),
					order: 1,
				},
			],
			notes: Vec::new(),
		};
		for (section, count) in [("sec_aaaaaaaa", 3), ("sec_bbbbbbbb", 2)] {
			for offset in 0..count {
				space.notes.push(Note {
					id: format!("nte_{}{offset}", &section[4..8]),
					section: section.to_string(),
					order: offset,
					done: false,
					body: format!("{section} note {offset}"),
					created: "2026-07-30T14:00:00Z".into(),
					updated: "2026-07-30T14:00:00Z".into(),
				});
			}
		}
		normalise(&mut space);
		space
	}

	fn ids_in(space: &Space, section: &str) -> Vec<String> {
		space
			.notes
			.iter()
			.filter(|note| note.section == section)
			.map(|note| note.id.clone())
			.collect()
	}

	fn strings(values: &[&str]) -> Vec<String> {
		values.iter().map(|value| value.to_string()).collect()
	}

	// --- add_note ---

	#[test]
	fn add_note_appends_to_the_active_section() {
		let mut space = space();
		let id = add_note(&mut space, "fresh", None).unwrap();

		let note = space.note(&id).unwrap();
		assert_eq!(note.section, "sec_aaaaaaaa");
		assert_eq!(note.order, 3);
		assert_eq!(note.created, note.updated);
		assert_eq!(ids_in(&space, "sec_aaaaaaaa").last().unwrap(), &id);
	}

	#[test]
	fn add_note_targets_a_named_section() {
		let mut space = space();
		let id = add_note(&mut space, "fresh", Some("sec_bbbbbbbb")).unwrap();
		assert_eq!(space.note(&id).unwrap().section, "sec_bbbbbbbb");
	}

	#[test]
	fn add_note_trims_trailing_whitespace_and_keeps_leading() {
		let mut space = space();
		let id = add_note(&mut space, "    indented code   \n\n", None).unwrap();
		assert_eq!(space.note(&id).unwrap().body, "    indented code");
	}

	#[test]
	fn add_note_rejects_an_empty_body() {
		let mut space = space();
		let err = add_note(&mut space, "   \n\t ", None).unwrap_err();
		assert_eq!(err.kind(), "invalid");
	}

	#[test]
	fn add_note_rejects_an_unknown_section() {
		let mut space = space();
		let err = add_note(&mut space, "fresh", Some("sec_nope")).unwrap_err();
		assert_eq!(err.kind(), "not-found");
		assert_eq!(space.notes.len(), 5);
	}

	// --- edit_note ---

	#[test]
	fn edit_note_replaces_the_body_and_bumps_updated() {
		let mut space = space();
		let id = space.notes[0].id.clone();
		edit_note(&mut space, &id, "rewritten").unwrap();

		let note = space.note(&id).unwrap();
		assert_eq!(note.body, "rewritten");
		assert_ne!(note.updated, note.created);
	}

	#[test]
	fn edit_note_rejects_an_empty_body_rather_than_deleting() {
		let mut space = space();
		let id = space.notes[0].id.clone();
		let err = edit_note(&mut space, &id, "  ").unwrap_err();

		assert_eq!(err.kind(), "invalid");
		assert!(space.note(&id).is_some(), "the note was destroyed");
	}

	#[test]
	fn edit_note_rejects_an_unknown_id() {
		let mut space = space();
		assert_eq!(
			edit_note(&mut space, "nte_nope", "x").unwrap_err().kind(),
			"not-found"
		);
	}

	// --- set_notes_done ---

	#[test]
	fn set_notes_done_bumps_updated_only_where_the_value_changed() {
		let mut space = space();
		let first = space.notes[0].id.clone();
		let second = space.notes[1].id.clone();
		set_notes_done(&mut space, &strings(&[&first]), true).unwrap();
		let stamped = space.note(&first).unwrap().updated.clone();

		// `first` is already done, so only `second` should restamp.
		set_notes_done(&mut space, &strings(&[&first, &second]), true).unwrap();

		assert_eq!(space.note(&first).unwrap().updated, stamped);
		assert_ne!(space.note(&second).unwrap().updated, "2026-07-30T14:00:00Z");
		assert!(space.note(&first).unwrap().done);
		assert!(space.note(&second).unwrap().done);
	}

	#[test]
	fn set_notes_done_rejects_the_whole_call_on_one_bad_id() {
		let mut space = space();
		let good = space.notes[0].id.clone();
		let err = set_notes_done(&mut space, &strings(&[&good, "nte_nope"]), true).unwrap_err();

		assert_eq!(err.kind(), "not-found");
		assert!(!space.note(&good).unwrap().done, "a partial change reached the document");
	}

	#[test]
	fn multi_id_operations_reject_an_empty_list() {
		let mut space = space();
		assert_eq!(
			set_notes_done(&mut space, &[], true).unwrap_err().kind(),
			"invalid"
		);
		assert_eq!(delete_notes(&mut space, &[]).unwrap_err().kind(), "invalid");
	}

	#[test]
	fn multi_id_operations_deduplicate() {
		let mut space = space();
		let id = space.notes[0].id.clone();
		delete_notes(&mut space, &strings(&[&id, &id])).unwrap();
		assert_eq!(space.notes.len(), 4);
	}

	// --- delete_notes ---

	#[test]
	fn delete_notes_removes_and_renumbers() {
		let mut space = space();
		let first = space.notes[0].id.clone();
		delete_notes(&mut space, &strings(&[&first])).unwrap();

		assert!(space.note(&first).is_none());
		let group = ids_in(&space, "sec_aaaaaaaa");
		assert_eq!(group.len(), 2);
		for (index, id) in group.iter().enumerate() {
			assert_eq!(space.note(id).unwrap().order, index as i64);
		}
	}

	// --- reorder_note ---

	#[test]
	fn reorder_note_interprets_the_index_after_removal() {
		let mut space = space();
		let group = ids_in(&space, "sec_aaaaaaaa");
		// [0, 1, 2] -> move the first to index 2 -> [1, 2, 0]
		reorder_note(&mut space, &group[0], "sec_aaaaaaaa", 2).unwrap();
		assert_eq!(
			ids_in(&space, "sec_aaaaaaaa"),
			vec![group[1].clone(), group[2].clone(), group[0].clone()]
		);
	}

	#[test]
	fn reorder_note_clamps_an_out_of_range_index() {
		let mut space = space();
		let group = ids_in(&space, "sec_aaaaaaaa");
		reorder_note(&mut space, &group[0], "sec_aaaaaaaa", 900).unwrap();
		assert_eq!(ids_in(&space, "sec_aaaaaaaa").last().unwrap(), &group[0]);

		reorder_note(&mut space, &group[0], "sec_aaaaaaaa", -5).unwrap();
		assert_eq!(ids_in(&space, "sec_aaaaaaaa").first().unwrap(), &group[0]);
	}

	#[test]
	fn reorder_note_moves_across_sections_without_touching_updated() {
		let mut space = space();
		let id = space.notes[0].id.clone();
		let stamp = space.note(&id).unwrap().updated.clone();

		reorder_note(&mut space, &id, "sec_bbbbbbbb", 0).unwrap();

		assert_eq!(space.note(&id).unwrap().section, "sec_bbbbbbbb");
		assert_eq!(ids_in(&space, "sec_bbbbbbbb").first().unwrap(), &id);
		assert_eq!(space.note(&id).unwrap().updated, stamp);
	}

	#[test]
	fn reorder_note_rejects_unknown_ids() {
		let mut space = space();
		let id = space.notes[0].id.clone();
		assert_eq!(
			reorder_note(&mut space, "nte_nope", "sec_aaaaaaaa", 0).unwrap_err().kind(),
			"not-found"
		);
		assert_eq!(
			reorder_note(&mut space, &id, "sec_nope", 0).unwrap_err().kind(),
			"not-found"
		);
	}

	// --- move_notes ---

	#[test]
	fn move_notes_appends_in_canonical_order_regardless_of_argument_order() {
		let mut space = space();
		let group = ids_in(&space, "sec_aaaaaaaa");
		let scrambled = strings(&[&group[2], &group[0]]);

		move_notes(&mut space, &scrambled, "sec_bbbbbbbb").unwrap();

		let target = ids_in(&space, "sec_bbbbbbbb");
		assert_eq!(target.len(), 4);
		assert_eq!(&target[2..], &[group[0].clone(), group[2].clone()]);
	}

	#[test]
	fn move_notes_within_the_same_section_moves_to_the_end() {
		let mut space = space();
		let group = ids_in(&space, "sec_aaaaaaaa");
		move_notes(&mut space, &strings(&[&group[0]]), "sec_aaaaaaaa").unwrap();
		assert_eq!(ids_in(&space, "sec_aaaaaaaa").last().unwrap(), &group[0]);
	}

	#[test]
	fn move_notes_rejects_every_bad_argument_without_moving_anything() {
		let mut space = space();
		let id = space.notes[0].id.clone();
		let before = space.clone();

		assert_eq!(
			move_notes(&mut space, &[], "sec_bbbbbbbb").unwrap_err().kind(),
			"invalid"
		);
		assert_eq!(
			move_notes(&mut space, &strings(&[&id, "nte_nope"]), "sec_bbbbbbbb")
				.unwrap_err()
				.kind(),
			"not-found"
		);
		assert_eq!(
			move_notes(&mut space, &strings(&[&id]), "sec_nope").unwrap_err().kind(),
			"not-found"
		);
		assert_eq!(space, before, "a rejected move still changed the document");
	}

	#[test]
	fn merge_notes_rejects_an_unknown_id_without_merging() {
		let mut space = space();
		let id = space.notes[0].id.clone();
		let before = space.clone();

		let err = merge_notes(&mut space, &strings(&[&id, "nte_nope"])).unwrap_err();

		assert_eq!(err.kind(), "not-found");
		assert_eq!(space, before, "a rejected merge still changed the document");
	}

	#[test]
	fn delete_notes_rejects_an_unknown_id_without_deleting() {
		let mut space = space();
		let id = space.notes[0].id.clone();
		let before = space.clone();

		let err = delete_notes(&mut space, &strings(&[&id, "nte_nope"])).unwrap_err();

		assert_eq!(err.kind(), "not-found");
		assert_eq!(space, before, "a rejected delete still removed a note");
	}

	#[test]
	fn move_notes_does_not_bump_updated() {
		let mut space = space();
		let id = space.notes[0].id.clone();
		let stamp = space.note(&id).unwrap().updated.clone();
		move_notes(&mut space, &strings(&[&id]), "sec_bbbbbbbb").unwrap();
		assert_eq!(space.note(&id).unwrap().updated, stamp);
	}

	// --- merge_notes ---

	#[test]
	fn merge_notes_folds_into_the_canonically_first_note() {
		let mut space = space();
		let group = ids_in(&space, "sec_aaaaaaaa");
		let created = space.note(&group[0]).unwrap().created.clone();

		merge_notes(&mut space, &strings(&[&group[2], &group[0]])).unwrap();

		let survivor = space.note(&group[0]).unwrap();
		assert_eq!(survivor.body, "sec_aaaaaaaa note 0\n\nsec_aaaaaaaa note 2");
		assert_eq!(survivor.created, created);
		assert_eq!(survivor.section, "sec_aaaaaaaa");
		assert_ne!(survivor.updated, created);
		assert!(space.note(&group[2]).is_none());
		assert_eq!(space.notes.len(), 4);
	}

	#[test]
	fn merge_notes_marks_done_only_when_every_note_was_done() {
		let mut space = space();
		let group = ids_in(&space, "sec_aaaaaaaa");
		set_notes_done(&mut space, &strings(&[&group[0]]), true).unwrap();

		merge_notes(&mut space, &strings(&[&group[0], &group[1]])).unwrap();
		assert!(!space.note(&group[0]).unwrap().done);

		let remaining = ids_in(&space, "sec_aaaaaaaa");
		set_notes_done(&mut space, &remaining, true).unwrap();
		merge_notes(&mut space, &remaining).unwrap();
		assert!(space.note(&remaining[0]).unwrap().done);
	}

	#[test]
	fn merge_notes_rejects_fewer_than_two_distinct_ids() {
		let mut space = space();
		let id = space.notes[0].id.clone();
		assert_eq!(
			merge_notes(&mut space, &strings(&[&id])).unwrap_err().kind(),
			"invalid"
		);
		assert_eq!(
			merge_notes(&mut space, &strings(&[&id, &id])).unwrap_err().kind(),
			"invalid"
		);
		assert_eq!(space.notes.len(), 5);
	}

	// --- sections ---

	#[test]
	fn add_section_appends_and_activates() {
		let mut space = space();
		let id = add_section(&mut space, "  Gamma  ").unwrap();

		assert_eq!(space.active_section, id);
		assert_eq!(space.sections.last().unwrap().name, "Gamma");
		assert_eq!(space.sections.last().unwrap().order, 2);
	}

	#[test]
	fn add_section_rejects_an_empty_name() {
		let mut space = space();
		assert_eq!(add_section(&mut space, "   ").unwrap_err().kind(), "invalid");
	}

	// --- add_section_and_activate ---

	#[test]
	fn add_section_and_activate_creates_when_the_name_is_new() {
		let mut space = space();
		let (id, created) = add_section_and_activate(&mut space, "Gamma").unwrap();

		assert!(created);
		assert_eq!(space.active_section, id);
		assert_eq!(space.sections.len(), 3);
		assert_eq!(space.sections.last().unwrap().name, "Gamma");
	}

	#[test]
	fn add_section_and_activate_resolves_a_duplicate_name_to_the_existing_section() {
		let mut space = space();
		let before = space.sections.clone();

		// Case and surrounding whitespace both fold away.
		for name in ["Beta", "beta", "  BETA  ", "\tBeta\n"] {
			let (id, created) = add_section_and_activate(&mut space, name).unwrap();
			assert!(!created, "{name:?} created a second section");
			assert_eq!(id, "sec_bbbbbbbb");
			assert_eq!(space.active_section, "sec_bbbbbbbb");
		}
		assert_eq!(space.sections, before, "resolving a duplicate changed the sections");
	}

	#[test]
	fn add_section_and_activate_matches_across_collapsed_whitespace() {
		let mut space = space();
		add_section_and_activate(&mut space, "Deep Research").unwrap();
		let (_, created) = add_section_and_activate(&mut space, "Deep    Research").unwrap();

		assert!(!created);
		assert_eq!(space.sections.len(), 3);
	}

	#[test]
	fn add_section_and_activate_stores_the_name_it_matches_duplicates_on() {
		let mut space = space();
		let long = format!("  Deep   {}  ", "x".repeat(200));
		let (id, created) = add_section_and_activate(&mut space, &long).unwrap();

		assert!(created);
		let stored = &space.sections.iter().find(|s| s.id == id).unwrap().name;
		// Collapsed and capped at the boundary, so the stored name and the name a
		// later lookup normalises to are the same string.
		assert_eq!(stored.chars().count(), 80);
		assert_eq!(stored, &normalise_name(&long));
		assert_eq!(section_by_name(&space, &long).unwrap().id, id);

		// And the round trip holds: submitting it again resolves to this section.
		let (again, created) = add_section_and_activate(&mut space, &long).unwrap();
		assert!(!created);
		assert_eq!(again, id);
	}

	#[test]
	fn add_section_and_activate_rejects_an_empty_name() {
		let mut space = space();
		let err = add_section_and_activate(&mut space, "   ").unwrap_err();

		assert_eq!(err.kind(), "invalid");
		assert_eq!(space.sections.len(), 2);
	}

	#[test]
	fn section_by_name_reads_without_mutating() {
		let space = space();
		assert_eq!(section_by_name(&space, "alpha").unwrap().id, "sec_aaaaaaaa");
		assert_eq!(section_by_name(&space, " ALPHA ").unwrap().id, "sec_aaaaaaaa");
		assert!(section_by_name(&space, "Gamma").is_none());
	}

	#[test]
	fn rename_section_allows_duplicate_names() {
		let mut space = space();
		rename_section(&mut space, "sec_bbbbbbbb", "Alpha").unwrap();
		assert_eq!(space.sections[1].name, "Alpha");
	}

	#[test]
	fn rename_section_rejects_empty_names_and_unknown_ids() {
		let mut space = space();
		assert_eq!(
			rename_section(&mut space, "sec_aaaaaaaa", " ").unwrap_err().kind(),
			"invalid"
		);
		assert_eq!(
			rename_section(&mut space, "sec_nope", "x").unwrap_err().kind(),
			"not-found"
		);
	}

	#[test]
	fn reorder_section_interprets_the_index_after_removal() {
		let mut space = space();
		for name in ["Gamma", "Delta"] {
			add_section(&mut space, name).unwrap();
		}
		let ids: Vec<String> = space.sections.iter().map(|s| s.id.clone()).collect();

		// [A, B, C, D] -> move B to index 2 -> [A, C, B, D]
		reorder_section(&mut space, &ids[1], 2).unwrap();

		let after: Vec<&str> = space.sections.iter().map(|s| s.id.as_str()).collect();
		assert_eq!(after, [&ids[0], &ids[2], &ids[1], &ids[3]]);
		assert_eq!(space.sections[2].order, 2);
	}

	#[test]
	fn reorder_section_clamps() {
		let mut space = space();
		reorder_section(&mut space, "sec_aaaaaaaa", 99).unwrap();
		assert_eq!(space.sections.last().unwrap().id, "sec_aaaaaaaa");
		// The notes follow their section.
		assert_eq!(space.notes[0].section, "sec_bbbbbbbb");
	}

	#[test]
	fn set_active_section_validates() {
		let mut space = space();
		set_active_section(&mut space, "sec_bbbbbbbb").unwrap();
		assert_eq!(space.active_section, "sec_bbbbbbbb");
		assert_eq!(
			set_active_section(&mut space, "sec_nope").unwrap_err().kind(),
			"not-found"
		);
	}

	#[test]
	fn delete_section_takes_its_notes_with_it() {
		let mut space = space();
		delete_section(&mut space, "sec_aaaaaaaa").unwrap();

		assert_eq!(space.sections.len(), 1);
		assert_eq!(space.notes.len(), 2);
		assert!(space.notes.iter().all(|note| note.section == "sec_bbbbbbbb"));
		// The deleted section was first and active, so the following one takes over.
		assert_eq!(space.active_section, "sec_bbbbbbbb");
	}

	#[test]
	fn delete_section_activates_the_preceding_section() {
		let mut space = space();
		set_active_section(&mut space, "sec_bbbbbbbb").unwrap();
		delete_section(&mut space, "sec_bbbbbbbb").unwrap();
		assert_eq!(space.active_section, "sec_aaaaaaaa");
	}

	#[test]
	fn delete_section_leaves_the_active_section_alone_when_it_was_not_deleted() {
		let mut space = space();
		add_section(&mut space, "Gamma").unwrap();
		let gamma = space.active_section.clone();
		delete_section(&mut space, "sec_bbbbbbbb").unwrap();
		assert_eq!(space.active_section, gamma);
	}

	#[test]
	fn delete_section_refuses_the_last_one() {
		let mut space = space();
		delete_section(&mut space, "sec_bbbbbbbb").unwrap();
		let err = delete_section(&mut space, "sec_aaaaaaaa").unwrap_err();

		assert_eq!(err.kind(), "invalid");
		assert_eq!(space.sections.len(), 1);
	}

	#[test]
	fn delete_section_rejects_an_unknown_id() {
		let mut space = space();
		assert_eq!(
			delete_section(&mut space, "sec_nope").unwrap_err().kind(),
			"not-found"
		);
	}
}
