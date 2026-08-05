import { mount } from '@vue/test-utils'
import axe from 'axe-core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import PanelShell from './PanelShell.vue'
// Statically imported, like PanelShell itself: a dynamic import after
// `vi.resetModules()` would resolve a *second* instance of a module whose state
// is module-scoped by design, and the component tree would not share it.
import { useNoteEditor } from '@/composables/useNoteEditor'
import { useNoteSearch } from '@/composables/useNoteSearch'
import { useSelection } from '@/composables/useSelection'
import { useSpace } from '@/composables/useSpace'
import type { Space, StoreStatus } from '@/composables/useSpace'

const editor = useNoteEditor()
const search = useNoteSearch()
const selection = useSelection()
const space = useSpace()

// happy-dom implements no Web Animations API, and auto-animate calls
// `el.animate` from a MutationObserver callback — so a test that adds or removes
// a row (filtering, undo, delete) throws out of band rather than failing an
// assertion. Stubbed rather than worked around in the component: the animation
// is real product behaviour and only the environment is missing.
//
// It has to *finish*, not merely exist: auto-animate re-appends a removed
// element to animate it out and only takes it back out of the DOM on the
// `finish` event, so a stub that never fires one leaves every filtered-out row
// on screen forever.
// Reached through an index signature rather than as `Element.prototype.animate`:
// the typed property is a method, and both reading it and narrowing on it upset
// the linter for reasons that have nothing to do with a stub.
const elementPrototype = Element.prototype as unknown as Record<string, unknown>
elementPrototype.animate ??= () => {
	const finishHandlers: (() => void)[] = []
	queueMicrotask(() => {
		for (const handler of finishHandlers) handler()
	})
	return {
		playState: 'finished',
		finished: Promise.resolve(),
		cancel: () => {},
		removeEventListener: () => {},
		addEventListener: (name: string, handler: () => void) => {
			if (name === 'finish') finishHandlers.push(handler)
		},
	}
}

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), openUrl: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
// `emit` is here for the capture notice the shell mounts, which signals frontend
// readiness once its listeners resolve.
vi.mock('@tauri-apps/api/event', () => ({
	listen: async () => () => {},
	emit: async () => {},
}))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: mocks.openUrl }))

const SPACE: Space = {
	id: 'spc_1',
	name: 'development',
	activeSection: 'sec_a',
	sections: [
		{ id: 'sec_a', name: 'Research', order: 0 },
		{ id: 'sec_b', name: 'Inbox', order: 1 },
	],
	notes: [
		{
			id: 'nte_1',
			section: 'sec_a',
			order: 0,
			done: false,
			body: 'first note',
			created: '2026-08-05T00:00:00Z',
			updated: '2026-08-05T00:00:00Z',
		},
		{
			id: 'nte_2',
			section: 'sec_a',
			order: 1,
			done: true,
			body: ['```js', 'const a = 1', '```'].join('\n'),
			created: '2026-08-05T00:00:00Z',
			updated: '2026-08-05T00:00:00Z',
		},
	],
}

const STATUS: StoreStatus = {
	path: 'C:\\notes.copper',
	errored: false,
	watching: true,
	canUndo: false,
	canRedo: false,
	startupNotice: null,
}

beforeEach(() => {
	vi.resetModules()
	mocks.invoke.mockReset()
	mocks.invoke.mockImplementation(async (command: string) => {
		if (command === 'get_active_space') return SPACE
		if (command === 'get_status') return STATUS
		if (command === 'get_settings') {
			return { recents: [], activeSpace: 0, panelPosition: null, shortcuts: {}, theme: 'system' }
		}
		if (command === 'clipboard_write_text') return null
		if (command === 'editor_handoffs') return []
		if (command === 'set_notes_done') return SPACE
		// An empty stack is `null`, not an error (task-003 §4.5).
		if (command === 'undo' || command === 'redo') return null
		throw { kind: 'invalid', message: command }
	})
})

/** The mounted panel, so it can be torn down rather than merely detached. */
let panel: ReturnType<typeof mount> | null = null

afterEach(() => {
	// Unmounted, not just wiped from the DOM. Clearing `body.innerHTML` leaves the
	// app alive and re-rendering into detached nodes — and an open portalled menu
	// with it, which then shows up as content outside a landmark in the axe run
	// several tests later.
	panel?.unmount()
	panel = null

	editor.cancel()
	// Module-scoped by design, so it outlives the component tree exactly as it
	// does in the app. A query left behind would filter the next test's list.
	search.clearQuery()
	selection.clear()
	document.body.innerHTML = ''
})

async function mountPanel() {
	panel = mount(PanelShell, { attachTo: document.body })
	// Let the mount pull, reconciliation and the post-nextTick restore settle.
	for (let i = 0; i < 6; i++) await new Promise((resolve) => setTimeout(resolve, 0))
	return panel as ReturnType<typeof mount<typeof PanelShell>>
}

describe('the grid structure', () => {
	it('is one grid spanning every section, with headers as rows', async () => {
		const wrapper = await mountPanel()

		// One composite widget, not one per section: a Shift range has to extend
		// across section boundaries.
		expect(wrapper.findAll('[role="grid"]')).toHaveLength(1)
		expect(wrapper.findAll('[role="rowgroup"]')).toHaveLength(2)

		// `grid` may own only row/rowgroup and `rowgroup` only row, so an <h2>
		// between rowgroups would violate aria-required-children.
		for (const rowgroup of wrapper.findAll('[role="rowgroup"]')) {
			for (const child of rowgroup.element.children) {
				expect(child.getAttribute('role')).toBe('row')
			}
		}

		for (const row of wrapper.findAll('[role="row"]')) {
			expect(row.element.children).toHaveLength(1)
			expect(row.element.children[0]?.getAttribute('role')).toBe('gridcell')
		}
	})

	it('labels each rowgroup by its section heading', async () => {
		const wrapper = await mountPanel()

		for (const rowgroup of wrapper.findAll('[role="rowgroup"]')) {
			const id = rowgroup.attributes('aria-labelledby')
			expect(id).toBeTruthy()
			expect(wrapper.find(`#${id}`).exists()).toBe(true)
		}
	})

	it('marks note rows selectable and header rows not', async () => {
		const wrapper = await mountPanel()

		const noteRows = wrapper.findAll('[data-row-id^="n:"]')
		expect(noteRows).toHaveLength(2)
		for (const row of noteRows) expect(row.attributes('aria-selected')).toBeDefined()

		for (const row of wrapper.findAll('[data-row-id^="s:"]')) {
			expect(row.attributes('aria-selected')).toBeUndefined()
		}
	})
})

describe('the roving tabindex', () => {
	it('leaves exactly one row and no descendant in the tab order', async () => {
		const wrapper = await mountPanel()

		const rows = wrapper.findAll('[data-row-id]')
		const tabbable = rows.filter((row) => row.attributes('tabindex') === '0')
		expect(tabbable).toHaveLength(1)

		// The one-Tab-stop claim only holds if every interactive descendant is out
		// of the tab order too.
		for (const button of wrapper.find('[role="grid"]').findAll('button')) {
			expect(button.attributes('tabindex')).toBe('-1')
		}
	})
})

describe('the composer', () => {
	it('reads its placeholder from the active space name', async () => {
		const wrapper = await mountPanel()

		expect(wrapper.find('#composer').attributes('placeholder')).toBe(
			'Add a note or a prompt (development)',
		)
	})

	it('is labelled and describes its own key bindings', async () => {
		const wrapper = await mountPanel()
		const composer = wrapper.find('#composer')

		expect(wrapper.find('label[for="composer"]').exists()).toBe(true)
		const describedBy = composer.attributes('aria-describedby')
		expect(wrapper.find(`#${describedBy}`).text()).toContain('Enter to add')
	})
})

describe('live regions', () => {
	it('pre-renders both, empty, so a later text change actually announces', async () => {
		const wrapper = await mountPanel()

		// Injecting the element and its text together does not announce.
		expect(wrapper.find('[role="alert"]').exists()).toBe(true)
		expect(wrapper.find('[role="status"]').exists()).toBe(true)
		expect(wrapper.find('[role="alert"]').text()).toBe('')
	})
})

describe('the conflict state', () => {
	it('keeps the editor mounted so its resolutions are reachable', async () => {
		const wrapper = await mountPanel()

		editor.beginEdit(SPACE, SPACE.notes[0]!)
		await wrapper.vm.$nextTick()
		expect(wrapper.find('textarea[aria-label="Edit note"]').exists()).toBe(true)

		// An external change lands under the draft.
		editor.reconcile(
			{ ...SPACE, notes: [{ ...SPACE.notes[0]!, body: 'someone else wrote this' }] },
			false,
		)
		await wrapper.vm.$nextTick()

		// Regression: the editor used to unmount here, taking the draft off screen
		// and leaving the conflict with no exit at all.
		expect(wrapper.find('textarea[aria-label="Edit note"]').exists()).toBe(true)
		const labels = wrapper.findAll('button').map((button) => button.text())
		expect(labels).toContain('Keep my version')
		expect(labels).toContain('Use the external version')

		editor.cancel()
	})
})

describe('code fences', () => {
	it('are not a second Tab stop inside the grid', async () => {
		const wrapper = await mountPanel()

		for (const pre of wrapper.find('[role="grid"]').findAll('pre')) {
			expect(pre.attributes('tabindex')).toBe('-1')
		}
	})
})

describe('row controls', () => {
	it('do not swallow keys the grid needs', async () => {
		const wrapper = await mountPanel()

		const circle = wrapper.find('button[aria-label="Mark as done"]')
		expect(circle.exists()).toBe(true)

		// A blanket `@keydown.stop` here meant Escape and Tab never reached the
		// grid handler, so interaction mode could not be left by keyboard. The
		// grid's own guard already early-returns for a button target, so stopping
		// propagation was redundant as well as harmful.
		let reachedGrid = false
		wrapper.find('[role="grid"]').element.addEventListener('keydown', () => {
			reachedGrid = true
		})
		await circle.trigger('keydown', { key: 'Escape' })

		expect(reachedGrid).toBe(true)
	})
})

describe('search', () => {
	async function typeQuery(wrapper: Awaited<ReturnType<typeof mountPanel>>, text: string) {
		await wrapper.find('#panel-search').setValue(text)
		// Several ticks: the filter reaches the list, the list re-renders, and
		// auto-animate's exit animation has to finish before a removed row is
		// actually out of the DOM.
		for (let i = 0; i < 4; i++) await new Promise((resolve) => setTimeout(resolve, 0))
	}

	it('filters to matching notes and drops sections with no match', async () => {
		const wrapper = await mountPanel()
		expect(wrapper.findAll('[role="rowgroup"]')).toHaveLength(2)

		await typeQuery(wrapper, 'first')

		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(1)
		expect(wrapper.find('[data-row-id="n:nte_1"]').exists()).toBe(true)
		// A section header only renders for a group that still has a match, so a
		// result's origin stays visible without a column of empty headings.
		expect(wrapper.findAll('[role="rowgroup"]')).toHaveLength(1)
	})

	it('filters both traversal orders, not just the note one', async () => {
		// Filtering only `visibleNoteIds` leaves ArrowDown stopping on header rows
		// of sections the list has removed from the DOM. Both come out of one walk
		// so they cannot disagree, and this is what says so.
		const wrapper = await mountPanel()
		await typeQuery(wrapper, 'first')

		const rendered = wrapper.findAll('[data-row-id]').map((row) => row.attributes('data-row-id'))
		expect(selection.rowIds.value).toEqual(rendered)
		expect(selection.visibleNoteIds.value).toEqual(['nte_1'])
	})

	it('renders the no-results state with a way out, and no empty grid', async () => {
		const wrapper = await mountPanel()
		await typeQuery(wrapper, 'zzzznothing')

		expect(wrapper.text()).toContain('No notes match “zzzznothing”.')
		// A `grid` with no row or rowgroup child fails aria-required-children.
		expect(wrapper.find('[role="grid"]').exists()).toBe(false)

		const clear = wrapper.findAll('button').find((button) => button.text() === 'Clear search')
		expect(clear).toBeTruthy()
		await clear!.trigger('click')
		expect(search.query.value).toBe('')
	})

	it('does not let a document change destroy the hidden half of a selection', async () => {
		// The regression: reconciliation pruned `selectedIds` against the
		// *filtered* order, treating "not on screen" as "does not exist". Any
		// document change landing while a query was active then silently dropped
		// every selected note the query happened to hide — the exact behaviour the
		// plan records as deliberately rejected, since a query is supposed to narrow
		// what an action targets, never the selection itself.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		selection.extendTo('nte_2')
		expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])

		await typeQuery(wrapper, 'first')
		expect(selection.visibleNoteIds.value).toEqual(['nte_1'])

		// Any applied document runs reconciliation.
		await space.refresh()
		for (let i = 0; i < 4; i++) await new Promise((resolve) => setTimeout(resolve, 0))

		search.clearQuery()
		await wrapper.vm.$nextTick()
		expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])
	})

	it('never changes the active section', async () => {
		const wrapper = await mountPanel()
		await typeQuery(wrapper, 'first')

		// A capture arriving mid-search still has to land where the composer says
		// it will.
		expect(mocks.invoke).not.toHaveBeenCalledWith('set_active_section', expect.anything())
		expect(wrapper.find('#composer').attributes('placeholder')).toContain('development')
	})
})

describe('the Escape ladder', () => {
	it('clears the query before the selection, and skips a rung with nothing to do', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await wrapper.find('#panel-search').setValue('first')
		await wrapper.vm.$nextTick()

		// One press, one level: the query goes and the selection survives.
		await wrapper.trigger('keydown', { key: 'Escape' })
		expect(search.query.value).toBe('')
		expect(selection.selectedIds.value).toEqual(['nte_1'])

		// The query rung now has nothing to do, so the press falls through to the
		// selection rather than being swallowed.
		await wrapper.trigger('keydown', { key: 'Escape' })
		expect(selection.selectedIds.value).toEqual([])
	})

	it('declines the press entirely while a menu is open', async () => {
		// The regression this exists for: the ladder assumed reka would have
		// `preventDefault`ed the press by the time the shell saw it. It does not —
		// reka listens on the window, and the shell is a DOM *ancestor* of the
		// portalled content, so the press arrives here first. Escape then closed
		// nothing and cleared the selection instead, and closing a submenu did both
		// at once.
		const wrapper = await mountPanel()
		selection.select('nte_1')

		await wrapper.find('[data-row-id="n:nte_1"]').trigger('contextmenu')
		for (let i = 0; i < 4; i++) await new Promise((resolve) => setTimeout(resolve, 0))

		const content = document.querySelector<HTMLElement>('[data-slot="context-menu-content"]')
		expect(content, 'the context menu did not open').not.toBeNull()

		// Dispatched from inside the menu, exactly as the real press is.
		content!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
		await wrapper.vm.$nextTick()

		// The menu owns the press; the selection is untouched.
		expect(selection.selectedIds.value).toEqual(['nte_1'])
	})

	it('leaves every other chord alone while a menu is open', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')

		await wrapper.find('[data-row-id="n:nte_1"]').trigger('contextmenu')
		for (let i = 0; i < 4; i++) await new Promise((resolve) => setTimeout(resolve, 0))

		const content = document.querySelector<HTMLElement>('[data-slot="context-menu-content"]')
		expect(content).not.toBeNull()

		// Delete typed at an open menu used to delete the notes *and* leave the
		// menu standing.
		content!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }))
		await wrapper.vm.$nextTick()

		expect(mocks.invoke).not.toHaveBeenCalledWith('delete_notes', expect.anything())
	})
})

describe('copy', () => {
	it('writes the targeted bodies through the Rust clipboard module', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')

		await wrapper.trigger('keydown', { key: 'c', ctrlKey: true })
		await wrapper.vm.$nextTick()

		expect(mocks.invoke).toHaveBeenCalledWith('clipboard_write_text', { text: 'first note' })
	})

	it('joins several notes with a blank line and confirms the count', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		selection.extendTo('nte_2')

		await wrapper.trigger('keydown', { key: 'c', ctrlKey: true })
		for (let i = 0; i < 3; i++) await new Promise((resolve) => setTimeout(resolve, 0))

		expect(mocks.invoke).toHaveBeenCalledWith('clipboard_write_text', {
			text: `first note\n\n${SPACE.notes[1]!.body}`,
		})
		// Singular and plural are separate whole strings, never `note(s)`.
		expect(wrapper.text()).toContain('Copied 2 notes')
	})

	it('leaves a live text selection to the native copy', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')

		const range = document.createRange()
		range.selectNodeContents(wrapper.find('.note-prose').element)
		window.getSelection()?.removeAllRanges()
		window.getSelection()?.addRange(range)

		await wrapper.trigger('keydown', { key: 'c', ctrlKey: true })
		await wrapper.vm.$nextTick()

		expect(mocks.invoke).not.toHaveBeenCalledWith('clipboard_write_text', expect.anything())
		window.getSelection()?.removeAllRanges()
	})
})

describe('mark as done', () => {
	it('applies to the whole selection as one store call', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		selection.extendTo('nte_2')

		await wrapper.find('[data-row-id="n:nte_2"]').trigger('keydown', { key: ' ' })
		await wrapper.vm.$nextTick()

		// One call, not one per note: five calls would be five snapshots and five
		// Ctrl+Z presses to undo.
		expect(mocks.invoke).toHaveBeenCalledWith('set_notes_done', {
			ids: ['nte_1', 'nte_2'],
			done: true,
		})
	})
})

describe('undo', () => {
	it('reports an empty stack rather than failing silently', async () => {
		const wrapper = await mountPanel()

		await wrapper.trigger('keydown', { key: 'z', ctrlKey: true })
		for (let i = 0; i < 3; i++) await new Promise((resolve) => setTimeout(resolve, 0))

		expect(wrapper.text()).toContain('Nothing to undo.')
	})

	it('is inert while a text surface has focus', async () => {
		const wrapper = await mountPanel()

		// Native text undo owns the composer, the inline editor and the search
		// field; omitting the third is what would let Ctrl+Z undo a note operation
		// mid-query.
		await wrapper.find('#panel-search').trigger('keydown', { key: 'z', ctrlKey: true })
		await wrapper.find('#composer').trigger('keydown', { key: 'z', ctrlKey: true })
		await wrapper.vm.$nextTick()

		expect(mocks.invoke).not.toHaveBeenCalledWith('undo')
	})
})

describe('axe', () => {
	it('reports no violations', async () => {
		await mountPanel()

		const results = await axe.run(document.body, {
			// Colour contrast needs a real layout and paint; it is verified by hand
			// over a black and a white desktop, because translucency shifts every
			// ratio with whatever is behind the panel.
			rules: { 'color-contrast': { enabled: false } },
		})

		expect(
			results.violations.map((violation) => `${violation.id}: ${violation.nodes.length} node(s)`),
		).toEqual([])
	}, 30_000)
})
