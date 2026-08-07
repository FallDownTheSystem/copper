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
 * **Spans are code-unit ranges into the string that was passed in**, which is
 * what lets the same function serve both callers: `useNoteSearch` scores a note's
 * `body`, and `searchHighlight` runs it again over the *rendered* text to decide
 * which characters to paint. Those are two different strings — markdown syntax is
 * stripped, code is tokenised — and there is no mapping between them, so the
 * ranking score and the painted characters come from separate runs by
 * construction rather than by oversight.
 *
 * ### The scan, and why it is not a bounded number of starts
 *
 * The best match is rarely the leftmost one: in ordinary prose the letters of
 * `error` can be assembled from five scattered words long before the word itself
 * appears, and a scan that gives up after a fixed number of anchors never reaches
 * the real match — it ranks the note on the scatter and paints the scatter too.
 * Raising the cap only moves the body length at which that happens.
 *
 * So every *maximal-tight window* is enumerated instead, fzf-v1 style. One
 * forward pass finds the earliest position the needle can finish at; one backward
 * pass from there finds the latest position it can start at; that window is
 * scored, and the scan resumes one code point after its start. Each round strictly
 * advances the start, so the loop terminates, and the total work is the classic
 * minimum-window-subsequence bound: linear in the text for anything prose-shaped,
 * O(text × query) only for a text that is almost entirely needle characters — and
 * queries here are a handful of characters long.
 *
 * Both assemblies inside a window are scored and the better kept. Sliding
 * everything right is usually the improvement, since it is what finds contiguous
 * runs, but not always: it can also slide a character off the word boundary it
 * had landed on, and a boundary bonus is worth more than the gap it closes.
 *
 * ### Case folding
 *
 * Folding happens once per string into an index-aligned array of code points,
 * rather than per comparison. That is a correctness change as much as a
 * performance one:
 *
 * - **Whole code points, never code units.** A query character can never match
 *   one half of a surrogate pair, and an astral character matches itself.
 * - **Whole strings where it is safe.** `Σ` at the end of a word lowercases to
 *   `ς`, which `'Σ'.toLowerCase()` on its own cannot know. A text with no
 *   length-changing fold in it is therefore folded as one string.
 * - **Conservative where it is not.** A code point whose fold is not a single
 *   code point — `İ` becomes `i` plus a combining dot — is marked unmatchable
 *   rather than approximated. It goes unfound, which is the safe direction: the
 *   alternative is a span over the wrong characters.
 *
 * **No Unicode normalization.** A decomposed `é` (`e` + U+0301) and a composed
 * one are different sequences here and do not match each other. NFC would need
 * its own offset map back to the source, and every string in this app comes from
 * the same editor and clipboard as the query.
 */

export type MatchSpan = {
	/** Code-unit index into the haystack. */
	start: number
	/** Exclusive, so `end - start` is 2 for an astral character. */
	end: number
}

export type FuzzyMatch = {
	score: number
	/** Ascending and non-overlapping, one per code point of the needle. */
	spans: MatchSpan[]
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

/** A code point whose fold is not a single code point, and so can never equal
 *  one needle code point. Outside the code-point range, so it collides with
 *  nothing real. */
const UNMATCHABLE = -1

const WORD = /[\p{L}\p{N}]/u
/** One test decides whether the string can hold an astral character at all,
 *  which is what lets the offset map be skipped for every ordinary body. */
const HIGH_SURROGATE = /[\uD800-\uDBFF]/

/**
 * A string as folded code points, aligned to it.
 *
 * `offsets` is `null` for a text of only single-unit code points — which is
 * every note body in practice — because then the code-point index *is* the
 * code-unit offset and the array would be an allocation that answers `k` with
 * `k`.
 */
type Folded = {
	source: string
	/** One entry per code point, folded, or [`UNMATCHABLE`]. */
	points: Int32Array
	/** Code-unit index of each code point, with a final sentinel so the end of
	 *  code point `k` is `offsets[k + 1]`. */
	offsets: Int32Array | null
	/** Code points, which is not `source.length` when there are surrogate pairs. */
	length: number
}

function startOf(text: Folded, at: number): number {
	return text.offsets ? (text.offsets[at] as number) : at
}

function endOf(text: Folded, at: number): number {
	return text.offsets ? (text.offsets[at + 1] as number) : at + 1
}

/**
 * One code point's fold, as a code point.
 *
 * The ASCII branch is the whole reason this returns a number: `charAt(i)
 * .toLowerCase()` allocates twice per character compared, and at two hundred
 * notes a keystroke that allocation *is* the cost of a search.
 */
function foldPoint(point: number): number {
	if (point < 0x80) return point >= 0x41 && point <= 0x5a ? point + 0x20 : point

	const lower = String.fromCodePoint(point).toLowerCase()
	const folded = lower.codePointAt(0)
	if (folded === undefined) return UNMATCHABLE
	// More code units than the single code point would occupy means the fold
	// expanded into several characters.
	return lower.length === (folded > 0xffff ? 2 : 1) ? folded : UNMATCHABLE
}

function isWordPoint(point: number): boolean {
	if (point < 0x80) {
		return (
			(point >= 0x30 && point <= 0x39) ||
			(point >= 0x41 && point <= 0x5a) ||
			(point >= 0x61 && point <= 0x7a)
		)
	}
	return WORD.test(String.fromCodePoint(point))
}

function countPoints(text: string): number {
	let count = 0
	for (let at = 0; at < text.length;) {
		at += (text.codePointAt(at) as number) > 0xffff ? 2 : 1
		count++
	}
	return count
}

/**
 * Replaces the per-code-point folds with a whole-string one, which is the only
 * way the context-sensitive cases come out right.
 *
 * Only reached when no code point folded to something other than a single code
 * point, so the two forms have the same count and index `k` means the same
 * character in both. The count is verified anyway rather than assumed — an
 * engine that disagreed would otherwise shift every span by one.
 */
function refold(source: string, points: Int32Array, count: number) {
	const lowered = source.toLowerCase()
	if (countPoints(lowered) !== count) return

	let index = 0
	for (let at = 0; at < lowered.length;) {
		const point = lowered.codePointAt(at) as number
		points[index++] = point
		at += point > 0xffff ? 2 : 1
	}
}

/**
 * The fold's buffers, reused across calls.
 *
 * A keystroke folds every note in the space, and a fresh pair of arrays per note
 * is two hundred allocations of a few kilobytes each that live for microseconds.
 * Safe to share because [`fuzzyMatch`] is synchronous and its `Folded` never
 * escapes the call — `spansOf` runs before it returns. **Nothing in this module
 * may become async or re-entrant without giving these back.**
 */
let scratchPoints = new Int32Array(0)
let scratchOffsets = new Int32Array(0)

function scratch(
	units: number,
	astral: boolean,
): { points: Int32Array; offsets: Int32Array | null } {
	// `units` is an upper bound on the code-point count, so no growth step is
	// needed once the buffer is big enough for the longest body seen.
	if (scratchPoints.length < units) scratchPoints = new Int32Array(units)
	if (!astral) return { points: scratchPoints, offsets: null }
	if (scratchOffsets.length < units + 1) scratchOffsets = new Int32Array(units + 1)
	return { points: scratchPoints, offsets: scratchOffsets }
}

function fold(source: string): Folded {
	const astral = HIGH_SURROGATE.test(source)
	const { points, offsets } = scratch(source.length, astral)

	let count = 0
	let ascii = true
	let exact = true

	for (let at = 0; at < source.length;) {
		const point = source.codePointAt(at) as number
		if (offsets) offsets[count] = at

		const folded = foldPoint(point)
		if (folded === UNMATCHABLE) exact = false
		if (point >= 0x80) ascii = false
		points[count] = folded

		count++
		at += point > 0xffff ? 2 : 1
	}
	if (offsets) offsets[count] = source.length

	if (!ascii && exact) refold(source, points, count)

	return { source, points, offsets, length: count }
}

/**
 * The needle as code points, remembered for the one query in flight.
 *
 * A keystroke asks the same question of every note, so converting the needle per
 * note would be the one part of this that scales with the note count for no
 * reason. One entry rather than a cache: there is only ever one live query.
 */
let lastNeedle = ''
let lastPoints = new Int32Array(0)

function needlePoints(needle: string): Int32Array {
	if (needle === lastNeedle) return lastPoints

	const points = new Int32Array(needle.length)
	let count = 0
	for (let at = 0; at < needle.length;) {
		const point = needle.codePointAt(at) as number
		points[count++] = point
		at += point > 0xffff ? 2 : 1
	}

	lastNeedle = needle
	lastPoints = points.subarray(0, count)
	return lastPoints
}

/**
 * The needle a query becomes: whitespace stripped, folded once.
 *
 * Exported because both callers need the *same* needle — one to score with, one
 * to paint with — and deriving it twice is how the two would drift apart. Folded
 * as a whole string, so the query side gets the context-sensitive cases right
 * for free.
 */
export function fuzzyNeedle(query: string): string {
	return query.replace(/\s+/gu, '').toLowerCase()
}

function boundaryBonus(text: Folded, at: number): number {
	if (at === 0) return BONUS_BOUNDARY

	const before = text.source.codePointAt(startOf(text, at - 1)) as number
	if (!isWordPoint(before)) return BONUS_BOUNDARY

	// A hump rather than a separator: `Request` inside `sendRequest`. Worth less
	// than a real word start, because the writer did not put a break there. Read
	// off the fold rather than by re-lowercasing: a code point the fold left alone
	// is one that was not uppercase.
	const here = text.source.codePointAt(startOf(text, at)) as number
	if (text.points[at - 1] === before && text.points[at] !== here) return BONUS_CAMEL
	return 0
}

function scoreOf(text: Folded, positions: Int32Array): number {
	let total = 0
	let previous = -1

	for (const at of positions) {
		total += SCORE_CHAR
		if (previous >= 0) {
			if (at === previous + 1) total += BONUS_CONSECUTIVE
			else total -= PENALTY_GAP * (at - previous - 1)
		}
		total += boundaryBonus(text, at)
		previous = at
	}

	const first = positions[0] ?? 0
	return total - Math.min(PENALTY_LEADING * first, MAX_LEADING_PENALTY)
}

/** The leftmost assembly at or after `from`, written into `into`. */
function forward(text: Folded, wanted: Int32Array, from: number, into: Int32Array): boolean {
	let at = from
	for (let index = 0; index < wanted.length; index++) {
		const point = wanted[index] as number
		while (at < text.length && text.points[at] !== point) at++
		if (at >= text.length) return false
		into[index] = at
		at++
	}
	return true
}

/**
 * The rightmost assembly ending at `end`, written into `into`.
 *
 * Cannot run off the front: it is only ever called with an `end` a forward pass
 * has just reached, so an assembly within `[0, end]` is known to exist.
 */
function backward(text: Folded, wanted: Int32Array, end: number, into: Int32Array) {
	let at = end
	for (let index = wanted.length - 1; index >= 0; index--) {
		const point = wanted[index] as number
		while (text.points[at] !== point) at--
		into[index] = at
		at--
	}
}

function spansOf(text: Folded, positions: Int32Array): MatchSpan[] {
	const spans: MatchSpan[] = []
	for (const at of positions) spans.push({ start: startOf(text, at), end: endOf(text, at) })
	return spans
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

	const wanted = needlePoints(needle)
	const text = fold(haystack)
	if (text.length < wanted.length) return null

	const leftmost = new Int32Array(wanted.length)
	const rightmost = new Int32Array(wanted.length)
	const best = new Int32Array(wanted.length)
	let bestScore = 0
	let found = false
	let from = 0

	while (from + wanted.length <= text.length) {
		// Nothing later can succeed either: a pass from further right has strictly
		// fewer code points to work with.
		if (!forward(text, wanted, from, leftmost)) break
		backward(text, wanted, leftmost[wanted.length - 1] as number, rightmost)

		const leftScore = scoreOf(text, leftmost)
		const rightScore = scoreOf(text, rightmost)

		// **Inside one window a tie goes to the slid assembly**, which is the one
		// with the longer runs — the same match described in fewer pieces, and the
		// better thing to paint. `a-b-abc` is exactly that tie: the scattered
		// assembly's two boundary bonuses come to the same total as the contiguous
		// one's consecutive bonuses.
		const windowScore = Math.max(leftScore, rightScore)
		// **Across windows the comparison stays strict**, so the earliest of two
		// equally good matches wins and the same text and query always paint the same
		// characters however often the text repeats itself.
		if (!found || windowScore > bestScore) {
			found = true
			bestScore = windowScore
			best.set(rightScore >= leftScore ? rightmost : leftmost)
		}

		// The window's own start, which the backward pass has just proved is the
		// latest one for this end — so the next round considers a genuinely different
		// window, and the loop advances by at least one code point every time.
		from = (rightmost[0] as number) + 1
	}

	return found ? { score: bestScore, spans: spansOf(text, best) } : null
}
