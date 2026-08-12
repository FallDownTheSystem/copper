import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import { useNoteList } from './useNoteList'
import { noteRow, sectionRow, useSelection } from './useSelection'
import { useNoteSearch } from './useNoteSearch'
import type { Note, Space } from './useSpace'

// The setters remember themselves through `useSettings`, so this file needs the
// Tauri boundary stubbed where it needed nothing before. The writes land here
// and are otherwise ignored: what is *persisted* is `settings.rs`'s contract,
// and these tests own the view state, not the file.
const mocks = vi.hoisted(() => ({ invoke: vi.fn(async () => ({})) }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }))

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
	mocks.invoke.mockClear()
	list.hydrate(null)
	selection.resetForNewSpace()
	search.clearQuery()
	const doc = document2()
	search.rebuild(doc)
	list.rebuild(doc)
	selection.syncDocument(doc)
})

afterEach(() => {
	list.hydrate(null)
	search.clearQuery()
})

describe('the done filter', () => {
	/** The default is the whole document — a ruling that reversed the short-lived
	 *  todo default: a note that vanishes the moment it is ticked is a note the
	 *  user has to work out a control to get back. */
	it('shows the whole document until asked to narrow it', () => {
		expect(list.doneFilter.value).toBe('all')
		expect(list.filtersByDone.value).toBe(false)
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n2', 'n3', 'n4', 'n5'])
	})

	/** AC2. */
	it('leaves only the done notes on screen', () => {
		list.setDoneFilter('done')
		expect(selection.visibleNoteIds.value).toEqual(['n2', 'n5'])
	})

	/** The narrowing state that is not `done`: the unfinished half on its own. */
	it('leaves only the unfinished notes on screen in the todo view', () => {
		list.setDoneFilter('todo')
		expect(list.filtersByDone.value).toBe(true)
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n3', 'n4'])
	})

	/** Three presses come back where they started, so every state is reachable
	 *  from every other without a menu. The resting view leads and every press
	 *  from it narrows. */
	it('cycles all → todo → done → all', () => {
		expect(list.nextDoneFilter.value).toBe('todo')
		list.cycleDoneFilter()
		expect(list.doneFilter.value).toBe('todo')

		list.cycleDoneFilter()
		expect(list.doneFilter.value).toBe('done')

		list.cycleDoneFilter()
		expect(list.doneFilter.value).toBe('all')
	})

	/** Document-wide, unlike `useNoteActions.doneCount`, which is the bulk
	 *  delete's active-section scope. The button's counts are these three. */
	it('counts every done note in the document', () => {
		expect(list.doneTotal.value).toBe(2)
	})

	/** The other two totals the cycle button offers, on the same document-wide
	 *  scale — one of them counted differently would be a number the press does
	 *  not deliver. */
	it('sizes all three views from the same census', () => {
		expect(list.allTotal.value).toBe(5)
		expect(list.todoTotal.value).toBe(3)
	})

	/** The todo view narrows the same way the done view does, so the same rule
	 *  applies to it: `Ctrl+A` under it selects the unfinished notes. */
	it('narrows what an action targets in the todo view too', () => {
		list.setDoneFilter('todo')
		expect(selection.actionableNoteIds.value).toEqual(['n1', 'n3', 'n4'])

		selection.selectAll()
		expect(selection.selectedIds.value).toEqual(['n1', 'n3', 'n4'])
	})

	/** A section whose notes are all finished keeps its heading in the todo view
	 *  (user ruling 2026-08-12, reversing the search-miss treatment): the heading
	 *  is how the section is still reached and captured into. Only its notes go. */
	it('keeps the heading of a section with nothing left to do', () => {
		const doc = document2()
		for (const entry of doc.notes) {
			if (entry.section === 'sec_b') entry.done = true
		}
		list.rebuild(doc)
		selection.syncDocument(doc)

		list.setDoneFilter('todo')
		expect(selection.rowIds.value).toEqual([
			sectionRow('sec_a'),
			noteRow('n1'),
			noteRow('n3'),
			sectionRow('sec_b'),
		])
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

	/** The done view keeps an empty section's heading for the same reason the todo
	 *  view does — the filter hides notes, never sections. A search is what drops
	 *  headings. */
	it('keeps the heading of a section with no done note', () => {
		const doc = document2()
		doc.notes = doc.notes.filter((entry) => entry.section === 'sec_a' || !entry.done)
		list.rebuild(doc)
		selection.syncDocument(doc)

		list.setDoneFilter('done')
		expect(selection.rowIds.value).toEqual([
			sectionRow('sec_a'),
			noteRow('n2'),
			sectionRow('sec_b'),
		])
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

	/** AC3's space-switch reset is gone with the controls' memory (user ruling
	 *  2026-08-12): the filter is an app-wide remembered preference now, and the
	 *  only thing that moves it besides the button is `hydrate`, at boot. Narrowed
	 *  by name on the way in, the same split `theme` uses. */
	it('starts where the file remembers, narrowed by name', () => {
		list.hydrate({ doneFilter: 'todo', sortMode: 'manual' })
		expect(list.doneFilter.value).toBe('todo')
		list.hydrate({ doneFilter: 'finished', sortMode: 'manual' })
		expect(list.doneFilter.value).toBe('all')
	})

	/** The write half of the memory: a change remembers itself, one key wide, and
	 *  a press that changes nothing writes nothing. */
	it('remembers a change through the settings surface', () => {
		list.setDoneFilter('done')
		expect(mocks.invoke).toHaveBeenCalledWith('update_settings', {
			patch: { doneFilter: 'done' },
		})
		mocks.invoke.mockClear()
		list.setDoneFilter('done')
		expect(mocks.invoke).not.toHaveBeenCalled()
	})

	/** A capture landing while the user reviews done notes is not a change of
	 *  intent, and taking the view away mid-task would be one. */
	it('survives a document change', () => {
		list.setDoneFilter('done')
		list.rebuild(document2())
		expect(list.doneOnly.value).toBe(true)
	})
})

describe('sort', () => {
	/** These are about the order, and the fixture's two done notes have to be in
	 *  the list for a reordering of it to be observable — so the filter is pinned
	 *  wide open rather than trusted to a default that has changed once already.
	 *  Each case that is about the filter as well says so by setting it. */
	beforeEach(() => {
		list.setDoneFilter('all')
	})

	/** AC13, and the scope the mode being document-wide did *not* change: notes are
	 *  ordered inside their own section, and the sections themselves neither move
	 *  nor interleave. */
	it('orders the notes inside every section by created', () => {
		list.setSort('oldest')
		expect(selection.visibleNoteIds.value).toEqual(['n2', 'n3', 'n1', 'n5', 'n4'])

		list.setSort('newest')
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n3', 'n2', 'n4', 'n5'])
	})

	/** AC12, restated for one mode: every section is on it, and the headers stay
	 *  where they are with the same notes under them. */
	it('applies the one mode to every section, headers included', () => {
		list.setSort('newest')

		expect(list.sortMode.value).toBe('newest')
		expect(list.isSorted.value).toBe(true)
		expect(selection.rowIds.value).toEqual([
			sectionRow('sec_a'),
			noteRow('n1'),
			noteRow('n3'),
			noteRow('n2'),
			sectionRow('sec_b'),
			noteRow('n4'),
			noteRow('n5'),
		])
	})

	/** AC15. */
	it('returns to document order on Manual', () => {
		list.setSort('newest')
		list.setSort('manual')

		expect(list.isSorted.value).toBe(false)
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n2', 'n3', 'n4', 'n5'])
	})

	/** The order is a *presentation* of the set. A multi-note copy out of a
	 *  newest-first list must still come out in document order, which is the
	 *  same contract the search ranking already respects. */
	it('reorders the rows but not what an action targets', () => {
		list.setSort('newest')
		expect(selection.actionableNoteIds.value).toEqual(['n1', 'n2', 'n3', 'n4', 'n5'])
	})

	/** An explicit sort outranks the implicit relevance ranking: relevance is
	 *  something the search computed, a mode is something the user went and chose. */
	it('wins over the search ranking where both apply', () => {
		list.setSort('oldest')
		search.query.value = 'n'
		expect(selection.visibleNoteIds.value.slice(0, 3)).toEqual(['n2', 'n3', 'n1'])
	})

	it('composes with the done filter', () => {
		list.setDoneFilter('done')
		list.setSort('oldest')
		expect(selection.visibleNoteIds.value).toEqual(['n2', 'n5'])
	})

	/** Notes the store could not date sort into a trailing tier rather than
	 *  claiming a position — the display rule applied to ordering. */
	it('trails a note whose stored date is unreadable', () => {
		const doc = document2()
		doc.notes[0]!.created = 'yesterday afternoon'
		list.rebuild(doc)
		selection.syncDocument(doc)

		list.setSort('oldest')
		expect(selection.visibleNoteIds.value.slice(0, 3)).toEqual(['n2', 'n3', 'n1'])

		list.setSort('newest')
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
		list.setSort('oldest')
		expect(selection.visibleNoteIds.value.slice(0, 4)).toEqual(['n2', 'n3', 'n1', 'n6'])
	})

	/** The sort's half of the same memory: hydrated by name at boot, remembered
	 *  one key wide on change. */
	it('starts where the file remembers, and remembers a change', () => {
		list.hydrate({ doneFilter: 'all', sortMode: 'newest' })
		expect(list.sortMode.value).toBe('newest')
		list.hydrate({ doneFilter: 'all', sortMode: 'alphabetical' })
		expect(list.sortMode.value).toBe('manual')

		list.setSort('oldest')
		expect(mocks.invoke).toHaveBeenCalledWith('update_settings', {
			patch: { sortMode: 'oldest' },
		})
	})

	/**
	 * The mode names no section, so nothing about a document can invalidate it.
	 *
	 * While the modes were per section, `rebuild` had to prune the ones naming a
	 * deleted section: dead weight nothing could remove, and since an undone
	 * section delete restores exactly the id it removed, the section came back
	 * mysteriously sorted. One document-wide mode outlives a section delete, a
	 * capture and a null document alike — like the filter, it says how to read
	 * whatever the document turns out to hold, and a document change is not a
	 * change of intent.
	 */
	it('survives a document change, a section going away and a null document', () => {
		list.setSort('newest')
		list.rebuild(document2())
		expect(list.sortMode.value).toBe('newest')

		const doc = document2()
		doc.sections = doc.sections.filter((section) => section.id !== 'sec_b')
		doc.notes = doc.notes.filter((entry) => entry.section !== 'sec_b')
		list.rebuild(doc)
		expect(list.sortMode.value).toBe('newest')

		list.rebuild(null)
		expect(list.sortMode.value).toBe('newest')
	})
})
