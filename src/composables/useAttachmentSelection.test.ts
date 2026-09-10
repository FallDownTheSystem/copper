import { beforeEach, describe, expect, it } from 'vite-plus/test'
import { nextTick } from 'vue'

import { useAttachmentSelection } from './useAttachmentSelection'
import { useSelection } from './useSelection'
import { useNoteSearch } from './useNoteSearch'
import { useNoteList } from './useNoteList'
import { useSections } from './useSections'
import type { Space } from './useSpace'

const selection = useAttachmentSelection()
const notes = useSelection()
const first = { note: 'note-a', attachment: 'a-1' }
const second = { note: 'note-a', attachment: 'a-2' }
const third = { note: 'note-b', attachment: 'b-1' }

function document(): Space {
	return {
		id: 'space',
		name: 'Test',
		activeSection: 'section',
		sections: [{ id: 'section', name: 'Notes', order: 0 }],
		notes: ['note-a', 'note-b'].map((id, order) => ({
			id,
			section: 'section',
			order,
			body: id,
			done: false,
			created: '',
			updated: '',
			attachments: (order === 0 ? ['a-1', 'a-2'] : ['b-1']).map((attachment) => ({
				id: attachment,
				name: `${attachment}.txt`,
				file: `${attachment}.txt`,
				mime: 'text/plain',
				bytes: 1,
			})),
		})),
	}
}

beforeEach(async () => {
	selection.syncDocument(null)
	notes.resetForNewSpace()
	useSections().reset()
	useNoteSearch().clearQuery()
	useNoteList().setDoneFilter('all')
	const space = document()
	useNoteList().rebuild(space)
	notes.syncDocument(space)
	selection.syncDocument(space)
	await nextTick()
})

describe('attachment selection', () => {
	it('keeps file and note selections mutually exclusive', () => {
		notes.select('note-a')
		selection.select(first)
		expect(notes.selectedIds.value).toEqual([])
		expect(selection.copyTargets()).toEqual([first])
		notes.select('note-b')
		expect(selection.active.value).toBe(false)
	})

	it('keeps attachment focus through passive reconciliation but clears it for an explicit note selection', () => {
		notes.select('note-a')
		selection.focus(first)
		notes.reconcile(notes.snapshot())
		expect(selection.active.value).toBe(true)
		expect(selection.copyTargets()).toEqual([first])
		notes.select('note-a')
		expect(selection.active.value).toBe(false)
	})

	it('supports additive and range selection across notes in visible order', () => {
		selection.select(third)
		selection.toggle(first)
		expect(selection.copyTargets()).toEqual([first, third])
		selection.select(second, { shiftKey: true })
		expect(selection.copyTargets()).toEqual([first, second])
		selection.select(third, { shiftKey: true })
		expect(selection.copyTargets()).toEqual([first, second, third])
	})

	it('uses the focused attachment alone when focus is outside the selection', () => {
		selection.select(first)
		selection.toggle(second)
		selection.focus(third)
		expect(selection.copyTargets()).toEqual([third])
		selection.ensure(first)
		expect(selection.copyTargets()).toEqual([first, second])
	})

	it('selects all attachments in the current note, not other notes', () => {
		selection.select(third)
		selection.focus(first)
		selection.selectAllInNote('note-a')
		expect(selection.copyTargets()).toEqual([first, second])
	})

	it('supports keyboard range selection and focus-only navigation', () => {
		selection.select(first)
		expect(selection.move(first, 1, { shiftKey: true })).toEqual(second)
		expect(selection.copyTargets()).toEqual([first, second])
		selection.move(second, 1, { ctrlKey: true })
		expect(selection.selected.value).toEqual([first, second])
		expect(selection.copyTargets()).toEqual([third])
		expect(selection.move(third, 1, {})).toBeNull()
	})

	it('prunes removed and hidden attachments and resets on a space switch', async () => {
		selection.select(first)
		selection.toggle(third)
		const next = document()
		next.notes[0]!.attachments = []
		selection.syncDocument(next)
		await nextTick()
		expect(selection.selected.value).toEqual([third])
		useSections().toggleCollapsed('section')
		await nextTick()
		expect(selection.active.value).toBe(false)
		useSections().toggleCollapsed('section')
		await nextTick()
		selection.select(third)
		selection.syncDocument({ ...next, id: 'other-space' })
		expect(selection.active.value).toBe(false)
	})
})
