import { describe, expect, it } from 'vite-plus/test'

import { fuzzyMatch, fuzzyNeedle } from './fuzzyMatch'

/** The characters a match actually landed on, which is what the highlighter
 *  paints — asserting on indices alone would pass for an off-by-one. */
function matched(haystack: string, query: string): string | null {
	const found = fuzzyMatch(haystack, fuzzyNeedle(query))
	if (!found) return null
	return found.positions.map((at) => haystack.charAt(at)).join('')
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

	it('is case-insensitive but reports positions in the original text', () => {
		const found = fuzzyMatch('İstanbul', fuzzyNeedle('stan'))
		// The regression the highlighter's own suite guards: `İ` folds to two code
		// units, so any offset taken from a wholesale-lowercased copy is shifted by
		// one and points at `tanb`.
		expect(found?.positions).toEqual([1, 2, 3, 4])
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

	it('returns strictly ascending positions, one per needle character', () => {
		const found = fuzzyMatch('a quick brown fox', fuzzyNeedle('abf'))
		expect(found?.positions).toHaveLength(3)
		const ascending = found!.positions.every(
			(at, index) => index === 0 || at > (found!.positions[index - 1] as number),
		)
		expect(ascending).toBe(true)
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

	describe('choosing among several possible matches', () => {
		it('slides the match right so a contiguous run is found rather than the leftmost one', () => {
			// A greedy left-to-right pass alone returns `a`(0) `b`(2) `c`(6) here, and
			// the consecutive bonus would then almost never fire.
			expect(fuzzyMatch('a-b-abc', fuzzyNeedle('abc'))?.positions).toEqual([4, 5, 6])
		})

		it('anchors on a later start when that is where the real match is', () => {
			const found = fuzzyMatch('a lot of words in between here abc', fuzzyNeedle('abc'))
			expect(found?.positions).toEqual([31, 32, 33])
		})

		it('takes the earliest of two equally good matches', () => {
			// Stability: the same text and query must always paint the same characters,
			// or a re-render moves the highlight for no reason.
			expect(fuzzyMatch('abc abc', fuzzyNeedle('abc'))?.positions).toEqual([0, 1, 2])
		})

		it('gives up on a text whose remaining characters cannot spell the needle', () => {
			// The early `break`: once a greedy pass from one start fails, every later
			// start has strictly less text to work with.
			expect(fuzzyMatch('aaaaaaaaaaaaaaaaaaaaab', fuzzyNeedle('abc'))).toBeNull()
		})

		it('stays cheap on a text whose first needle character is everywhere', () => {
			// The bound that keeps a keystroke from going quadratic. Not a timing
			// assertion — just that a pathological text still answers.
			const haystack = `${'a '.repeat(5000)}bc`
			expect(fuzzyMatch(haystack, fuzzyNeedle('abc'))).not.toBeNull()
		})
	})
})
