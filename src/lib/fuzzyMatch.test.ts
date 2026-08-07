import { describe, expect, it } from 'vite-plus/test'

import { fuzzyMatch, fuzzyNeedle } from './fuzzyMatch'

/** The characters a match actually landed on, which is what the highlighter
 *  paints — asserting on indices alone would pass for an off-by-one. */
function matched(haystack: string, query: string): string | null {
	const found = fuzzyMatch(haystack, fuzzyNeedle(query))
	if (!found) return null
	return found.spans.map((span) => haystack.slice(span.start, span.end)).join('')
}

/** Where it landed, as code-unit starts. */
function at(haystack: string, query: string): number[] | null {
	const found = fuzzyMatch(haystack, fuzzyNeedle(query))
	return found?.spans.map((span) => span.start) ?? null
}

function score(haystack: string, query: string): number {
	const found = fuzzyMatch(haystack, fuzzyNeedle(query))
	if (!found) throw new Error(`${query} did not match ${haystack}`)
	return found.score
}

describe('fuzzyNeedle', () => {
	it('strips whitespace and folds case, so a query is one character sequence', () => {
		expect(fuzzyNeedle('  HTTP  Req ')).toBe('httpreq')
		expect(fuzzyNeedle('a b c')).toBe('abc')
		expect(fuzzyNeedle('   ')).toBe('')
	})
})

describe('fuzzyMatch', () => {
	it('matches a contiguous substring on the characters it names', () => {
		expect(matched('Send HTTP requests', 'http')).toBe('HTTP')
	})

	it('matches a subsequence spread across words, which substring search cannot', () => {
		// The example from the specification: the words are not adjacent and the
		// query is not a phrase.
		expect(matched('Send HTTP requests to the API', 'http req')).toBe('HTTPreq')
		expect(matched('albert brown carrot', 'a b c')).toBe('abc')
		expect(matched('abc', 'a b c')).toBe('abc')
	})

	it('is case-insensitive but reports spans in the original text', () => {
		// `İ` folds to two code units, so any offset taken from a wholesale-lowercased
		// copy is shifted by one and points at `tanb`.
		expect(at('İstanbul', 'stan')).toEqual([1, 2, 3, 4])
		expect(matched('İstanbul', 'stan')).toBe('stan')
	})

	it('refuses characters that are absent or out of order', () => {
		expect(fuzzyMatch('albert brown carrot', fuzzyNeedle('abz'))).toBeNull()
		expect(fuzzyMatch('abc', fuzzyNeedle('cba'))).toBeNull()
		// A needle longer than the text cannot be a subsequence of it.
		expect(fuzzyMatch('ab', fuzzyNeedle('abc'))).toBeNull()
	})

	it('matches nothing on an empty needle rather than everything', () => {
		// "No query" is a separate state in every caller; answering it here would
		// make an empty field look like a search that matched every note.
		expect(fuzzyMatch('anything at all', '')).toBeNull()
	})

	it('returns ascending, non-overlapping spans, one per needle character', () => {
		const found = fuzzyMatch('a quick brown fox', fuzzyNeedle('abf'))
		expect(found?.spans).toHaveLength(3)
		const ordered = found!.spans.every(
			(span, index) =>
				span.end > span.start && (index === 0 || span.start >= found!.spans[index - 1]!.end),
		)
		expect(ordered).toBe(true)
	})

	describe('ranking', () => {
		it('puts a consecutive run above word-boundary starts above a scattered match', () => {
			// The three shapes the same query can take, in the order the design asks
			// for. Asserted as one chain so the relationship is the test rather than
			// three thresholds that could each drift.
			const run = score('abc definitely', 'abc')
			const boundaries = score('albert brown carrot', 'abc')
			const scattered = score('axxbxxc', 'abc')

			expect(run).toBeGreaterThan(boundaries)
			expect(boundaries).toBeGreaterThan(scattered)
		})

		it('prefers a match that starts earlier', () => {
			expect(score('abc trailing text', 'abc')).toBeGreaterThan(
				score('a long run-up before abc', 'abc'),
			)
		})

		it('stops paying attention to how late a very late match is', () => {
			// The leading penalty is capped, so a note's *length* cannot decide its
			// rank: two matches that are both plainly "not at the beginning" score the
			// same rather than one being punished for the prose in front of it.
			const near = score(`${'x'.repeat(40)}abc`, 'abc')
			const far = score(`${'x'.repeat(4000)}abc`, 'abc')
			expect(near).toBe(far)
		})

		it('keeps punishing a wider gap inside the match, however wide', () => {
			// The opposite rule to the one above, and deliberately so: a match spread
			// over half a note really is worse than a tight one.
			expect(score('a-b-c', 'abc')).toBeGreaterThan(score('a---b---c', 'abc'))
		})

		it('rewards a camelCase hump, but less than a real word start', () => {
			const separated = score('send request', 'sr')
			const hump = score('sendRequest', 'sr')
			const neither = score('assorted rubbish', 'sr')

			expect(separated).toBeGreaterThan(hump)
			expect(hump).toBeGreaterThan(neither)
		})
	})

	describe('choosing among the possible matches', () => {
		it('slides the match right so a contiguous run is found rather than the leftmost one', () => {
			// A greedy left-to-right pass alone returns `a`(0) `b`(2) `c`(6) here, and
			// the consecutive bonus would then almost never fire.
			expect(at('a-b-abc', 'abc')).toEqual([4, 5, 6])
		})

		it('keeps the leftmost assembly when sliding right would cost a word boundary', () => {
			// Sliding is usually the improvement and is not always: here it moves `s`
			// off the start of a word and onto the middle of one, and a boundary bonus
			// outweighs the gap it closes. Scoring only the slid form loses this.
			expect(at('assess results', 'sr')).toEqual([1, 7])
		})

		it('takes the earliest of two equally good matches', () => {
			// Stability: the same text and query must always paint the same characters,
			// or a re-render moves the highlight for no reason.
			expect(at('abc abc', 'abc')).toEqual([0, 1, 2])
		})

		it('gives up on a text whose remaining characters cannot spell the needle', () => {
			expect(fuzzyMatch('aaaaaaaaaaaaaaaaaaaaab', fuzzyNeedle('abc'))).toBeNull()
		})

		/**
		 * The defect that retired the bounded-anchor scan, at the length the review
		 * measured it: a body of ordinary prose whose letters happen to spell the
		 * query long before the word itself appears. A scan that gave up after a fixed
		 * number of anchors ranked this note on the scatter *and painted the scatter*.
		 */
		it('finds a verbatim word two hundred characters into realistic prose', () => {
			const body =
				'The panel refuses to reveal itself when every monitor reported by the ' +
				'operating system has been unplugged, and the saved position no longer ' +
				'names anywhere a person could reach it. A capture that fails now shows an ' +
				'error notice instead of failing silently.'
			const verbatim = body.indexOf('error')
			// Comfortably past any bounded window of leading anchors.
			expect(verbatim).toBeGreaterThan(200)

			expect(at(body, 'error')).toEqual([
				verbatim,
				verbatim + 1,
				verbatim + 2,
				verbatim + 3,
				verbatim + 4,
			])
			expect(matched(body, 'error')).toBe('error')
		})

		it('anchors on a later start when that is where the real match is', () => {
			expect(at('a lot of words in between here abc', 'abc')).toEqual([31, 32, 33])
		})
	})

	describe('code points', () => {
		it('never assembles a match out of halves of different characters', () => {
			// `🍎` is U+1F34E — the surrogate pair D83C DF4E — and `🍬` is U+1F36C, the
			// pair D83C DF6C. They share a leading unit, so a code-unit matcher happily
			// spells one out of the other's halves. Nothing here may.
			expect(fuzzyMatch('🍎🍬', '🍬🍎')).toBeNull()
			// And the needle's own halves are one character, not two to find.
			expect(at('🍎x🍎', '🍎')).toEqual([0])
		})

		it('matches an astral character as itself, spanning both of its code units', () => {
			const found = fuzzyMatch('a 🍎 b', fuzzyNeedle('🍎'))
			expect(found?.spans).toEqual([{ start: 2, end: 4 }])
		})

		it('reports spans past an astral character at the right offsets', () => {
			// The offset map earns its place here: the code-point index and the
			// code-unit offset have diverged by one before `bc` is reached.
			expect(at('🍎abc', 'bc')).toEqual([3, 4])
		})

		it('folds the whole string, so a Greek final sigma matches its lowercase form', () => {
			// `'Σ'.toLowerCase()` is `σ` — folding one character at a time cannot know
			// it ends a word. Folding the string once can.
			expect(matched('ΟΔΟΣ', 'οδος')).toBe('ΟΔΟΣ')
		})

		it('leaves a character whose fold is several characters unmatched rather than misplaced', () => {
			// `İ` folds to `i` plus a combining dot. Painting it for a query of `i`
			// would put a span over one code unit of a two-unit fold.
			expect(fuzzyMatch('İ', fuzzyNeedle('i'))).toBeNull()
		})
	})

	describe('cost', () => {
		/**
		 * The shape rather than a wall-clock target: two hundred bodies is what a
		 * keystroke actually costs, and the bounded-anchor scan this replaced did the
		 * same work up to eight times over with two string allocations per character
		 * compared. A budget this loose fails only on a return to that, or on a
		 * genuinely quadratic scan.
		 */
		it('scans a keystroke’s worth of notes without going quadratic', () => {
			const bodies = Array.from(
				{ length: 200 },
				(_, index) =>
					`note ${index} ${'the quick brown fox jumps over the lazy dog and reports an error '.repeat(30)}`,
			)

			const started = performance.now()
			let matches = 0
			for (const body of bodies) if (fuzzyMatch(body, fuzzyNeedle('error'))) matches++
			const elapsed = performance.now() - started

			expect(matches).toBe(200)
			expect(elapsed).toBeLessThan(500)
		})

		it('answers a text that is almost entirely needle characters', () => {
			// The case the old cap existed to bound. Every position anchors a window,
			// so this is where a scan with no cap has to be linear rather than lucky.
			expect(fuzzyMatch('a'.repeat(20_000), fuzzyNeedle('aaa'))).not.toBeNull()
			expect(fuzzyMatch(`${'a '.repeat(5000)}bc`, fuzzyNeedle('abc'))).not.toBeNull()
		})
	})
})
