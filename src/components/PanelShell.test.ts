import { mount } from '@vue/test-utils'
import axe from 'axe-core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import PanelShell from './PanelShell.vue'
// Statically imported, like PanelShell itself: a dynamic import after
// `vi.resetModules()` would resolve a *second* instance of a module whose state
// is module-scoped by design, and the component tree would not share it.
import { useNoteEditor } from '@/composables/useNoteEditor'
import { useNoteSearch } from '@/composables/useNoteSearch'
import { useSections } from '@/composables/useSections'
import { useSelection } from '@/composables/useSelection'
import { useSpace } from '@/composables/useSpace'
import type { Space, StoreStatus } from '@/composables/useSpace'

const editor = useNoteEditor()
const search = useNoteSearch()
const sections = useSections()
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

/** The store as every test finds it. Named so a test that replaces it can put it
 *  back — see the teardown below. */
async function baseInvoke(command: string) {
	if (command === 'get_active_space') return SPACE
	if (command === 'get_status') return STATUS
	if (command === 'get_settings') {
		return { recents: [], activeSpace: 0, panelPosition: null, shortcuts: {}, theme: 'system' }
	}
	if (command === 'clipboard_write_text') return null
	if (command === 'hide_panel') return null
	if (command === 'editor_handoffs') return []
	if (command === 'set_notes_done') return SPACE
	if (command === 'set_active_section') return SPACE
	if (command === 'edit_note') return SPACE
	// `# Name` is classified in Rust, so the mock answers whatever the real command
	// would: this default is the ordinary-note case, and the two section outcomes
	// are set per test.
	if (command === 'submit_entry') {
		return { space: SPACE, outcome: 'note', noteId: 'nte_1', sectionId: 'sec_a' }
	}
	// An empty stack is `null`, not an error (task-003 §4.5).
	if (command === 'undo' || command === 'redo') return null
	throw { kind: 'invalid', message: command }
}

beforeEach(() => {
	vi.resetModules()
	mocks.invoke.mockReset()
	mocks.invoke.mockImplementation(baseInvoke)
})

/** The mounted panel, so it can be torn down rather than merely detached. */
let panel: ReturnType<typeof mount> | null = null

afterEach(async () => {
	// Unmounted, not just wiped from the DOM. Clearing `body.innerHTML` leaves the
	// app alive and re-rendering into detached nodes — and an open portalled menu
	// with it, which then shows up as content outside a landmark in the axe run
	// several tests later.
	panel?.unmount()
	panel = null

	editor.cancel()
	// Module-scoped by design, so it outlives the component tree exactly as it
	// does in the app. A query left behind would filter the next test's list, and
	// a collapsed section would empty it.
	search.clearQuery()
	sections.reset()
	selection.clear()

	// The *document* is module-scoped too, and `initialize()` is memoised — so a
	// second mount does not re-pull it and a test that installed a different one
	// hands it to every test after it. Restoring here rather than asking each such
	// test to remember is the difference between one line and a class of failures
	// that only show up in file order.
	mocks.invoke.mockImplementation(baseInvoke)
	await space.refresh()

	document.body.innerHTML = ''
})

/**
 * Lets the chained work of an applied document finish: the pull, reconciliation,
 * the post-`nextTick` DOM restore, the re-render, and auto-animate's exit
 * animation — which only takes a removed row back out of the DOM on `finish`.
 *
 * A macrotask per turn rather than `nextTick`, because several of those steps
 * are promises chained behind an `invoke`, and a flush does not reach the end of
 * them.
 */
async function settle(turns = 4) {
	for (let i = 0; i < turns; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

async function mountPanel() {
	panel = mount(PanelShell, { attachTo: document.body })
	await settle(6)
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

	it('is labelled, and spends no panel height on a key-binding hint', async () => {
		const wrapper = await mountPanel()
		const composer = wrapper.find('#composer')

		expect(wrapper.find('label[for="composer"]').exists()).toBe(true)
		// Task-004 shipped a permanent "Enter to add · Shift+Enter for newline"
		// line under the field and this overrides it: the panel is a keyboard-first
		// tool for its own author, and a standing row of text restating the most
		// standard chord in text entry costs height on every launch to teach
		// nothing twice.
		expect(composer.attributes('aria-describedby')).toBeUndefined()
		expect(wrapper.text()).not.toContain('Enter to add')
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
		await settle()
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
		await settle()

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
		// Neither press reached the last rung, which is the point of skipping a
		// level with nothing to do rather than consuming the press.
		expect(mocks.invoke).not.toHaveBeenCalledWith('hide_panel')
	})

	it('dismisses the panel once every rung above it has nothing to do', async () => {
		const wrapper = await mountPanel()

		await wrapper.trigger('keydown', { key: 'Escape' })

		expect(mocks.invoke).toHaveBeenCalledWith('hide_panel')
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
		await settle()

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
		await settle()

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
		await settle(3)

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
		await settle(3)

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

describe('the composer submit', () => {
	async function submit(wrapper: Awaited<ReturnType<typeof mountPanel>>, text: string) {
		const composer = wrapper.find('#composer')
		await composer.setValue(text)
		await composer.trigger('keydown', { key: 'Enter' })
		await settle(3)
	}

	it('sends the body verbatim and classifies nothing itself', async () => {
		const wrapper = await mountPanel()

		// Every one of these is a note or a directive by a rule that lives in Rust.
		// The frontend must not have a second copy of it, so what it sends is what
		// was typed — including the leading backslash of the escape hatch, which
		// Rust consumes rather than the composer.
		for (const body of ['# Research', '## Research', '#Research', '\\# Research', '  x  ']) {
			await submit(wrapper, body)
			expect(mocks.invoke).toHaveBeenCalledWith('submit_entry', { body })
		}

		expect(mocks.invoke).not.toHaveBeenCalledWith('add_note', expect.anything())
		expect(mocks.invoke).not.toHaveBeenCalledWith('add_section', expect.anything())
	})

	it('clears and keeps focus on a directive, without moving the roving target', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_2')
		mocks.invoke.mockImplementationOnce(async () => ({
			space: SPACE,
			outcome: 'section-created',
			noteId: null,
			sectionId: 'sec_new',
		}))

		await submit(wrapper, '# Research')

		expect((wrapper.find('#composer').element as HTMLTextAreaElement).value).toBe('')
		expect(document.activeElement?.id).toBe('composer')
		// No note was created, so nothing takes the roving focus. A `noteId` acted on
		// here would point the grid at a row that does not exist.
		expect(selection.focusedId.value).toBe('n:nte_2')
	})

	it('leaves an inline editor commit opaque — `# Name` stays a note body', async () => {
		const wrapper = await mountPanel()

		editor.beginEdit(SPACE, SPACE.notes[0]!)
		await wrapper.vm.$nextTick()
		const field = wrapper.find('textarea[aria-label="Edit note"]')
		await field.setValue('# Research')
		await field.trigger('keydown', { key: 'Enter', ctrlKey: true })
		await settle(3)

		// Editing a body must never be able to delete the note being edited.
		expect(mocks.invoke).toHaveBeenCalledWith('edit_note', { id: 'nte_1', body: '# Research' })
		expect(mocks.invoke).not.toHaveBeenCalledWith('submit_entry', expect.anything())
	})
})

describe('the active-section chip', () => {
	it('names the active section without touching the placeholder', async () => {
		const wrapper = await mountPanel()
		const chip = wrapper.find('[data-slot="dropdown-menu-trigger"][title]')

		expect(chip.text()).toContain('Research')
		// Task-004 acceptance criterion 3 stands: the placeholder names the *space*.
		expect(wrapper.find('#composer').attributes('placeholder')).toBe(
			'Add a note or a prompt (development)',
		)
	})

	it('carries the full name in a title, so a truncated one is still readable', async () => {
		const wrapper = await mountPanel()
		const long = 'A section name long enough to need an ellipsis in a 390px panel'
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'get_active_space') {
				return { ...SPACE, sections: [{ id: 'sec_a', name: long, order: 0 }] }
			}
			if (command === 'get_status') return STATUS
			if (command === 'editor_handoffs') return []
			throw { kind: 'invalid', message: command }
		})
		await space.refresh()
		await settle(3)

		const chip = wrapper.find('[data-slot="dropdown-menu-trigger"][title]')
		expect(chip.attributes('title')).toBe(long)
		expect(chip.find('.truncate').exists()).toBe(true)
		// It updates when the active section changes — that is the whole reason it
		// exists, since the header it duplicates scrolls out of view.
		expect(chip.text()).toContain(long)
	})
})

describe('the section switcher', () => {
	function content() {
		return document.querySelector<HTMLElement>('[data-slot="dropdown-menu-content"]')
	}

	async function openWithChord(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
		const composer = wrapper.find('#composer')
		;(composer.element as HTMLTextAreaElement).focus()
		await composer.trigger('keydown', { key: 'k', ctrlKey: true })
		await settle(3)
	}

	it('opens from the composer, which is the documented suppression exception', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		expect(sections.switcherOpen.value).toBe(true)
		// The filter takes focus on open: reka's own open-focus would land on the
		// first item, and typing is the point of the surface.
		expect(document.activeElement?.id).toBe('section-filter')
		// Every section of the active space, whatever the list is showing.
		const names = [...(content()?.querySelectorAll('[role="menuitem"]') ?? [])].map((item) =>
			item.textContent?.trim(),
		)
		expect(names).toHaveLength(2)
		expect(names[0]).toContain('Research')
		expect(names[1]).toContain('Inbox')
		// Marked with colour *and* a non-colour cue.
		expect(content()?.querySelector('[aria-current="true"]')?.textContent).toContain(
			'(active section)',
		)
	})

	it('stays suppressed in the search field and the inline editor', async () => {
		const wrapper = await mountPanel()

		await wrapper.find('#panel-search').trigger('keydown', { key: 'k', ctrlKey: true })
		await settle()
		expect(sections.switcherOpen.value).toBe(false)

		editor.beginEdit(SPACE, SPACE.notes[0]!)
		await wrapper.vm.$nextTick()
		await wrapper
			.find('textarea[aria-label="Edit note"]')
			.trigger('keydown', { key: 'k', ctrlKey: true })
		await settle()
		expect(sections.switcherOpen.value).toBe(false)
	})

	it('activates a section and gives the composer back its text and its caret', async () => {
		const wrapper = await mountPanel()
		const composer = wrapper.find('#composer').element as HTMLTextAreaElement
		await wrapper.find('#composer').setValue('half a thought')
		composer.focus()
		composer.setSelectionRange(4, 4)

		await openWithChord(wrapper)
		const inbox = [...(content()?.querySelectorAll('[role="menuitem"]') ?? [])].find((item) =>
			item.textContent?.includes('Inbox'),
		)
		expect(inbox, 'the Inbox row is missing').toBeTruthy()
		;(inbox as HTMLElement).click()
		await settle(4)

		expect(mocks.invoke).toHaveBeenCalledWith('set_active_section', { id: 'sec_b' })
		expect(sections.switcherOpen.value).toBe(false)
		// Switching a destination must cost nothing: not the half-typed line, and
		// not the position in it.
		expect(composer.value).toBe('half a thought')
		expect(document.activeElement).toBe(composer)
		expect(composer.selectionStart).toBe(4)
	})

	it('filters, and offers to create what nothing matches', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		const filter = content()!.querySelector<HTMLInputElement>('#section-filter')!
		filter.value = 'inb'
		filter.dispatchEvent(new Event('input', { bubbles: true }))
		await settle()

		expect(content()?.querySelectorAll('[role="menuitem"]')).toHaveLength(1)
		expect(content()?.textContent).toContain('Inbox')

		filter.value = 'Reading'
		filter.dispatchEvent(new Event('input', { bubbles: true }))
		await settle()

		const rows = [...content()!.querySelectorAll('[role="menuitem"]')]
		expect(rows).toHaveLength(1)
		expect(rows[0]?.textContent).toContain('Create section “Reading”')

		;(rows[0] as HTMLElement).click()
		await settle(4)

		// The *same* path as the `# Name` directive, so the duplicate-name rule, the
		// whitespace collapsing and the length cap are inherited rather than copied.
		expect(mocks.invoke).toHaveBeenCalledWith('submit_entry', { body: '# Reading' })
	})

	it('closes on Escape without taking a rung of the ladder with it', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await wrapper.find('#panel-search').setValue('first')
		await settle()
		await openWithChord(wrapper)
		expect(sections.switcherOpen.value).toBe(true)

		content()!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
		await settle(3)

		expect(sections.switcherOpen.value).toBe(false)
		// The press resolves at the overlay guard and never reaches the ladder, so
		// neither the query nor the selection moves.
		expect(search.query.value).toBe('first')
		expect(selection.selectedIds.value).toEqual(['nte_1'])
	})

	it('is reachable from the overflow menu as well as from the chord', async () => {
		const wrapper = await mountPanel()

		await wrapper.find('[aria-label="More actions"]').trigger('pointerdown', { button: 0 })
		await wrapper.find('[aria-label="More actions"]').trigger('click')
		await settle(3)

		const menu = document.querySelector('[data-slot="dropdown-menu-content"]')
		expect(menu?.textContent).toContain('Switch section')
	})

	it('reports no axe violations while open', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)
		expect(content(), 'the switcher did not open').not.toBeNull()

		// Scoped to the switcher, which is what the criterion asks about. A
		// whole-document run while any reka menu is up reports the library's own
		// modal behaviour — it marks the rest of the tree `aria-hidden`, and every
		// focusable element left behind it becomes an `aria-hidden-focus` node —
		// plus a `region` finding for portalled content sitting outside the
		// landmarks. Neither is this component's, and neither is actionable here.
		const results = await axe.run(content()!, {
			rules: {
				// Colour contrast needs a real layout and paint; verified by hand, as
				// the whole-panel run above records.
				'color-contrast': { enabled: false },
				// **Disabled with a known, singular cause, not to make the test pass.**
				// The task specifies a reka dropdown whose contents include a filter
				// field, and `role="menu"` may not own a textbox — axe flattens
				// `group` wrappers inside menus, so no markup resolves it. The one
				// node it reports is reka's own content element. Asserted below rather
				// than assumed, so this exclusion cannot quietly start hiding a
				// second, genuine finding.
				'aria-required-children': { enabled: false },
			},
		})

		expect(
			results.violations.map((violation) => `${violation.id}: ${violation.nodes.length} node(s)`),
		).toEqual([])

		// The excluded rule, run on its own: exactly one node, and it is the menu
		// container. Anything else here is a real regression.
		const known = await axe.run(content()!, { runOnly: ['aria-required-children'] })
		expect(known.violations.flatMap((violation) => violation.nodes)).toHaveLength(1)
		expect(known.violations[0]?.nodes[0]?.html).toContain('role="menu"')
	}, 30_000)
})

describe('collapsible sections', () => {
	function disclosure(wrapper: Awaited<ReturnType<typeof mountPanel>>, name: string) {
		return wrapper.find(
			`button[aria-label="Collapse ${name}"], button[aria-label="Expand ${name}"]`,
		)
	}

	it('folds a section away on a click and brings it back', async () => {
		const wrapper = await mountPanel()
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(2)

		await disclosure(wrapper, 'Research').trigger('click')
		await settle(3)

		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(0)
		// The header stays: it is the control that brings the section back, and it
		// is still where a capture lands.
		expect(wrapper.find('[data-row-id="s:sec_a"]').exists()).toBe(true)
		expect(disclosure(wrapper, 'Research').attributes('aria-expanded')).toBe('false')
		// Not the empty-section line — the notes are folded away, not absent.
		expect(wrapper.text()).not.toContain('No notes in this section yet.')

		await disclosure(wrapper, 'Research').trigger('click')
		await settle(3)
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(2)
	})

	it('collapses with ArrowLeft and expands with ArrowRight on the header row', async () => {
		const wrapper = await mountPanel()
		const header = wrapper.find('[data-row-id="s:sec_a"]')

		await header.trigger('keydown', { key: 'ArrowLeft' })
		await settle(3)
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(0)

		// Explicit rather than a toggle, so holding the key cannot flap.
		await wrapper.find('[data-row-id="s:sec_a"]').trigger('keydown', { key: 'ArrowLeft' })
		await settle()
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(0)

		await wrapper.find('[data-row-id="s:sec_a"]').trigger('keydown', { key: 'ArrowRight' })
		await settle(3)
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(2)
	})

	it('keeps both traversal orders agreeing with the DOM', async () => {
		// The same invariant the search filter has to hold: a roving `tabindex="0"`
		// left on an unmounted row makes the grid unreachable by Tab.
		const wrapper = await mountPanel()
		await disclosure(wrapper, 'Research').trigger('click')
		await settle(3)

		const rendered = wrapper.findAll('[data-row-id]').map((row) => row.attributes('data-row-id'))
		expect(selection.rowIds.value).toEqual(rendered)
		expect(selection.visibleNoteIds.value).toEqual([])
		expect(rendered).toContain(selection.focusedId.value)
		expect(wrapper.findAll('[data-row-id][tabindex="0"]')).toHaveLength(1)
	})

	it('never destroys a selection it merely folded away', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		selection.extendTo('nte_2')

		await disclosure(wrapper, 'Research').trigger('click')
		await settle(3)
		// Any applied document runs reconciliation, which prunes against the whole
		// document rather than against what is on screen.
		await space.refresh()
		await settle()

		expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])
	})

	it('is overridden by a search and restored when the query clears', async () => {
		const wrapper = await mountPanel()
		await disclosure(wrapper, 'Research').trigger('click')
		await settle(3)
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(0)

		await wrapper.find('#panel-search').setValue('first')
		await settle(3)

		// A matching note is never hidden by a collapse.
		expect(wrapper.find('[data-row-id="n:nte_1"]').exists()).toBe(true)
		// And the control withdraws rather than sitting there doing nothing.
		expect(disclosure(wrapper, 'Research').exists()).toBe(false)

		search.clearQuery()
		await settle(3)
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(0)
	})

	it('auto-expands the section a new note lands in', async () => {
		const wrapper = await mountPanel()
		await disclosure(wrapper, 'Research').trigger('click')
		await settle(3)
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(0)

		// A capture landing behind a fold is the one outcome a tool whose promise is
		// "capture is silent on success" cannot afford.
		const captured = {
			...SPACE,
			notes: [
				...SPACE.notes,
				{
					id: 'nte_3',
					section: 'sec_a',
					order: 2,
					done: false,
					body: 'captured',
					created: '2026-08-06T00:00:00Z',
					updated: '2026-08-06T00:00:00Z',
				},
			],
		}
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'get_active_space') return captured
			if (command === 'get_status') return STATUS
			if (command === 'editor_handoffs') return []
			throw { kind: 'invalid', message: command }
		})
		await space.refresh()
		await settle(3)

		expect(wrapper.find('[data-row-id="n:nte_3"]').exists()).toBe(true)
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(3)
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
