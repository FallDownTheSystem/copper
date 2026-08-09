//! fzf-style subsequence matching over note bodies: does this text contain the
//! query's characters in order, and how good is the match.
//!
//! A port of `src/lib/fuzzyMatch.ts`, and the same answers on the same inputs
//! (`tests/search.rs` is that file's corpus). It is here rather than only in the
//! frontend because the CLI has to search too, and two subsequence matchers is
//! two definitions of what "matches" means.
//!
//! **The query is a character sequence, not a list of words.** Whitespace is
//! stripped before matching, so `http req` matches "Send **HTTP** **req**uests"
//! and also "**h**y**p**er**t**ext" — the second is a genuine match, it simply
//! scores far below the first.
//!
//! # What the port dropped, and what it could not
//!
//! Everything in the TypeScript that exists because JavaScript strings are UTF-16
//! is gone: the `Int32Array` scratch buffers, the surrogate-pair branch, the
//! offset map from code points back to code units. A Rust `char` is already a
//! Unicode scalar value, so there is no second index space to bridge.
//!
//! What could not be dropped is the scan itself. The best match is rarely the
//! leftmost one — in ordinary prose the letters of `error` can be assembled from
//! five scattered words long before the word appears — so every *maximal-tight
//! window* is enumerated, fzf-v1 style. One forward pass finds the earliest
//! position the needle can finish at; one backward pass from there finds the
//! latest position it can start at; that window is scored both ways and the
//! better kept; the scan resumes one character after the window's start. Each
//! round strictly advances the start, so the loop terminates.
//!
//! Getting the two tie-breaks backwards is the one defect here that no type would
//! catch, so both are stated where they are decided, in [`fuzzy_match`].
//!
//! # Case folding
//!
//! Folded once per string rather than per comparison, and in two passes for the
//! same reason the TypeScript uses two:
//!
//! - **Per character first**, because a character whose fold is *several*
//!   characters — `İ` becomes `i` plus a combining dot — cannot stand for one
//!   needle character and is marked unmatchable rather than approximated. It goes
//!   unfound, which is the safe direction.
//! - **Then the whole string, where that is safe.** `Σ` at the end of a word
//!   lowercases to `ς`, which no per-character fold can know. When no character's
//!   fold changed length, the string is re-folded as a whole and Rust's
//!   `str::to_lowercase` — which implements the same Unicode special-casing table
//!   JavaScript's `toLowerCase` does — gets the context-sensitive cases right.
//!
//! **No Unicode normalization.** A decomposed `é` and a composed one are
//! different sequences here and do not match each other.

use crate::js;
use crate::store::model::{Note, Space};

/// A matched character, before any bonus.
const SCORE_CHAR: i32 = 16;
/// Immediately after the previous match — what makes a contiguous run win.
const BONUS_CONSECUTIVE: i32 = 8;
/// First character, or the first after a separator.
const BONUS_BOUNDARY: i32 = 12;
/// A `camelCase` hump: a word start with no separator in front of it.
const BONUS_CAMEL: i32 = 8;
/// Per character skipped inside the match, uncapped — a match spread over half a
/// note really is worse than a tight one, however far apart the two ends are.
const PENALTY_GAP: i32 = 2;
/// Per character skipped before the first match.
const PENALTY_LEADING: i32 = 3;
/// ...but capped, unlike the gap penalty. Beyond a few characters "the match
/// starts late" stops carrying information: a hit at offset 400 and one at offset
/// 4,000 are equally "not at the beginning", and leaving this uncapped would let
/// a note's *length* decide its rank.
const MAX_LEADING_PENALTY: i32 = 15;

/// Where the best assembly landed, and what it scored.
pub struct FuzzyMatch {
	pub score: i32,
	/// Byte offset into the haystack of each matched character, ascending, one
	/// per needle character.
	///
	/// Nothing in the CLI reads these — spec 9 keeps highlighting out of the core.
	/// They are here because the window-selection rules above are the part of this
	/// port a reviewer cannot check by eye, and a test that can only see a score
	/// cannot tell "found the contiguous run" from "found a scatter that happened
	/// to score the same".
	pub starts: Vec<usize>,
}

/// The needle a query becomes: whitespace stripped, folded once.
///
/// Whitespace by JavaScript's definition rather than Rust's — see
/// [`crate::js::is_whitespace`] — because a query is typed into the same box in
/// both front ends and must become the same needle in both.
pub fn fuzzy_needle(query: &str) -> String {
	query
		.chars()
		.filter(|&ch| !js::is_whitespace(ch))
		.collect::<String>()
		.to_lowercase()
}

/// The notes matching `query`, in canonical document order.
///
/// **Not ranked.** `space.notes` is already in the order the panel shows, and
/// that is the order a scripted caller can predict; sorting by score would make
/// `copper note list` and `copper search` disagree about what "first" means for
/// no benefit a CLI can use. Scoring still runs — it is what decides *whether* a
/// scattered assembly is a match at all — its result simply does not reorder
/// anything.
///
/// `exact` swaps the subsequence matcher for a plain case-insensitive substring
/// test, for callers that want the predictable answer rather than the generous
/// one.
///
/// An empty query matches **nothing** in either mode, rather than everything.
/// "No query" is a separate state in every caller, and a `--query ''` that
/// selected the whole space would be a surprising way to copy it.
pub fn search_notes<'a>(
	space: &'a Space,
	query: &str,
	section: Option<&str>,
	done: Option<bool>,
	exact: bool,
) -> Vec<&'a Note> {
	let candidates = space
		.notes
		.iter()
		.filter(|note| section.is_none_or(|id| note.section == id))
		.filter(|note| done.is_none_or(|wanted| note.done == wanted));

	if exact {
		let needle = query.to_lowercase();
		if needle.is_empty() {
			return Vec::new();
		}
		candidates
			.filter(|note| note.body.to_lowercase().contains(&needle))
			.collect()
	} else {
		let needle = fuzzy_needle(query);
		candidates
			.filter(|note| fuzzy_score(&note.body, &needle).is_some())
			.collect()
	}
}

/// [`fuzzy_match`] without the positions, for the callers that only ask whether.
pub fn fuzzy_score(haystack: &str, needle: &str) -> Option<i32> {
	fuzzy_match(haystack, needle).map(|found| found.score)
}

/// The best match of `needle` in `haystack`, or `None` when the characters do not
/// appear in order.
///
/// `needle` must already have been through [`fuzzy_needle`]. An empty needle
/// matches nothing rather than everything.
pub fn fuzzy_match(haystack: &str, needle: &str) -> Option<FuzzyMatch> {
	let wanted: Vec<char> = needle.chars().collect();
	if wanted.is_empty() {
		return None;
	}
	let text = Folded::of(haystack);
	if text.len() < wanted.len() {
		return None;
	}

	let mut leftmost = vec![0usize; wanted.len()];
	let mut rightmost = vec![0usize; wanted.len()];
	let mut best = vec![0usize; wanted.len()];
	let mut best_score = 0;
	let mut found = false;
	let mut from = 0;

	while from + wanted.len() <= text.len() {
		// Nothing later can succeed either: a pass from further right has strictly
		// fewer characters to work with.
		if !forward(&text, &wanted, from, &mut leftmost) {
			break;
		}
		backward(&text, &wanted, leftmost[wanted.len() - 1], &mut rightmost);

		let left = score_of(&text, &leftmost);
		let right = score_of(&text, &rightmost);

		// **Inside one window a tie goes to the slid assembly** (`>=`), which is the
		// one with the longer runs — the same match described in fewer pieces.
		// `a-b-abc` is exactly that tie: the scattered assembly's two boundary
		// bonuses come to the same total as the contiguous one's consecutive
		// bonuses.
		let window = left.max(right);
		// **Across windows the comparison stays strict** (`>`), so the earliest of
		// two equally good matches wins and the same text and query always answer
		// the same however often the text repeats itself.
		if !found || window > best_score {
			found = true;
			best_score = window;
			best.copy_from_slice(if right >= left { &rightmost } else { &leftmost });
		}

		// The window's own start, which the backward pass has just proved is the
		// latest one for this end — so the next round considers a genuinely
		// different window, and the loop advances by at least one character.
		from = rightmost[0] + 1;
	}

	found.then(|| FuzzyMatch {
		score: best_score,
		starts: best.iter().map(|&at| text.offsets[at]).collect(),
	})
}

/// A string as folded characters, aligned to it.
struct Folded {
	/// The source characters, for the boundary bonus, which asks what the writer
	/// typed rather than what it folds to.
	source: Vec<char>,
	/// One entry per source character: its fold, or `None` where the fold is not
	/// a single character and so can never equal one needle character.
	points: Vec<Option<char>>,
	/// Byte offset of each character, with a final sentinel.
	offsets: Vec<usize>,
}

impl Folded {
	fn of(source: &str) -> Self {
		let mut chars = Vec::new();
		let mut points = Vec::new();
		let mut offsets = Vec::new();
		let mut ascii = true;
		let mut every_fold_is_one_character = true;

		for (offset, ch) in source.char_indices() {
			offsets.push(offset);
			chars.push(ch);
			if !ch.is_ascii() {
				ascii = false;
			}

			let mut lowered = ch.to_lowercase();
			let folded = match (lowered.next(), lowered.next()) {
				(Some(one), None) => Some(one),
				_ => {
					every_fold_is_one_character = false;
					None
				}
			};
			points.push(folded);
		}
		offsets.push(source.len());

		// Only safe when no fold changed length: the whole-string fold and the
		// per-character one then have the same character count, so index `k` means
		// the same character in both. The count is verified rather than assumed.
		if !ascii && every_fold_is_one_character {
			let lowered = source.to_lowercase();
			if lowered.chars().count() == points.len() {
				for (slot, ch) in points.iter_mut().zip(lowered.chars()) {
					*slot = Some(ch);
				}
			}
		}

		Self {
			source: chars,
			points,
			offsets,
		}
	}

	fn len(&self) -> usize {
		self.points.len()
	}
}

/// `\p{L}` or `\p{N}` — the one place this port is knowingly approximate.
///
/// `char::is_alphanumeric` is `Alphabetic | N`, and `Alphabetic` is wider than
/// `\p{L}`: it also holds Other_Alphabetic, which is mostly combining marks.
/// Where the two disagree, a character the TypeScript treats as a separator is
/// treated here as part of a word, so the character after it earns no boundary
/// bonus. `"\u{345}a a"` searched for `"a"` is a worked example — the TypeScript
/// picks the first `a`, this picks the second.
///
/// It cannot change **whether** a text matches, only which equally-valid assembly
/// wins and what it scores, and `search_notes` returns document order rather than
/// score order — so no CLI output moves. Closing it exactly needs a
/// General_Category table, which is a dependency this crate has no other use for
/// and which `cargo tree -p copper-cli` is an acceptance criterion about.
fn is_word(ch: char) -> bool {
	ch.is_alphanumeric()
}

fn boundary_bonus(text: &Folded, at: usize) -> i32 {
	if at == 0 {
		return BONUS_BOUNDARY;
	}

	let before = text.source[at - 1];
	if !is_word(before) {
		return BONUS_BOUNDARY;
	}

	// A hump rather than a separator: `Request` inside `sendRequest`. Worth less
	// than a real word start, because the writer did not put a break there. Read
	// off the fold rather than by re-lowercasing: a character the fold left alone
	// is one that was not uppercase.
	let here = text.source[at];
	if text.points[at - 1] == Some(before) && text.points[at] != Some(here) {
		return BONUS_CAMEL;
	}
	0
}

fn score_of(text: &Folded, positions: &[usize]) -> i32 {
	let mut total = 0;
	let mut previous: Option<usize> = None;

	for &at in positions {
		total += SCORE_CHAR;
		if let Some(previous) = previous {
			if at == previous + 1 {
				total += BONUS_CONSECUTIVE;
			} else {
				total -= PENALTY_GAP * (at - previous - 1) as i32;
			}
		}
		total += boundary_bonus(text, at);
		previous = Some(at);
	}

	let first = positions.first().copied().unwrap_or(0) as i32;
	total - PENALTY_LEADING.saturating_mul(first).min(MAX_LEADING_PENALTY)
}

/// The leftmost assembly at or after `from`, written into `into`.
fn forward(text: &Folded, wanted: &[char], from: usize, into: &mut [usize]) -> bool {
	let mut at = from;
	for (index, &point) in wanted.iter().enumerate() {
		while at < text.len() && text.points[at] != Some(point) {
			at += 1;
		}
		if at >= text.len() {
			return false;
		}
		into[index] = at;
		at += 1;
	}
	true
}

/// The rightmost assembly ending at `end`, written into `into`.
///
/// Cannot run off the front: it is only ever called with an `end` a forward pass
/// has just reached, so an assembly within `[0, end]` is known to exist — which
/// is what makes every `at -= 1` below safe.
fn backward(text: &Folded, wanted: &[char], end: usize, into: &mut [usize]) {
	let mut at = end;
	for index in (0..wanted.len()).rev() {
		let point = wanted[index];
		while text.points[at] != Some(point) {
			at -= 1;
		}
		into[index] = at;
		if index == 0 {
			break;
		}
		at -= 1;
	}
}
