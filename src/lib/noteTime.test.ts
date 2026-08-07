import { describe, expect, it } from 'vite-plus/test'

import { formatCreated, parseCreated, sortByCreated } from './noteTime'

describe('parseCreated', () => {
	it('reads the RFC3339 the store writes', () => {
		expect(parseCreated('2026-07-30T14:02:11Z')).toBe(Date.UTC(2026, 6, 30, 14, 2, 11))
	})

	/** The store models `created` as a plain `String` precisely so a hand-edited
	 *  value cannot make the document unloadable — `format.rs` has a test loading a
	 *  note whose `created` is "yesterday afternoon". Everything downstream has to
	 *  survive that value reaching it. */
	it('answers null for a value that is not a timestamp', () => {
		expect(parseCreated('yesterday afternoon')).toBeNull()
		expect(parseCreated('')).toBeNull()
		expect(parseCreated(undefined)).toBeNull()
		expect(parseCreated(null)).toBeNull()
	})
})

describe('formatCreated', () => {
	it('renders a readable line for a real timestamp', () => {
		const shown = formatCreated('2026-07-30T14:02:11Z')
		// The exact text is the machine's locale and timezone, which the test must
		// not assume. What it must assert is that something was produced and that it
		// names the right day somewhere in it.
		expect(shown).toBeTruthy()
		expect(shown).toContain('2026')
	})

	/** AC20. Nothing is shown rather than a placeholder: a dash would claim the
	 *  note has no date, when the truth is that the one it has cannot be read. */
	it('renders nothing at all for an unreadable value', () => {
		expect(formatCreated('yesterday afternoon')).toBeNull()
		expect(formatCreated(undefined)).toBeNull()
	})
})

describe('sortByCreated', () => {
	const times: Record<string, string> = {
		oldest: '2026-01-01T00:00:00Z',
		middle: '2026-06-01T00:00:00Z',
		newest: '2026-12-01T00:00:00Z',
	}
	const at = (id: string) => parseCreated(times[id])

	it('orders ascending under oldest and descending under newest', () => {
		const ids = ['middle', 'newest', 'oldest']
		expect(sortByCreated(ids, at, 'oldest')).toEqual(['oldest', 'middle', 'newest'])
		expect(sortByCreated(ids, at, 'newest')).toEqual(['newest', 'middle', 'oldest'])
	})

	it('leaves the input array alone', () => {
		const ids = ['middle', 'newest', 'oldest']
		sortByCreated(ids, at, 'oldest')
		expect(ids).toEqual(['middle', 'newest', 'oldest'])
	})

	/**
	 * AC21, and the decision behind it: a note whose `created` cannot be parsed is
	 * *unknown*, not old and not new. Sorting it to the front under "oldest" would
	 * assert it is the oldest thing there, which the document does not say. It
	 * trails in file order instead — the same answer in both directions, which is
	 * what makes the rule statable in one sentence.
	 */
	it('trails the unreadable ones in file order, the same way in both directions', () => {
		const withGaps: Record<string, string> = { ...times, broken: 'not a date', alsoBroken: '' }
		const lookup = (id: string) => parseCreated(withGaps[id])
		const ids = ['broken', 'newest', 'alsoBroken', 'oldest']

		expect(sortByCreated(ids, lookup, 'oldest')).toEqual([
			'oldest',
			'newest',
			'broken',
			'alsoBroken',
		])
		expect(sortByCreated(ids, lookup, 'newest')).toEqual([
			'newest',
			'oldest',
			'broken',
			'alsoBroken',
		])
	})

	/** The store stamps to second precision, so a burst of captures genuinely
	 *  shares a timestamp. A stable sort is what stops those notes reshuffling
	 *  between renders. */
	it('keeps notes sharing a second in the order they arrived', () => {
		const same = () => Date.UTC(2026, 0, 1)
		expect(sortByCreated(['c', 'a', 'b'], same, 'newest')).toEqual(['c', 'a', 'b'])
		expect(sortByCreated(['c', 'a', 'b'], same, 'oldest')).toEqual(['c', 'a', 'b'])
	})

	it('handles an empty section', () => {
		expect(sortByCreated([], at, 'oldest')).toEqual([])
	})
})
