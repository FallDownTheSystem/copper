//! The `src/lib/fuzzyMatch.test.ts` corpus, ported one `#[test]` per `it(…)`,
//! plus the `search_notes` filter the CLI actually calls.
//!
//! Inputs and expectations are copied from the TypeScript file rather than
//! re-derived. The scoring constants and the window scan are real algorithm, and
//! the two tie-break rules in particular are the part of the port no type error
//! would catch: getting either backwards silently changes which characters a
//! query is considered to have matched.
//!
//! **Positions are char indices, not JavaScript code units.** For everything in
//! the Basic Multilingual Plane the two numbers are identical, so the TypeScript's
//! expectations port across unchanged. They differ only past an astral character,
//! which JavaScript counts as two and Rust as one — the two cases where that
//! happens say so.

use copper_core::search::{fuzzy_match, fuzzy_needle, search_notes};
use copper_core::store::format;
use copper_core::store::model::{Note, Space};

/// The characters a match actually landed on, which is what a highlighter would
/// paint — asserting on indices alone would pass for an off-by-one.
fn matched(haystack: &str, query: &str) -> Option<String> {
	let found = fuzzy_match(haystack, &fuzzy_needle(query))?;
	Some(
		found
			.starts
			.iter()
			.map(|&start| haystack[start..].chars().next().expect("a start is a char boundary"))
			.collect(),
	)
}

/// Where it landed, as char indices.
fn at(haystack: &str, query: &str) -> Option<Vec<usize>> {
	let found = fuzzy_match(haystack, &fuzzy_needle(query))?;
	Some(
		found
			.starts
			.iter()
			.map(|&byte| haystack[..byte].chars().count())
			.collect(),
	)
}

fn score(haystack: &str, query: &str) -> i32 {
	fuzzy_match(haystack, &fuzzy_needle(query))
		.unwrap_or_else(|| panic!("{query} did not match {haystack}"))
		.score
}

fn misses(haystack: &str, query: &str) -> bool {
	fuzzy_match(haystack, &fuzzy_needle(query)).is_none()
}

// --- fuzzyNeedle --------------------------------------------------------------

#[test]
fn strips_whitespace_and_folds_case_so_a_query_is_one_character_sequence() {
	assert_eq!(fuzzy_needle("  HTTP  Req "), "httpreq");
	assert_eq!(fuzzy_needle("a b c"), "abc");
	assert_eq!(fuzzy_needle("   "), "");
}

// --- fuzzyMatch ---------------------------------------------------------------

#[test]
fn matches_a_contiguous_substring_on_the_characters_it_names() {
	assert_eq!(matched("Send HTTP requests", "http").as_deref(), Some("HTTP"));
}

#[test]
fn matches_a_subsequence_spread_across_words() {
	// The example from the specification: the words are not adjacent and the
	// query is not a phrase.
	assert_eq!(
		matched("Send HTTP requests to the API", "http req").as_deref(),
		Some("HTTPreq")
	);
	assert_eq!(matched("albert brown carrot", "a b c").as_deref(), Some("abc"));
	assert_eq!(matched("abc", "a b c").as_deref(), Some("abc"));
}

#[test]
fn is_case_insensitive_but_reports_positions_in_the_original_text() {
	// `İ` folds to two characters, so any offset taken from a wholesale-lowercased
	// copy is shifted by one and points at `tanb`.
	assert_eq!(at("İstanbul", "stan"), Some(vec![1, 2, 3, 4]));
	assert_eq!(matched("İstanbul", "stan").as_deref(), Some("stan"));
}

#[test]
fn refuses_characters_that_are_absent_or_out_of_order() {
	assert!(misses("albert brown carrot", "abz"));
	assert!(misses("abc", "cba"));
	// A needle longer than the text cannot be a subsequence of it.
	assert!(misses("ab", "abc"));
}

#[test]
fn matches_nothing_on_an_empty_needle_rather_than_everything() {
	// "No query" is a separate state in every caller; answering it here would make
	// an empty field look like a search that matched every note.
	assert!(fuzzy_match("anything at all", "").is_none());
}

#[test]
fn returns_ascending_non_overlapping_positions_one_per_needle_character() {
	let found = fuzzy_match("a quick brown fox", &fuzzy_needle("abf")).expect("a match");
	assert_eq!(found.starts.len(), 3);
	assert!(
		found.starts.windows(2).all(|pair| pair[0] < pair[1]),
		"{:?}",
		found.starts
	);
}

// --- ranking -------------------------------------------------------------------

#[test]
fn puts_a_consecutive_run_above_word_boundary_starts_above_a_scattered_match() {
	// The three shapes the same query can take, in the order the design asks for.
	// Asserted as one chain so the relationship is the test rather than three
	// thresholds that could each drift.
	let run = score("abc definitely", "abc");
	let boundaries = score("albert brown carrot", "abc");
	let scattered = score("axxbxxc", "abc");

	assert!(run > boundaries, "{run} vs {boundaries}");
	assert!(boundaries > scattered, "{boundaries} vs {scattered}");
}

#[test]
fn prefers_a_match_that_starts_earlier() {
	assert!(score("abc trailing text", "abc") > score("a long run-up before abc", "abc"));
}

#[test]
fn stops_paying_attention_to_how_late_a_very_late_match_is() {
	// The leading penalty is capped, so a note's *length* cannot decide its rank:
	// two matches that are both plainly "not at the beginning" score the same
	// rather than one being punished for the prose in front of it.
	let near = score(&format!("{}abc", "x".repeat(40)), "abc");
	let far = score(&format!("{}abc", "x".repeat(4000)), "abc");
	assert_eq!(near, far);
}

#[test]
fn keeps_punishing_a_wider_gap_inside_the_match_however_wide() {
	// The opposite rule to the one above, and deliberately so: a match spread over
	// half a note really is worse than a tight one.
	assert!(score("a-b-c", "abc") > score("a---b---c", "abc"));
}

#[test]
fn rewards_a_camel_case_hump_but_less_than_a_real_word_start() {
	let separated = score("send request", "sr");
	let hump = score("sendRequest", "sr");
	let neither = score("assorted rubbish", "sr");

	assert!(separated > hump, "{separated} vs {hump}");
	assert!(hump > neither, "{hump} vs {neither}");
}

// --- choosing among the possible matches ---------------------------------------

#[test]
fn slides_the_match_right_so_a_contiguous_run_is_found_rather_than_the_leftmost() {
	// A greedy left-to-right pass alone returns `a`(0) `b`(2) `c`(6) here, and the
	// consecutive bonus would then almost never fire. This is the within-window
	// tie: the scattered assembly's two boundary bonuses come to exactly the
	// contiguous one's consecutive bonuses, and the tie has to go to the slid form.
	assert_eq!(at("a-b-abc", "abc"), Some(vec![4, 5, 6]));
}

#[test]
fn keeps_the_leftmost_assembly_when_sliding_right_would_cost_a_word_boundary() {
	// Sliding is usually the improvement and is not always: here it moves `s` off
	// the start of a word and onto the middle of one, and a boundary bonus
	// outweighs the gap it closes. Scoring only the slid form loses this.
	assert_eq!(at("assess results", "sr"), Some(vec![1, 7]));
}

#[test]
fn takes_the_earliest_of_two_equally_good_matches() {
	// Stability: the same text and query must always answer the same, or a
	// re-render moves the highlight for no reason. This is the across-window
	// comparison, and it stays strict.
	assert_eq!(at("abc abc", "abc"), Some(vec![0, 1, 2]));
}

#[test]
fn gives_up_on_a_text_whose_remaining_characters_cannot_spell_the_needle() {
	assert!(misses("aaaaaaaaaaaaaaaaaaaaab", "abc"));
}

/// The defect that retired the bounded-anchor scan, at the length the review
/// measured it: a body of ordinary prose whose letters happen to spell the query
/// long before the word itself appears. A scan that gave up after a fixed number
/// of anchors ranked this note on the scatter.
#[test]
fn finds_a_verbatim_word_two_hundred_characters_into_realistic_prose() {
	let body = "The panel refuses to reveal itself when every monitor reported by the \
	            operating system has been unplugged, and the saved position no longer \
	            names anywhere a person could reach it. A capture that fails now shows an \
	            error notice instead of failing silently.";
	let verbatim = body.find("error").expect("the word is in the body");
	// Comfortably past any bounded window of leading anchors.
	assert!(verbatim > 200, "{verbatim}");

	assert_eq!(
		at(body, "error"),
		Some(vec![
			verbatim,
			verbatim + 1,
			verbatim + 2,
			verbatim + 3,
			verbatim + 4
		])
	);
	assert_eq!(matched(body, "error").as_deref(), Some("error"));
}

#[test]
fn anchors_on_a_later_start_when_that_is_where_the_real_match_is() {
	assert_eq!(at("a lot of words in between here abc", "abc"), Some(vec![31, 32, 33]));
}

// --- code points ----------------------------------------------------------------

#[test]
fn never_assembles_a_match_out_of_halves_of_different_characters() {
	// `🍎` is U+1F34E and `🍬` is U+1F36C. In JavaScript they share a leading
	// surrogate, so a code-unit matcher spells one out of the other's halves. Rust
	// has no halves to spell from — this pins that the port did not reintroduce
	// the problem by matching bytes.
	assert!(fuzzy_match("🍎🍬", "🍬🍎").is_none());
	assert_eq!(at("🍎x🍎", "🍎"), Some(vec![0]));
}

#[test]
fn matches_an_astral_character_as_itself() {
	// The TypeScript asserts a span of `{ start: 2, end: 4 }` — two code units.
	// Here the character is one char at char index 2, and four bytes wide.
	let found = fuzzy_match("a 🍎 b", &fuzzy_needle("🍎")).expect("a match");
	assert_eq!(found.starts, vec![2]);
	assert_eq!(at("a 🍎 b", "🍎"), Some(vec![2]));
}

#[test]
fn reports_positions_past_an_astral_character_at_the_right_offsets() {
	// The one place the ported number changes: JavaScript counts `🍎` as two code
	// units and expects `[3, 4]`; `b` and `c` are at char indices 2 and 3 here.
	assert_eq!(at("🍎abc", "bc"), Some(vec![2, 3]));
	// And as byte offsets, past the emoji's four bytes.
	let found = fuzzy_match("🍎abc", &fuzzy_needle("bc")).expect("a match");
	assert_eq!(found.starts, vec![5, 6]);
}

#[test]
fn folds_the_whole_string_so_a_greek_final_sigma_matches_its_lowercase_form() {
	// `'Σ'.to_lowercase()` on its own is `σ` — folding one character at a time
	// cannot know it ends a word. Folding the string once can, and Rust's
	// `str::to_lowercase` implements the same special-casing table JavaScript's
	// `toLowerCase` does. This is the one behaviour of the port that is not
	// visible by reading it.
	assert_eq!(matched("ΟΔΟΣ", "οδος").as_deref(), Some("ΟΔΟΣ"));
	assert_eq!("ΟΔΟΣ".to_lowercase(), "οδος", "the final sigma rule is what carries this");
}

#[test]
fn leaves_a_character_whose_fold_is_several_characters_unmatched() {
	// `İ` folds to `i` plus a combining dot. Treating it as `i` would report a
	// match over one half of a two-character fold.
	assert!(misses("İ", "i"));
}

// --- cost ------------------------------------------------------------------------

/// The shape rather than a wall-clock target: two hundred bodies is what a
/// keystroke costs, and a budget this loose fails only on a genuinely quadratic
/// scan.
#[test]
fn scans_a_keystrokes_worth_of_notes_without_going_quadratic() {
	let bodies: Vec<String> = (0..200)
		.map(|index| {
			format!(
				"note {index} {}",
				"the quick brown fox jumps over the lazy dog and reports an error ".repeat(30)
			)
		})
		.collect();

	let needle = fuzzy_needle("error");
	let started = std::time::Instant::now();
	let matches = bodies
		.iter()
		.filter(|body| fuzzy_match(body, &needle).is_some())
		.count();
	let elapsed = started.elapsed();

	assert_eq!(matches, 200);
	assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
}

#[test]
fn answers_a_text_that_is_almost_entirely_needle_characters() {
	// The case a bounded-anchor scan existed to bound. Every position anchors a
	// window, so this is where a scan with no cap has to be linear rather than
	// lucky.
	assert!(fuzzy_match(&"a".repeat(20_000), &fuzzy_needle("aaa")).is_some());
	assert!(fuzzy_match(&format!("{}bc", "a ".repeat(5000)), &fuzzy_needle("abc")).is_some());
}

// --- search_notes ----------------------------------------------------------------

/// A two-section document whose note order is *not* the order any ranking would
/// produce, so "canonical document order" is falsifiable.
fn space() -> Space {
	format::parse_normalised(
		r#"{
  "id": "spc_00000001",
  "name": "work",
  "activeSection": "sec_00000001",
  "sections": [
    { "id": "sec_00000001", "name": "Notes", "order": 0 },
    { "id": "sec_00000002", "name": "Later", "order": 1 }
  ],
  "notes": [
    { "id": "nte_00000001", "section": "sec_00000001", "order": 0, "done": true,
      "body": "a plain ERROR, shouted", "created": "2026-01-01T00:00:00Z",
      "updated": "2026-01-01T00:00:00Z" },
    { "id": "nte_00000002", "section": "sec_00000001", "order": 1, "done": false,
      "body": "a scattered e r r o r spelled out", "created": "2026-01-01T00:00:00Z",
      "updated": "2026-01-01T00:00:00Z" },
    { "id": "nte_00000003", "section": "sec_00000002", "order": 0, "done": false,
      "body": "nothing to see", "created": "2026-01-01T00:00:00Z",
      "updated": "2026-01-01T00:00:00Z" }
  ]
}
"#,
	)
	.expect("the fixture parses")
}

fn ids(found: Vec<&Note>) -> Vec<&str> {
	found.iter().map(|note| note.id.as_str()).collect()
}

/// The retracted design ranked these.
///
/// The fixture is built so the two orders disagree: the scattered note scores
/// *higher* than the verbatim one — five separate word-boundary bonuses beat one
/// contiguous run — and it is second in the document. A score sort would
/// therefore return them the other way round, which is what makes this
/// falsifiable rather than a coincidence.
#[test]
fn search_notes_returns_canonical_document_order_not_score_order() {
	let space = space();
	assert!(
		score("a plain ERROR, shouted", "error")
			< score("a scattered e r r o r spelled out", "error"),
		"the fixture no longer distinguishes the two orders"
	);
	assert_eq!(
		ids(search_notes(&space, "error", None, None, false)),
		["nte_00000001", "nte_00000002"]
	);
}

#[test]
fn exact_mode_is_a_plain_case_insensitive_substring_test() {
	let space = space();
	// The scattered note matches the subsequence and not the substring.
	assert_eq!(
		ids(search_notes(&space, "error", None, None, true)),
		["nte_00000001"]
	);
	assert_eq!(
		ids(search_notes(&space, "ErRoR", None, None, true)),
		["nte_00000001"],
		"case must not matter"
	);
}

#[test]
fn the_section_and_done_filters_apply_before_matching() {
	let space = space();
	assert_eq!(
		ids(search_notes(&space, "error", Some("sec_00000002"), None, false)),
		Vec::<&str>::new()
	);
	assert_eq!(
		ids(search_notes(&space, "error", None, Some(true), false)),
		["nte_00000001"]
	);
	assert_eq!(
		ids(search_notes(&space, "error", None, Some(false), false)),
		["nte_00000002"]
	);
}

/// Consistent across both modes, and deliberately not "everything": `--query ''`
/// selecting the whole space would be a surprising way to copy it.
#[test]
fn an_empty_query_matches_nothing_in_either_mode() {
	let space = space();
	assert!(search_notes(&space, "", None, None, false).is_empty());
	assert!(search_notes(&space, "", None, None, true).is_empty());
	assert!(
		search_notes(&space, "   ", None, None, false).is_empty(),
		"a whitespace-only query strips to an empty needle"
	);
}
