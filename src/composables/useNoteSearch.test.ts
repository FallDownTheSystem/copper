import { beforeEach, describe, expect, it } from 'vite-plus/test'

import { useNoteSearch } from './useNoteSearch'
import type { Note, Space } from './useSpace'

function note(id: string, body: string): Note {
	return {
		id,
		section: 'sec_1',
		order: 0,
		done: false,
		body,
		created: '2026-08-05T00:00:00Z',
		updated: '2026-08-05T00:00:00Z',
	}
}

function space(notes: Note[]): Space {
	return {
		id: 'spc_1',
		name: 'test',
		activeSection: 'sec_1',
		sections: [{ id: 'sec_1', name: 'Notes', order: 0 }],
		notes,
	}
}

const { query, matchedIds, matchNeedle, resultCount, rebuild, clearQuery } = useNoteSearch()

/** Matched ids in score order, which is what `useSelection` ranks a section by. */
function ranked(): string[] {
	const scores = matchedIds.value
	if (!scores) return []
	return [...scores.entries()].sort(([, a], [, b]) => b - a).map(([id]) => id)
}

const NOTES = [
	note('a', 'Negation in inherited configuration files'),
	note('b', 'Inherited scrollbar styling on Windows'),
	note('c', 'A note about kittens'),
]

beforeEach(() => {
	// Module-scope state outlives a test, exactly as it does a component.
	clearQuery()
	rebuild(space(NOTES))
})

describe('matchedIds', () => {
	it('is null for an empty query, which is not the same as an empty map', () => {
		expect(matchedIds.value).toBeNull()
		// A query of only whitespace is indistinguishable from an empty field to
		// the user, so it must not filter either.
		query.value = '   '
		expect(matchedIds.value).toBeNull()
	})

	it('matches every note whose body spells the query in order', () => {
		query.value = 'inherited configuration'
		expect([...(matchedIds.value ?? []).keys()]).toEqual(['a'])
	})

	it('matches a partial word, so results narrow while typing', () => {
		query.value = 'inherit'
		expect(new Set((matchedIds.value ?? new Map()).keys())).toEqual(new Set(['a', 'b']))
	})

	it('matches characters spread across words, which a substring search cannot', () => {
		// Task-014's whole point: `config file` is not a phrase in note `a`, and
		// task-006's `AND`-of-substrings would have found it only because both words
		// happen to be present. Here the query is one character sequence.
		query.value = 'negconf'
		expect([...(matchedIds.value ?? []).keys()]).toEqual(['a'])
	})

	it('ranks a tighter match above a scattered one', () => {
		rebuild(
			space([
				note('scattered', 'silently reordering the arguments'),
				note('tight', 'the sort order of a list'),
			]),
		)
		query.value = 'sort'
		// Both match — `s…o…r…t` appears in order in each — and the ranking is what
		// makes the answer useful rather than merely correct.
		expect(ranked()[0]).toBe('tight')
		expect(resultCount.value).toBe(2)
	})

	it('is an empty map, not null, when a query matches nothing', () => {
		query.value = 'zzzznothing'
		expect(matchedIds.value).not.toBeNull()
		expect(resultCount.value).toBe(0)
	})

	it('reflects a body change after a rebuild, with the query untouched', () => {
		// The regression this file exists for: results have to follow the applied
		// document, because task-003 emits no event for a change the frontend
		// invoked — so every local merge, move and undo would otherwise leave them
		// describing a document that no longer exists.
		query.value = 'kittens'
		expect([...(matchedIds.value ?? []).keys()]).toEqual(['c'])

		rebuild(space([NOTES[0]!, NOTES[1]!, note('c', 'A note about puppies')]))
		expect([...(matchedIds.value ?? []).keys()]).toEqual([])

		query.value = 'puppies'
		expect([...(matchedIds.value ?? []).keys()]).toEqual(['c'])
	})

	it('drops every match when the document goes away', () => {
		query.value = 'inherited'
		expect(resultCount.value).toBeGreaterThan(0)
		rebuild(null)
		expect(resultCount.value).toBe(0)
	})
})

describe('matchNeedle', () => {
	it('is empty with no query and one folded sequence otherwise', () => {
		expect(matchNeedle.value).toBe('')
		// Stripped rather than split: the highlighter has to paint the same sequence
		// this module ranked by, and a list of terms is a different question.
		query.value = '  Two  Terms '
		expect(matchNeedle.value).toBe('twoterms')
	})
})
