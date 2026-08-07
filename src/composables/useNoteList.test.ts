import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test'

import { useNoteList } from './useNoteList'
import { noteRow, sectionRow, useSelection } from './useSelection'
import { useNoteSearch } from './useNoteSearch'
import type { Note, Space } from './useSpace'

/** Five notes across two sections, deliberately created out of document order so
 *  a sort has something to actually move. `n2` and `n5` are done. */
function note(id: string, section: string, order: number, created: string, done = false): Note {
	return { id, section, order, done, body: id, created, updated: created }
}

function document2(): Space {
	return {
		id: 'spc_1',
		name: 'development',
		activeSection: 'sec_a',
		sections: [
			{ id: 'sec_a', name: 'Research', order: 0 },
			{ id: 'sec_b', name: 'Inbox', order: 1 },
		],
		notes: [
			note('n1', 'sec_a', 0, '2026-03-01T00:00:00Z'),
			note('n2', 'sec_a', 1, '2026-01-01T00:00:00Z', true),
			note('n3', 'sec_a', 2, '2026-02-01T00:00:00Z'),
			note('n4', 'sec_b', 0, '2026-05-01T00:00:00Z'),
			note('n5', 'sec_b', 1, '2026-04-01T00:00:00Z', true),
		],
	}
}

const list = useNoteList()
const selection = useSelection()
const search = useNoteSearch()

beforeEach(() => {
	list.reset()
	selection.resetForNewSpace()
	search.clearQuery()
	const doc = document2()
	search.rebuild(doc)
	list.rebuild(doc)
	selection.syncDocument(doc)
})

afterEach(() => {
	list.reset()
	search.clearQuery()
})

describe('the done filter', () => {
	it('is off until asked for', () => {
		expect(list.doneOnly.value).toBe(false)
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n2', 'n3', 'n4', 'n5'])
	})

	/** AC2. */
	it('leaves only the done notes on screen', () => {
		list.setDoneFilter('done')
		expect(selection.visibleNoteIds.value).toEqual(['n2', 'n5'])
	})

	/**
	 * The filter is on the `actionable` side of the split rather than the collapse
	 * side: it is a scope the user chose in order to act on it, so `Ctrl+A` in the
	 * done view selects the done notes and nothing else.
	 */
	it('narrows what an action targets, the way a search does', () => {
		list.setDoneFilter('done')
		expect(selection.actionableNoteIds.value).toEqual(['n2', 'n5'])

		selection.selectAll()
		expect(selection.selectedIds.value).toEqual(['n2', 'n5'])
	})

	/** A section with nothing done drops out entirely, header included — the same
	 *  treatment a search miss gets, and what keeps the done view from being a wall
	 *  of empty headings. */
	it('drops a section with no done note, header and all', () => {
		const doc = document2()
		doc.notes = doc.notes.filter((entry) => entry.section === 'sec_a' || !entry.done)
		list.rebuild(doc)
		selection.syncDocument(doc)

		list.setDoneFilter('done')
		expect(selection.rowIds.value).toEqual([sectionRow('sec_a'), noteRow('n2')])
	})

	/** AC4. The two are orthogonal in both directions. */
	it('composes with a search rather than replacing it', () => {
		list.setDoneFilter('done')
		search.query.value = 'n5'
		expect(selection.visibleNoteIds.value).toEqual(['n5'])

		// Clearing the query keeps the filter, rather than dropping back to all.
		search.clearQuery()
		expect(list.doneOnly.value).toBe(true)
		expect(selection.visibleNoteIds.value).toEqual(['n2', 'n5'])
	})

	it('toggles back to all', () => {
		list.toggleDoneFilter()
		expect(list.doneOnly.value).toBe(true)
		list.toggleDoneFilter()
		expect(list.doneOnly.value).toBe(false)
		expect(selection.visibleNoteIds.value).toHaveLength(5)
	})

	/** AC3, restated against the event that actually exists: the panel renders
	 *  every section at once, so a space switch — not a section change — is what
	 *  drops the filter. */
	it('drops back to all when the space is replaced', () => {
		list.setDoneFilter('done')
		list.reset()
		expect(list.doneOnly.value).toBe(false)
	})

	/** A capture landing while the user reviews done notes is not a change of
	 *  intent, and taking the view away mid-task would be one. */
	it('survives a document change', () => {
		list.setDoneFilter('done')
		list.rebuild(document2())
		expect(list.doneOnly.value).toBe(true)
	})
})

describe('per-section sort', () => {
	/** AC13. */
	it('orders one section by created without touching the other', () => {
		list.setSort('sec_a', 'oldest')
		expect(selection.visibleNoteIds.value).toEqual(['n2', 'n3', 'n1', 'n4', 'n5'])

		list.setSort('sec_a', 'newest')
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n3', 'n2', 'n4', 'n5'])
	})

	/** AC12. Each section keeps its own choice; they do not share one. */
	it('holds a separate mode per section', () => {
		list.setSort('sec_a', 'newest')
		list.setSort('sec_b', 'oldest')

		expect(list.sortOf('sec_a')).toBe('newest')
		expect(list.sortOf('sec_b')).toBe('oldest')
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n3', 'n2', 'n5', 'n4'])
	})

	/** AC15. */
	it('returns to document order on Manual', () => {
		list.setSort('sec_a', 'newest')
		list.setSort('sec_a', 'manual')

		expect(list.isSorted('sec_a')).toBe(false)
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n2', 'n3', 'n4', 'n5'])
	})

	/** The order is a *presentation* of the set. A multi-note copy out of a
	 *  newest-first section must still come out in document order, which is the
	 *  same contract the search ranking already respects. */
	it('reorders the rows but not what an action targets', () => {
		list.setSort('sec_a', 'newest')
		expect(selection.actionableNoteIds.value).toEqual(['n1', 'n2', 'n3', 'n4', 'n5'])
	})

	/** An explicit sort outranks the implicit relevance ranking: relevance is
	 *  something the search computed, a mode is something the user went and chose. */
	it('wins over the search ranking where both apply', () => {
		list.setSort('sec_a', 'oldest')
		search.query.value = 'n'
		expect(selection.visibleNoteIds.value.slice(0, 3)).toEqual(['n2', 'n3', 'n1'])
	})

	it('composes with the done filter', () => {
		list.setDoneFilter('done')
		list.setSort('sec_a', 'oldest')
		expect(selection.visibleNoteIds.value).toEqual(['n2', 'n5'])
	})

	/** Notes the store could not date sort into a trailing tier rather than
	 *  claiming a position — the display rule applied to ordering. */
	it('trails a note whose stored date is unreadable', () => {
		const doc = document2()
		doc.notes[0]!.created = 'yesterday afternoon'
		list.rebuild(doc)
		selection.syncDocument(doc)

		list.setSort('sec_a', 'oldest')
		expect(selection.visibleNoteIds.value.slice(0, 3)).toEqual(['n2', 'n3', 'n1'])

		list.setSort('sec_a', 'newest')
		expect(selection.visibleNoteIds.value.slice(0, 3)).toEqual(['n3', 'n2', 'n1'])
	})

	/**
	 * AC16, and the reason it costs no code.
	 *
	 * Task-013's `insertionPoint` is consumed in Rust, inside `ops::add_note`, as
	 * the `order` the new note is written with. The sort here is a *view transform*
	 * over that document order — so under Manual the document order is the view and
	 * the setting is plainly visible, while under a computed sort the note lands
	 * where the setting says in the file and is then sorted into its chronological
	 * place. Nothing has to detect the sort at capture time, and task-013's
	 * semantics are not contradicted: they are about the document, not the view.
	 */
	it('shows task-013’s insertion point under Manual and sorts past it otherwise', () => {
		// A top insertion: newest note, written to the head of `sec_a`. Its `created`
		// is the most recent of the three.
		const doc = document2()
		doc.notes = [
			note('n6', 'sec_a', 0, '2026-09-01T00:00:00Z'),
			...doc.notes.map((entry) =>
				entry.section === 'sec_a' ? { ...entry, order: entry.order + 1 } : entry,
			),
		]
		list.rebuild(doc)
		selection.syncDocument(doc)

		// Manual: the document order is the view, so the setting is what you see.
		expect(selection.visibleNoteIds.value.slice(0, 4)).toEqual(['n6', 'n1', 'n2', 'n3'])

		// Oldest first: it appended-and-re-sorted into last place despite being
		// written first.
		list.setSort('sec_a', 'oldest')
		expect(selection.visibleNoteIds.value.slice(0, 4)).toEqual(['n2', 'n3', 'n1', 'n6'])
	})

	it('drops every mode when the space is replaced', () => {
		list.setSort('sec_a', 'newest')
		list.reset()
		expect(list.sortOf('sec_a')).toBe('manual')
	})

	/**
	 * A mode for a section that no longer exists is dead weight nothing can
	 * remove — and if the id comes back, which an undone section delete does
	 * exactly, the section would return mysteriously sorted.
	 */
	/** `rebuild` brings the sort map into line with the document it is handed, and
	 *  a null document has no sections — so every mode in it names something that
	 *  does not exist. Contract consistency, not a reachable bug. */
	it('drops every mode when the document goes away', () => {
		list.setSort('sec_a', 'oldest')
		list.rebuild(null)
		expect(list.sortOf('sec_a')).toBe('manual')
	})

	it('prunes a mode whose section is gone', () => {
		list.setSort('sec_b', 'newest')

		const doc = document2()
		doc.sections = doc.sections.filter((section) => section.id !== 'sec_b')
		doc.notes = doc.notes.filter((entry) => entry.section !== 'sec_b')
		list.rebuild(doc)

		expect(list.sortOf('sec_b')).toBe('manual')
	})
})
