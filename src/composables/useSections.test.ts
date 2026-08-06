import { afterEach, describe, expect, it } from 'vite-plus/test'

import { useNoteSearch } from './useNoteSearch'
import { useSections } from './useSections'
import type { Space } from './useSpace'

const sections = useSections()
const search = useNoteSearch()

function makeSpace(notes: { id: string; section: string }[]): Space {
	return {
		id: 'spc_1',
		name: 'development',
		activeSection: 'sec_a',
		sections: [
			{ id: 'sec_a', name: 'Research', order: 0 },
			{ id: 'sec_b', name: 'Inbox', order: 1 },
		],
		notes: notes.map((note, order) => ({
			...note,
			order,
			done: false,
			body: note.id,
			created: '2026-08-06T00:00:00Z',
			updated: '2026-08-06T00:00:00Z',
		})),
	}
}

afterEach(() => {
	// Module-scoped by design, so it outlives any one test exactly as it outlives
	// the component tree in the app.
	sections.reset()
	search.clearQuery()
})

describe('collapse', () => {
	it('toggles one section without touching the others', () => {
		sections.toggleCollapsed('sec_a')

		expect(sections.isCollapsed('sec_a')).toBe(true)
		expect(sections.isCollapsed('sec_b')).toBe(false)

		sections.toggleCollapsed('sec_a')
		expect(sections.isCollapsed('sec_a')).toBe(false)
	})

	it('sets an explicit value idempotently, which is what the arrow keys need', () => {
		// ArrowLeft collapses and ArrowRight expands; holding either must not toggle
		// back and forth.
		sections.setCollapsed('sec_a', true)
		sections.setCollapsed('sec_a', true)
		expect(sections.isCollapsed('sec_a')).toBe(true)

		sections.setCollapsed('sec_a', false)
		sections.setCollapsed('sec_a', false)
		expect(sections.isCollapsed('sec_a')).toBe(false)
	})
})

describe('the search override', () => {
	it('expands everything while a query is active and restores it afterwards', () => {
		sections.toggleCollapsed('sec_a')
		expect(sections.isCollapsed('sec_a')).toBe(true)

		search.query.value = 'anything'
		// A matching note inside a collapsed section must never be hidden by the
		// collapse — search decides what is on screen while it is running.
		expect(sections.isCollapsed('sec_a')).toBe(false)
		// The stored state is overridden, not cleared: that is what makes the
		// restore below free rather than a save-and-replay step.
		expect(sections.isCollapsedStored('sec_a')).toBe(true)

		search.clearQuery()
		expect(sections.isCollapsed('sec_a')).toBe(true)
	})

	it('treats a whitespace-only query as no query at all', () => {
		sections.toggleCollapsed('sec_a')
		search.query.value = '   '
		expect(sections.isCollapsed('sec_a')).toBe(true)
	})
})

describe('reconciling the collapse set against an applied document', () => {
	const before = makeSpace([{ id: 'n1', section: 'sec_a' }])

	it('expands the section a new note arrived in', () => {
		sections.toggleCollapsed('sec_a')

		sections.reconcile(
			before,
			makeSpace([
				{ id: 'n1', section: 'sec_a' },
				{ id: 'n2', section: 'sec_a' },
			]),
		)

		// A capture that vanished into a collapsed section is the one failure a
		// silent-on-success tool cannot afford.
		expect(sections.isCollapsed('sec_a')).toBe(false)
	})

	it('leaves other collapsed sections alone', () => {
		sections.toggleCollapsed('sec_a')
		sections.toggleCollapsed('sec_b')

		sections.reconcile(
			before,
			makeSpace([
				{ id: 'n1', section: 'sec_a' },
				{ id: 'n2', section: 'sec_b' },
			]),
		)

		expect(sections.isCollapsed('sec_b')).toBe(false)
		expect(sections.isCollapsed('sec_a')).toBe(true)
	})

	it('is not fooled by a note that merely moved into a collapsed section', () => {
		sections.toggleCollapsed('sec_b')

		// `n1` already existed; `Move to ▸` put it somewhere folded away, which is a
		// destination the user chose rather than a note appearing unannounced.
		sections.reconcile(before, makeSpace([{ id: 'n1', section: 'sec_b' }]))

		expect(sections.isCollapsed('sec_b')).toBe(true)
	})

	it('does nothing when nothing is collapsed, and needs no previous document', () => {
		const next = makeSpace([
			{ id: 'n1', section: 'sec_a' },
			{ id: 'n2', section: 'sec_a' },
		])

		expect(() => sections.reconcile(before, next)).not.toThrow()
		expect(() => sections.reconcile(null, next)).not.toThrow()
		expect(sections.isCollapsed('sec_a')).toBe(false)
	})

	it('clears the stored state even while a query is hiding the collapse', () => {
		sections.toggleCollapsed('sec_a')
		search.query.value = 'anything'

		sections.reconcile(
			before,
			makeSpace([
				{ id: 'n1', section: 'sec_a' },
				{ id: 'n2', section: 'sec_a' },
			]),
		)

		// Otherwise clearing the query would fold the new note away again — the
		// vanishing capture, one step later.
		search.clearQuery()
		expect(sections.isCollapsed('sec_a')).toBe(false)
	})
})

describe('pruning ids the document no longer has', () => {
	it('forgets a collapsed section that was deleted', () => {
		sections.toggleCollapsed('sec_b')

		sections.reconcile(makeSpace([]), {
			...makeSpace([]),
			sections: [{ id: 'sec_a', name: 'Research', order: 0 }],
		})

		expect(sections.isCollapsed('sec_b')).toBe(false)
		// And a section reintroduced under the same id — which is exactly what
		// undoing a delete does — comes back expanded rather than mysteriously shut.
		sections.reconcile(makeSpace([]), makeSpace([]))
		expect(sections.isCollapsed('sec_b')).toBe(false)
	})

	it('restores the empty-set fast path, so a dead id cannot cost every apply', () => {
		sections.toggleCollapsed('sec_b')
		sections.reconcile(makeSpace([]), {
			...makeSpace([]),
			sections: [{ id: 'sec_a', name: 'Research', order: 0 }],
		})

		// The set is genuinely empty again, not merely reporting false: an id that
		// can never be removed defeats the size check for the rest of the session.
		sections.toggleCollapsed('sec_a')
		sections.toggleCollapsed('sec_a')
		expect(sections.isCollapsedStored('sec_b')).toBe(false)
	})
})

describe('the switcher state', () => {
	it('opens with an empty filter and clears it again on close', () => {
		sections.filterQuery.value = 'left over'
		sections.openSwitcher()

		expect(sections.switcherOpen.value).toBe(true)
		expect(sections.filterQuery.value).toBe('')

		sections.filterQuery.value = 'res'
		sections.closeSwitcher()

		expect(sections.switcherOpen.value).toBe(false)
		expect(sections.filterQuery.value).toBe('')
	})

	it('shows in one host at a time, and each runs the same lifecycle', () => {
		sections.openSwitcher('menu')
		expect(sections.isSwitcherOpenIn('menu')).toBe(true)
		// One shared boolean would have opened the chip's dropdown as well.
		expect(sections.isSwitcherOpenIn('chip')).toBe(false)

		sections.filterQuery.value = 'res'
		sections.closeSwitcher('menu')
		// The filter is cleared by *either* host closing. Leaving it behind is what
		// made a reopened submenu come up pre-filtered, showing only
		// `Create section "<old query>"` for a query nobody had typed.
		expect(sections.filterQuery.value).toBe('')

		sections.openSwitcher('menu')
		sections.filterQuery.value = 'res'
		// A close from the host that is not showing must not dismiss the one that is.
		sections.closeSwitcher('chip')
		expect(sections.isSwitcherOpenIn('menu')).toBe(true)
		expect(sections.filterQuery.value).toBe('res')
	})

	it('is closed rather than re-pointed when the document identity changes', () => {
		for (const host of ['chip', 'menu'] as const) {
			sections.toggleCollapsed('sec_a')
			sections.openSwitcher(host)
			sections.filterQuery.value = 'res'

			sections.reset()

			// Section ids address a different document now, so neither the collapse
			// set nor an open menu full of the previous space's rows can carry over —
			// and that holds for whichever host was showing it.
			expect(sections.switcherOpen.value, host).toBe(false)
			expect(sections.filterQuery.value, host).toBe('')
			expect(sections.isCollapsed('sec_a'), host).toBe(false)
		}
	})
})
