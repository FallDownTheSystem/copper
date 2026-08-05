import { beforeEach, describe, expect, it } from 'vite-plus/test'

import { noteRow, sectionRow, useSelection } from './useSelection'
import type { Note, Space } from './useSpace'

function note(id: string, section: string, order: number): Note {
	return {
		id,
		section,
		order,
		done: false,
		body: id,
		created: '2026-08-05T00:00:00Z',
		updated: '2026-08-05T00:00:00Z',
	}
}

/** Two sections, so range extension has a boundary to cross. */
function document2(
	noteIds: [string[], string[]] = [
		['n1', 'n2'],
		['n3', 'n4'],
	],
): Space {
	return {
		id: 'spc_1',
		name: 'development',
		activeSection: 'sec_a',
		sections: [
			{ id: 'sec_a', name: 'Research', order: 0 },
			{ id: 'sec_b', name: 'Inbox', order: 1 },
		],
		notes: [
			...noteIds[0].map((id, index) => note(id, 'sec_a', index)),
			...noteIds[1].map((id, index) => note(id, 'sec_b', index)),
		],
	}
}

const selection = useSelection()

beforeEach(() => {
	selection.resetForNewSpace()
	selection.syncDocument(document2())
})

describe('the two orders', () => {
	it('puts header rows in rowIds and keeps them out of visibleNoteIds', () => {
		expect(selection.rowIds.value).toEqual([
			sectionRow('sec_a'),
			noteRow('n1'),
			noteRow('n2'),
			sectionRow('sec_b'),
			noteRow('n3'),
			noteRow('n4'),
		])
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n2', 'n3', 'n4'])
	})
})

describe('select', () => {
	it('replaces the selection and sets both focus and the anchor', () => {
		selection.select('n1')
		selection.select('n3')

		expect(selection.selectedIds.value).toEqual(['n3'])
		expect(selection.focusedId.value).toBe(noteRow('n3'))
		expect(selection.anchorId.value).toBe('n3')
	})
})

describe('toggle', () => {
	it('adds then removes without clearing the rest', () => {
		selection.select('n1')
		selection.toggle('n3')
		expect(selection.selectedIds.value).toEqual(['n1', 'n3'])

		selection.toggle('n3')
		expect(selection.selectedIds.value).toEqual(['n1'])
	})
})

describe('extendTo', () => {
	it('produces a contiguous range across a section boundary', () => {
		selection.select('n2')
		selection.extendTo('n4')

		expect(selection.selectedIds.value).toEqual(['n2', 'n3', 'n4'])
		// The anchor stays put, so extending again grows from the same origin.
		expect(selection.anchorId.value).toBe('n2')
		expect(selection.focusedId.value).toBe(noteRow('n4'))
	})

	it('shrinks back towards the anchor rather than accumulating', () => {
		selection.select('n1')
		selection.extendTo('n4')
		selection.extendTo('n2')

		expect(selection.selectedIds.value).toEqual(['n1', 'n2'])
	})
})

describe('moveFocus', () => {
	it('traverses header rows and selects only when it lands on a note', () => {
		selection.focusRow(sectionRow('sec_a'))
		selection.moveFocus(1)
		expect(selection.focusedId.value).toBe(noteRow('n1'))
		expect(selection.selectedIds.value).toEqual(['n1'])

		selection.moveFocus(1)
		selection.moveFocus(1)
		// Landed on the second section's header: selection is left alone.
		expect(selection.focusedId.value).toBe(sectionRow('sec_b'))
		expect(selection.selectedIds.value).toEqual(['n2'])
	})

	it('clamps at both ends rather than wrapping', () => {
		selection.focusRow(sectionRow('sec_a'))
		selection.moveFocus(-1)
		expect(selection.focusedId.value).toBe(sectionRow('sec_a'))

		selection.focusLast()
		selection.moveFocus(1)
		expect(selection.focusedId.value).toBe(noteRow('n4'))
	})
})

describe('extendFocus', () => {
	it('moves between notes only, skipping the header between them', () => {
		selection.select('n2')
		selection.extendFocus(1)

		expect(selection.focusedId.value).toBe(noteRow('n3'))
		expect(selection.selectedIds.value).toEqual(['n2', 'n3'])
	})
})

describe('selectAll and clear', () => {
	it('selects every rendered note and then clears the anchor too', () => {
		selection.selectAll()
		expect(selection.selectedIds.value).toEqual(['n1', 'n2', 'n3', 'n4'])

		selection.clear()
		expect(selection.selectedIds.value).toEqual([])
		expect(selection.anchorId.value).toBeNull()
	})
})

describe('reconcile', () => {
	it('prunes selected ids that no longer exist and clears a deleted anchor', () => {
		selection.select('n2')
		selection.extendTo('n4')
		const snapshot = selection.snapshot()

		selection.syncDocument(document2([['n1'], ['n4']]))
		selection.reconcile(snapshot)

		expect(selection.selectedIds.value).toEqual(['n4'])
		expect(selection.anchorId.value).toBeNull()
	})

	it('relocates focus to the nearest survivor by the former flattened index', () => {
		selection.select('n2')
		const snapshot = selection.snapshot()

		// n2 is gone; n3 was the next in the former order and survives.
		selection.syncDocument(document2([['n1'], ['n3', 'n4']]))
		selection.reconcile(snapshot)

		expect(selection.focusedId.value).toBe(noteRow('n3'))
	})

	it('falls back to a preceding survivor when nothing after it remains', () => {
		selection.select('n3')
		const snapshot = selection.snapshot()

		selection.syncDocument(document2([['n1', 'n2'], []]))
		selection.reconcile(snapshot)

		expect(selection.focusedId.value).toBe(noteRow('n2'))
	})

	it('follows a note by id when it moved to another section', () => {
		selection.select('n3')
		const snapshot = selection.snapshot()

		selection.syncDocument(document2([['n1', 'n2', 'n3'], ['n4']]))
		selection.reconcile(snapshot)

		expect(selection.focusedId.value).toBe(noteRow('n3'))
		expect(selection.selectedIds.value).toEqual(['n3'])
	})

	it('gives the grid a roving target on first load without selecting anything', () => {
		selection.resetForNewSpace()
		const snapshot = selection.snapshot()
		selection.syncDocument(document2())
		selection.reconcile(snapshot)

		// Without this every row renders tabindex="-1" and the list cannot be
		// reached by Tab at all.
		expect(selection.focusedId.value).toBe(noteRow('n1'))
		expect(selection.selectedIds.value).toEqual([])
	})

	it('leaves nothing focused when the space has no rows at all', () => {
		selection.select('n1')
		const snapshot = selection.snapshot()

		selection.syncDocument(null)
		selection.reconcile(snapshot)

		expect(selection.focusedId.value).toBeNull()
		expect(selection.selectedIds.value).toEqual([])
	})
})
