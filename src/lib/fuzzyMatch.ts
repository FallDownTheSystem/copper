/**
 * fzf-style subsequence matching: does this text contain the query's characters
 * in order, and how good is the match.
 *
 * **The query is a character sequence, not a list of words.** Whitespace is
 * stripped before matching, so `http req` matches "Send **HTTP** **req**uests"
 * and also "**h**y**p**er**t**ext" — the second is a genuine match, it simply
 * scores far below the first. That is the whole point of scoring rather than
 * filtering: a subsequence matcher on its own says yes far too often, and the
 * ranking is what makes the answer useful.
 *
 * **Positions are indices into the string that was passed in**, which is what
 * lets the same function serve both callers: `useNoteSearch` scores a note's
 * `body`, and `searchHighlight` runs it again over the *rendered* text to decide
 * which characters to paint. Those are two different strings — markdown syntax is
 * stripped, code is tokenised — and there is no mapping between them, so the
 * ranking score and the painted characters come from separate runs by
 * construction rather than by oversight.
 *
 * Case folding is per character and by `toLowerCase()`, so an offset is always an
 * index into the original string. A character whose fold is longer than itself
 * (`İ`, `ẞ`) simply fails to equal a single-character needle and goes unmatched —
 * the safe direction, since the alternative is a position pointing at the wrong
 * character.
 */

export type FuzzyMatch = {
	score: number
	/** Ascending indices into the haystack, one per needle character. */
	positions: number[]
}

/** A matched character, before any bonus. */
const SCORE_CHAR = 16
/** Immediately after the previous match — what makes a contiguous run win. */
const BONUS_CONSECUTIVE = 8
/** First character, or the first after a separator. */
const BONUS_BOUNDARY = 12
/** A `camelCase` hump: a word start with no separator in front of it. */
const BONUS_CAMEL = 8
/** Per character skipped inside the match, uncapped — a match spread over half a
 *  note really is worse than a tight one, however far apart the two ends are. */
const PENALTY_GAP = 2
/** Per character skipped before the first match. */
const PENALTY_LEADING = 3
/**
 * ...but capped, unlike the gap penalty. Beyond a few characters "the match
 * starts late" stops carrying information: a hit at offset 400 and one at offset
 * 4,000 are equally "not at the beginning", and leaving this uncapped would let
 * a note's *length* decide its rank.
 */
const MAX_LEADING_PENALTY = 15

/**
 * How many starting positions are tried before settling for the best so far.
 *
 * The best match is not always the leftmost one — `abc` in `a…b…(a lot)…abc`
 * scores far better anchored on the second `a` — so a single greedy pass from
 * offset zero is not enough. Trying *every* start is, but it makes the cost
 * quadratic in a body whose first needle character is common, and this runs for
 * every note on every keystroke with no debounce in front of it. Eight is
 * comfortably enough to find the cluster in ordinary prose and keeps the work
 * linear-ish.
 */
const MAX_STARTS = 8

const WORD = /[\p{L}\p{N}]/u

/**
 * The needle a query becomes: whitespace stripped, folded once.
 *
 * Exported because both callers need the *same* needle — one to score with, one
 * to paint with — and deriving it twice is how the two would drift apart.
 */
export function fuzzyNeedle(query: string): string {
	return query.replace(/\s+/gu, '').toLowerCase()
}

function boundaryBonus(haystack: string, at: number): number {
	if (at === 0) return BONUS_BOUNDARY
	const before = haystack.charAt(at - 1)
	if (!WORD.test(before)) return BONUS_BOUNDARY
	const here = haystack.charAt(at)
	// A hump rather than a separator: `Request` inside `sendRequest`. Worth less
	// than a real word start, because the writer did not put a break there.
	if (before === before.toLowerCase() && here !== here.toLowerCase()) return BONUS_CAMEL
	return 0
}

/** The leftmost positions for `needle` at or after `from`, or null. */
function greedy(haystack: string, needle: string, from: number): number[] | null {
	const positions: number[] = []
	let at = from
	for (let index = 0; index < needle.length; index++) {
		const wanted = needle.charAt(index)
		while (at < haystack.length && haystack.charAt(at).toLowerCase() !== wanted) at++
		if (at >= haystack.length) return null
		positions.push(at)
		at++
	}
	return positions
}

/**
 * Slides every character as far right as it will go without passing the one
 * after it, keeping the end fixed.
 *
 * The greedy pass finds the *earliest* end, which is what makes it cheap, but it
 * leaves the characters before that end as far left as possible — so `abc` in
 * `a-b-abc` comes back as `a`(0) `b`(2) `c`(6) rather than as the contiguous run
 * ending in the same place. Without this the consecutive bonus would almost never
 * fire.
 */
function tighten(haystack: string, needle: string, positions: number[]): number[] {
	const tightened = positions.slice()
	for (let index = needle.length - 2; index >= 0; index--) {
		const wanted = needle.charAt(index)
		const floor = positions[index] as number
		let at = (tightened[index + 1] as number) - 1
		// `floor` is known to match, so this cannot run off the front.
		while (at > floor && haystack.charAt(at).toLowerCase() !== wanted) at--
		tightened[index] = at
	}
	return tightened
}

function scoreOf(haystack: string, positions: number[]): number {
	let total = 0
	let previous = -1

	for (const at of positions) {
		total += SCORE_CHAR
		if (previous >= 0) {
			if (at === previous + 1) total += BONUS_CONSECUTIVE
			else total -= PENALTY_GAP * (at - previous - 1)
		}
		total += boundaryBonus(haystack, at)
		previous = at
	}

	const first = positions[0] ?? 0
	return total - Math.min(PENALTY_LEADING * first, MAX_LEADING_PENALTY)
}

/**
 * The best match of `needle` in `haystack`, or null when the characters do not
 * appear in order.
 *
 * `needle` must already have been through [`fuzzyNeedle`]. An empty needle
 * matches nothing rather than everything: the callers treat "no query" as a
 * separate state, and answering it here would make an empty field look like a
 * search that matched every note with a score of zero.
 */
export function fuzzyMatch(haystack: string, needle: string): FuzzyMatch | null {
	if (needle.length === 0 || haystack.length < needle.length) return null

	const wanted = needle.charAt(0)
	let best: FuzzyMatch | null = null
	let starts = 0

	for (let at = 0; at <= haystack.length - needle.length; at++) {
		if (haystack.charAt(at).toLowerCase() !== wanted) continue

		const found = greedy(haystack, needle, at)
		// Nothing later can succeed either: a greedy pass from further right has
		// strictly fewer characters to work with.
		if (!found) break

		const positions = tighten(haystack, needle, found)
		const score = scoreOf(haystack, positions)
		// Strictly greater, so the earliest of two equal matches wins and the result
		// is stable against a haystack that repeats itself.
		if (!best || score > best.score) best = { score, positions }

		if (++starts >= MAX_STARTS) break
	}

	return best
}
