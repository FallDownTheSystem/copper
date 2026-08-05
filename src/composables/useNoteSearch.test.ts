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

const { query, matchedIds, matchTerms, resultCount, rebuild, clearQuery } = useNoteSearch()

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
	it('is null for an empty query, which is not the same as an empty set', () => {
		expect(matchedIds.value).toBeNull()
		// A query of only whitespace is indistinguishable from an empty field to
		// the user, so it must not filter either.
		query.value = '   '
		expect(matchedIds.value).toBeNull()
	})

	it('matches only notes containing every term', () => {
		query.value = 'inherited configuration'
		expect([...(matchedIds.value ?? [])]).toEqual(['a'])
	})

	it('matches on a prefix of the last word, so results narrow while typing', () => {
		query.value = 'inherit'
		expect(new Set(matchedIds.value)).toEqual(new Set(['a', 'b']))
	})

	it('is an empty set, not null, when a query matches nothing', () => {
		query.value = 'zzzznothing'
		expect(matchedIds.value).not.toBeNull()
		expect(resultCount.value).toBe(0)
	})

	it('reflects a body change after a rebuild, with the query untouched', () => {
		// The regression this file exists for: a `MiniSearch` instance is not
		// reactive, so without `indexRevision` the computed would only re-run when
		// the query changed — and every local merge, move and undo would leave the
		// results describing a document that no longer exists.
		query.value = 'kittens'
		expect([...(matchedIds.value ?? [])]).toEqual(['c'])

		rebuild(space([NOTES[0]!, NOTES[1]!, note('c', 'A note about puppies')]))
		expect([...(matchedIds.value ?? [])]).toEqual([])

		query.value = 'puppies'
		expect([...(matchedIds.value ?? [])]).toEqual(['c'])
	})

	it('drops every match when the document goes away', () => {
		query.value = 'inherited'
		expect(resultCount.value).toBeGreaterThan(0)
		rebuild(null)
		expect(resultCount.value).toBe(0)
	})
})

describe('matchTerms', () => {
	it('is empty with no query and splits on whitespace otherwise', () => {
		expect(matchTerms.value).toEqual([])
		query.value = '  two  terms '
		expect(matchTerms.value).toEqual(['two', 'terms'])
	})
})
