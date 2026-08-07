import { mount } from '@vue/test-utils'
import axe from 'axe-core'
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import PanelShell from './PanelShell.vue'
// Statically imported, like PanelShell itself: a dynamic import after
// `vi.resetModules()` would resolve a *second* instance of a module whose state
// is module-scoped by design, and the component tree would not share it.
import { useAttachments, type Attachment } from '@/composables/useAttachments'
import { useInteractionMode } from '@/composables/useInteractionMode'
import { useNoteActions } from '@/composables/useNoteActions'
import { useNoteEditor } from '@/composables/useNoteEditor'
import { useNoteSearch } from '@/composables/useNoteSearch'
import { useSections } from '@/composables/useSections'
import { noteRow, takeRow, useSelection } from '@/composables/useSelection'
import { useSpace } from '@/composables/useSpace'
import type { Space, StoreStatus } from '@/composables/useSpace'

const actions = useNoteActions()
const interaction = useInteractionMode()
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
//
// Torn down again below. `restoreMocks` does not reach a plain assignment to a
// host prototype, so a stub left in place would outlive this file and hand every
// later suite in the worker a fake WAAPI they never asked for — which is exactly
// the kind of environment difference that makes one suite pass only when another
// ran first.
const elementPrototype = Element.prototype as unknown as Record<string, unknown>
const stubbedAnimate = elementPrototype.animate === undefined
if (stubbedAnimate) {
	elementPrototype.animate = () => {
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
}

afterAll(() => {
	if (stubbedAnimate) Reflect.deleteProperty(elementPrototype, 'animate')
})

const mocks = vi.hoisted(() => ({
	invoke: vi.fn(),
	openUrl: vi.fn(),
	/** `DropTarget`'s own listener, held so a test can hand it an OS drag event.
	 *  Boxed rather than a bare binding: the `vi.mock` factory below closes over
	 *  this object, and reassigning a hoisted binding from inside it would not be
	 *  seen out here. */
	dragDrop: { deliver: null as ((payload: unknown) => unknown) | null },
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
// `emit` is here for the capture notice the shell mounts, which signals frontend
// readiness once its listeners resolve.
vi.mock('@tauri-apps/api/event', () => ({
	listen: async () => () => {},
	emit: async () => {},
}))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: mocks.openUrl }))
// The drop target subscribes on mount. `getCurrentWebview` reaches into
// `window.__TAURI_INTERNALS__` for the current window's label, which does not
// exist outside the real webview — so the whole module is stubbed rather than
// the internals faked, which would be a second, worse copy of Tauri's shape.
vi.mock('@tauri-apps/api/webview', () => ({
	getCurrentWebview: () => ({
		onDragDropEvent: async (handler: (payload: unknown) => unknown) => {
			mocks.dragDrop.deliver = handler
			// Dropped on unmount as the real unlisten is, so a test that fires a drop
			// cannot reach a component the previous test tore down.
			return () => {
				mocks.dragDrop.deliver = null
			}
		},
	}),
}))

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

/** Hands the store a different document and re-pulls it. Deliberately narrower
 *  than `baseInvoke`: nothing else should be reachable while it is installed.
 *  The teardown puts `baseInvoke` back. */
async function installDocument(next: Space) {
	mocks.invoke.mockImplementation(async (command: string) => {
		if (command === 'get_active_space') return next
		if (command === 'get_status') return STATUS
		if (command === 'editor_handoffs') return []
		throw { kind: 'invalid', message: command }
	})
	await space.refresh()
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

describe('the header mark', () => {
	/**
	 * Two properties that are invisible in a screenshot and easy to undo.
	 *
	 * Tauri reads `data-tauri-drag-region` off the element the mousedown actually
	 * lands on, so wrapping the glyph in a span — the obvious thing to do when
	 * someone next restyles it — would leave the mark looking identical and
	 * dragging nothing. And it is branding rather than a control: a tab stop here
	 * would put a dead target in front of the search field, which is the first
	 * thing the panel's keyboard flow reaches.
	 */
	it('is the drag handle itself and takes no focus', async () => {
		const wrapper = await mountPanel()
		// A descendant selector, so the header's own drag region is not what this
		// finds.
		const mark = wrapper.find('header [data-tauri-drag-region]')

		expect(mark.exists()).toBe(true)
		expect(mark.element.children).toHaveLength(0)
		expect(mark.text()).toBe('c')
		expect(mark.attributes('tabindex')).toBeUndefined()
		expect(mark.element.tagName).not.toBe('BUTTON')
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
			expect(mocks.invoke).toHaveBeenCalledWith('submit_entry', { body, attachments: [] })
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
		await installDocument({ ...SPACE, sections: [{ id: 'sec_a', name: long, order: 0 }] })
		await settle(3)

		const chip = wrapper.find('[data-slot="dropdown-menu-trigger"][title]')
		expect(chip.attributes('title')).toBe(long)
		expect(chip.find('.truncate').exists()).toBe(true)
		// It updates when the active section changes — that is the whole reason it
		// exists, since the header it duplicates scrolls out of view.
		expect(chip.text()).toContain(long)
	})
})

describe('the New section field', () => {
	async function openField(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
		await wrapper.find('[aria-label="More actions"]').trigger('click')
		await settle(3)

		const item = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((row) =>
			row.textContent?.includes('New section…'),
		)
		expect(item, 'the New section item is missing').toBeTruthy()
		item!.click()
		await settle(3)

		const field = document.querySelector<HTMLInputElement>('#new-section-name')
		expect(field, 'the New section field did not open').not.toBeNull()
		return field!
	}

	it('refuses a name the store would resolve to an existing section', async () => {
		const wrapper = await mountPanel()
		await installDocument({
			...SPACE,
			sections: [{ id: 'sec_a', name: 'Deep Research', order: 0 }],
		})
		await settle(3)

		const field = await openField(wrapper)
		// Two spaces. The store collapses whitespace before deciding which names
		// collide, so this *is* the existing section — validating on the raw text
		// let it through and produced a store-level collision instead of an answer
		// the user could act on while the field is still open.
		field.value = 'Deep  Research'
		field.dispatchEvent(new Event('input', { bubbles: true }))
		await settle()
		field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
		await settle(3)

		expect(document.querySelector('#new-section-error')?.textContent).toContain(
			'This space already has a section with that name.',
		)
		expect(mocks.invoke).not.toHaveBeenCalledWith('add_section', expect.anything())
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
		expect(mocks.invoke).toHaveBeenCalledWith('submit_entry', {
			body: '# Reading',
			attachments: [],
		})
	})

	it('matches on the name the store will keep, not the one that was typed', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		const filter = content()!.querySelector<HTMLInputElement>('#section-filter')!
		// Two spaces. The store collapses them, so this *is* the existing section —
		// filtering on the raw text offered to create it and then silently activated
		// the existing one, which is a create button that never creates.
		filter.value = 'Deep  Research'
		filter.dispatchEvent(new Event('input', { bubbles: true }))
		await settle()
		expect(content()?.textContent).toContain('Create section “Deep Research”')

		// Padding is normalised away too, so this resolves to the existing section
		// and offers to create nothing. On the raw text it did not: `"research"`
		// does not contain `" research "`.
		filter.value = '  Research  '
		filter.dispatchEvent(new Event('input', { bubbles: true }))
		await settle()
		expect(content()?.textContent).not.toContain('Create section')

		// And the row promises the normalised name, so what it says is what gets
		// stored.
		filter.value = '  Deep   Reading  '
		filter.dispatchEvent(new Event('input', { bubbles: true }))
		await settle()
		const row = content()!.querySelector<HTMLElement>('[data-create-row]')!
		expect(row.textContent).toContain('Create section “Deep Reading”')
		row.click()
		await settle(4)
		expect(mocks.invoke).toHaveBeenCalledWith('submit_entry', {
			body: '# Deep Reading',
			attachments: [],
		})
	})

	it('activates the row reka has highlighted, not always the first', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		// Reka highlights on hover while the filter keeps focus, so Enter has to
		// resolve that row rather than the top of the list.
		const rows = [...content()!.querySelectorAll<HTMLElement>('[role="menuitem"]')]
		const inbox = rows.find((row) => row.textContent?.includes('Inbox'))!
		inbox.setAttribute('data-highlighted', '')

		const filter = content()!.querySelector<HTMLInputElement>('#section-filter')!
		filter.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
		await settle(4)

		expect(mocks.invoke).toHaveBeenCalledWith('set_active_section', { id: 'sec_b' })
	})

	it('gives ArrowLeft to the caret unless the caret is already at the start', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		const filter = content()!.querySelector<HTMLInputElement>('#section-filter')!
		filter.value = 'res'
		filter.dispatchEvent(new Event('input', { bubbles: true }))
		await settle()

		// Mid-text it is the caret key and must not reach reka, which would close a
		// submenu out from under someone editing their query.
		filter.setSelectionRange(2, 2)
		let reached = false
		const listen = () => (reached = true)
		content()!.addEventListener('keydown', listen)
		filter.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }))
		expect(reached).toBe(false)

		// At 0/0 there is nothing to move over, so the press belongs to the menu.
		filter.setSelectionRange(0, 0)
		filter.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }))
		expect(reached).toBe(true)
		content()!.removeEventListener('keydown', listen)
	})

	it('runs the same lifecycle from the overflow menu, so no filter survives it', async () => {
		const wrapper = await mountPanel()

		await wrapper.find('[aria-label="More actions"]').trigger('click')
		await settle(3)
		expect(
			document.querySelector('[data-slot="dropdown-menu-sub-trigger"]'),
			'the Switch section submenu trigger is missing',
		).not.toBeNull()

		// Driven through the shared state rather than by hovering the trigger, which
		// is what proves the binding is *controlled*: an uncontrolled submenu would
		// ignore this entirely. That is the whole fix — it is what lets the filter be
		// cleared on every open and close, and an epoch change close it.
		sections.openSwitcher('menu')
		await settle(4)

		const sub = document.querySelector<HTMLElement>('[data-slot="dropdown-menu-sub-content"]')
		expect(sub, 'the submenu did not follow the shared open state').not.toBeNull()
		// The same list component the chip hosts, not a second copy of it.
		expect(sub!.querySelector('#section-filter')).not.toBeNull()
		expect(sub!.textContent).toContain('Research')

		sections.filterQuery.value = 'zzz'
		sections.closeSwitcher('menu')
		await settle(3)

		expect(sections.filterQuery.value).toBe('')
		expect(document.querySelector('[data-slot="dropdown-menu-sub-content"]')).toBeNull()
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

	it('keeps a folded-away selection actionable', async () => {
		// Collapse is folding, not deselection. Targeting the collapse-filtered order
		// turned copy, delete, mark-done, merge, Move to and the $EDITOR handoff into
		// silent no-ops the moment a section was folded — with no status message to
		// say so.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		selection.extendTo('nte_2')

		await disclosure(wrapper, 'Research').trigger('click')
		await settle(3)

		// The roving target moved to the header, so `focusedNoteId` is null — which
		// must not defeat a multi-select either.
		expect(selection.focusedNoteId.value).toBeNull()

		await wrapper.trigger('keydown', { key: 'c', ctrlKey: true })
		await settle(3)

		expect(mocks.invoke).toHaveBeenCalledWith('clipboard_write_text', {
			text: `first note\n\n${SPACE.notes[1]!.body}`,
		})
		expect(wrapper.text()).toContain('Copied 2 notes')
	})

	it('leaves the grid a tab stop when notes move into a collapsed section', async () => {
		const wrapper = await mountPanel()
		await disclosure(wrapper, 'Inbox').trigger('click')
		await settle(3)

		selection.select('nte_1')
		// The document `move_notes` returns, with the note actually in the collapsed
		// destination — which is what leaves its row unrendered.
		const moved = {
			...SPACE,
			notes: [{ ...SPACE.notes[0]!, section: 'sec_b' }, SPACE.notes[1]!],
		}
		mocks.invoke.mockImplementationOnce(async () => moved)
		await actions.moveTo('sec_b')
		await settle(3)

		// The moved note has no row — a move deliberately does not auto-expand its
		// destination, because that destination was chosen rather than arrived at —
		// so focus lands on the destination's header instead of a key naming nothing.
		expect(selection.focusedId.value).toBe('s:sec_b')
		expect(wrapper.findAll('[data-row-id][tabindex="0"]')).toHaveLength(1)
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
		await installDocument(captured)
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

describe('attachments', () => {
	const PNG: Attachment = {
		id: 'att_1',
		file: '3f9a1c0e7b2d5481.png',
		name: 'Screenshot 2026-08-04 141233.png',
		mime: 'image/png',
		bytes: 184_320,
		width: 1280,
		height: 720,
	}
	const PDF: Attachment = {
		id: 'att_2',
		file: 'c1d40ab97e6f2235.pdf',
		name: 'spec.pdf',
		mime: 'application/pdf',
		bytes: 2048,
	}

	const attachments = useAttachments()

	/** A one-pixel PNG's worth of bytes. Only the length matters — the component
	 *  hands it to `URL.createObjectURL`, which happy-dom stubs. */
	const THUMB_BYTES = new Uint8Array([0x89, 0x50, 0x4e, 0x47]).buffer

	/**
	 * `baseInvoke` plus the attachment surface. Written as an override rather
	 * than a replacement so a test that only cares about pasting still gets the
	 * document, the status and the settings.
	 */
	let attachmentOverrides: Record<string, unknown> = {}

	function withAttachmentCommands(overrides: Record<string, unknown> = {}) {
		// Merged rather than replaced: `installWithAttachments` calls this again to
		// swap the document, and a replacement would silently drop the preview
		// behaviour the test had just set up — leaving every card in the state the
		// default produces and the assertion failing for the wrong reason.
		attachmentOverrides = { ...attachmentOverrides, ...overrides }
		const active = attachmentOverrides
		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
			if (command in active) {
				const value = active[command]
				return typeof value === 'function' ? value(args) : value
			}
			// The default for every preview: the blob is there and has no picture.
			// A test that wants a thumbnail or a missing file overrides it.
			if (command === 'attachment_thumb') return new ArrayBuffer(0)
			if (command === 'attach_paste' || command === 'attach_pick' || command === 'attach_paths') {
				return []
			}
			if (command === 'attachment_open') return null
			return baseInvoke(command)
		})
	}

	/** A document whose first note carries `files`. */
	function documentWith(files: Attachment[]): Space {
		return {
			...SPACE,
			notes: SPACE.notes.map((note, index) =>
				index === 0 ? { ...note, attachments: files } : note,
			),
		}
	}

	/**
	 * `installDocument`, but with the attachment surface still answering.
	 *
	 * The shared helper deliberately narrows the mock to three commands so that
	 * nothing else is reachable while it is installed — which for these tests
	 * would make every preview request throw and render every card unavailable,
	 * including the ones that are supposed to work.
	 */
	async function installWithAttachments(next: Space, overrides: Record<string, unknown> = {}) {
		withAttachmentCommands({ ...overrides, get_active_space: next })
		await space.refresh()
		await settle(3)
	}

	async function composerPaste(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
		await wrapper.find('#composer').trigger('paste')
		await settle(3)
	}

	/** An OS file drop, delivered to the listener `DropTarget` registered on
	 *  mount — the third ingest path, and the only one with no DOM event behind
	 *  it. */
	async function dropFiles(paths: string[]) {
		expect(mocks.dragDrop.deliver, 'DropTarget registered no drag listener').not.toBeNull()
		await mocks.dragDrop.deliver?.({ payload: { type: 'drop', paths } })
		await settle(3)
	}

	/**
	 * Cleared going *in* rather than coming out, and the difference is not
	 * stylistic.
	 *
	 * A nested `afterEach` runs before the outer one, so it fires while the panel
	 * is still mounted — and `clearPreviews` bumps the epoch every mounted card
	 * watches, so every one of them asks again on the spot. Those requests land
	 * on whatever mock the teardown has installed by then and refill the cache
	 * with answers from the wrong test, so the next test finds a cached preview
	 * and never calls the command it is asserting on.
	 */
	beforeEach(() => {
		attachments.clearPending()
		attachments.clearPreviews()
		attachmentOverrides = {}
	})

	// --- the pending tray ---

	/** AC1. */
	it('shows a pasted attachment in the tray with its name, size and count chip', async () => {
		withAttachmentCommands({ attach_paste: [PNG] })
		const wrapper = await mountPanel()

		await composerPaste(wrapper)

		const tray = wrapper.find('[aria-label="Add a note"]')
		expect(tray.text()).toContain('Attached 1 file')
		expect(tray.text()).toContain(PNG.name)
		expect(tray.text()).toContain('180 KB')
	})

	it('pluralises the chip and removes one item at a time', async () => {
		withAttachmentCommands({ attach_paste: [PNG, PDF] })
		const wrapper = await mountPanel()

		await composerPaste(wrapper)
		expect(wrapper.text()).toContain('Attached 2 files')

		await wrapper.find(`[aria-label="Remove ${PDF.name}"]`).trigger('click')
		await settle(1)

		expect(wrapper.text()).toContain('Attached 1 file')
		expect(wrapper.text()).not.toContain(PDF.name)
	})

	/** AC4. Rust answers with an empty list when the clipboard carries text, and
	 *  the composer must then leave the native paste alone rather than treating
	 *  the empty answer as a failure. */
	it('creates no attachment when the clipboard carries text', async () => {
		withAttachmentCommands({ attach_paste: [] })
		const wrapper = await mountPanel()

		await composerPaste(wrapper)

		expect(mocks.invoke).toHaveBeenCalledWith('attach_paste')
		expect(wrapper.text()).not.toContain('Attached')
	})

	/** AC11. A refusal is reported by name on the composer's own error surface,
	 *  and it does not empty a tray that already has something in it. */
	it('reports a refused file without losing what is already attached', async () => {
		withAttachmentCommands({
			attach_paste: [PNG],
			attach_pick: () => {
				throw { kind: 'invalid', message: 'huge.bin is 12.0 MB and the limit is 10.0 MB' }
			},
		})
		const wrapper = await mountPanel()
		await composerPaste(wrapper)

		await wrapper.find('[aria-label="Attach files"]').trigger('click')
		await settle(3)

		expect(wrapper.text()).toContain('huge.bin is 12.0 MB')
		expect(wrapper.text()).toContain('Attached 1 file')
	})

	/**
	 * The reported bug. A refused file left its message on the composer, and the
	 * next attach — which worked — landed in the tray underneath it, because
	 * `report` only ever added and `onInput` was the only thing that took a
	 * message away. So the message survived until the user happened to type.
	 *
	 * Asserted per ingest path rather than once, because the clearing lives at
	 * each entry point: `useAttachments` deliberately holds no opinion about
	 * error surfaces, so there is no single place downstream that could cover all
	 * three at once.
	 */
	describe('a refusal does not outlive the next attach that succeeds', () => {
		const REFUSED = 'empty.md is empty'

		/** Leaves the composer showing a refusal, through the picker. */
		async function refuse(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
			withAttachmentCommands({
				attach_pick: () => {
					throw { kind: 'invalid', message: REFUSED }
				},
			})
			await wrapper.find('[aria-label="Attach files"]').trigger('click')
			await settle(3)
			expect(wrapper.text()).toContain(REFUSED)
		}

		/**
		 * The message is gone and the file arrived — and the field is still empty,
		 * which is the half that distinguishes the fix from the old behaviour:
		 * nothing was typed, so `onInput` never ran.
		 */
		function expectRetired(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
			expect(wrapper.text()).not.toContain(REFUSED)
			expect(wrapper.text()).toContain('Attached 1 file')
			expect((wrapper.find('#composer').element as HTMLTextAreaElement).value).toBe('')
		}

		it('through the picker', async () => {
			const wrapper = await mountPanel()
			await refuse(wrapper)

			withAttachmentCommands({ attach_pick: [PNG] })
			await wrapper.find('[aria-label="Attach files"]').trigger('click')
			await settle(3)

			expectRetired(wrapper)
		})

		it('through a paste', async () => {
			const wrapper = await mountPanel()
			await refuse(wrapper)

			withAttachmentCommands({ attach_paste: [PNG] })
			await composerPaste(wrapper)

			expectRetired(wrapper)
		})

		it('through a drop', async () => {
			const wrapper = await mountPanel()
			await refuse(wrapper)

			withAttachmentCommands({ attach_paths: [PNG] })
			await dropFiles(['C:\\shot.png'])

			expectRetired(wrapper)
		})
	})

	/** AC2. The tray is emptied only after the store accepted the submission,
	 *  and its contents travel with the body. */
	it('submits the pending attachments with the note and then clears the tray', async () => {
		withAttachmentCommands({ attach_paste: [PNG, PDF] })
		const wrapper = await mountPanel()
		await composerPaste(wrapper)

		const composer = wrapper.find('#composer')
		await composer.setValue('with two files')
		await composer.trigger('keydown', { key: 'Enter' })
		await settle(4)

		expect(mocks.invoke).toHaveBeenCalledWith('submit_entry', {
			body: 'with two files',
			attachments: [PNG, PDF],
		})
		expect(wrapper.text()).not.toContain('Attached')
	})

	it('keeps the tray when the submission fails', async () => {
		withAttachmentCommands({
			attach_paste: [PNG],
			submit_entry: () => {
				throw { kind: 'io', message: 'the space could not be written' }
			},
		})
		const wrapper = await mountPanel()
		await composerPaste(wrapper)

		const composer = wrapper.find('#composer')
		await composer.setValue('will not land')
		await composer.trigger('keydown', { key: 'Enter' })
		await settle(4)

		// Clearing here would drop the only reference to a blob the sweep will
		// eventually collect, which is the one way this surface can lose a file.
		expect(wrapper.text()).toContain('Attached 1 file')
		expect(wrapper.text()).toContain('the space could not be written')
		expect((composer.element as HTMLTextAreaElement).value).toBe('will not land')
	})

	// --- rendering in a note ---

	/** AC7. */
	it('renders an image as a preview card and a pdf as a file chip', async () => {
		withAttachmentCommands({
			attachment_thumb: (args?: Record<string, unknown>) =>
				args?.file === PNG.file ? THUMB_BYTES : new ArrayBuffer(0),
		})
		await mountPanel()
		await installWithAttachments(documentWith([PNG, PDF]))

		const cards = document.querySelectorAll<HTMLElement>(
			'[data-note-row] button[aria-label^="Open"]',
		)
		expect(cards).toHaveLength(2)
		expect(cards[0]?.textContent).toContain(PNG.name)
		expect(cards[0]?.querySelector('img')).not.toBeNull()
		// A file with no preview is not a broken image: it renders a glyph and
		// stays enabled, because the blob is there.
		expect(cards[1]?.textContent).toContain(PDF.name)
		expect(cards[1]?.querySelector('img')).toBeNull()
		expect(cards[1]?.hasAttribute('disabled')).toBe(false)
	})

	/** AC8. A missing blob says so, and the rest of the note still renders. */
	it('renders a missing attachment as unavailable with its cause', async () => {
		withAttachmentCommands({
			attachment_thumb: () => {
				throw { kind: 'not-found', message: 'could not read 3f9a1c0e7b2d5481.png' }
			},
		})
		await mountPanel()
		await installWithAttachments(documentWith([PNG]))

		const card = document.querySelector<HTMLElement>(
			'[data-note-row] button[aria-label*="unavailable"]',
		)
		expect(card).not.toBeNull()
		expect(card?.textContent).toContain('could not read 3f9a1c0e7b2d5481.png')
		expect(card?.hasAttribute('disabled')).toBe(true)
		// The note itself is untouched.
		expect(document.body.textContent).toContain('first note')
	})

	/** One request per content hash, however many cards point at it — the same
	 *  screenshot on two notes is one blob and one preview. */
	it('requests a preview once per content hash', async () => {
		withAttachmentCommands()
		await mountPanel()
		const twice: Space = {
			...SPACE,
			notes: SPACE.notes.map((note) => ({ ...note, attachments: [PNG] })),
		}
		await installWithAttachments(twice)

		const requests = mocks.invoke.mock.calls.filter(([command]) => command === 'attachment_thumb')
		expect(requests).toHaveLength(1)
	})

	/** AC17's keyboard half. Cards are tabbable only inside task-004's
	 *  interaction mode, like every other in-card control. */
	it('makes cards tabbable only in interaction mode, and opens on Enter', async () => {
		withAttachmentCommands()
		await mountPanel()
		await installWithAttachments(documentWith([PDF]))

		const card = document.querySelector<HTMLElement>('button[aria-label^="Open"]')
		expect(card?.getAttribute('tabindex')).toBe('-1')

		const row = document.querySelector<HTMLElement>('[data-note-row]')
		row?.focus()
		interaction.enter(row!.dataset.rowId!)
		await settle(2)
		expect(card?.getAttribute('tabindex')).toBe('0')

		card?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
		await settle(2)
		expect(mocks.invoke).toHaveBeenCalledWith('attachment_open', { file: PDF.file })
	})

	it('opens on double-click and not on a single click', async () => {
		withAttachmentCommands()
		await mountPanel()
		await installWithAttachments(documentWith([PNG]))

		const card = document.querySelector<HTMLElement>('button[aria-label^="Open"]')
		card?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
		await settle(2)
		expect(mocks.invoke).not.toHaveBeenCalledWith('attachment_open', expect.anything())

		card?.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))
		await settle(2)
		expect(mocks.invoke).toHaveBeenCalledWith('attachment_open', { file: PNG.file })
	})

	/** AC20. `Copy` and `Copy as list` are about bodies; a local file path means
	 *  nothing to whatever the text is pasted into. */
	it('copies body text only, byte-identically to a note without attachments', async () => {
		withAttachmentCommands()
		await mountPanel()

		await installWithAttachments(SPACE)
		selection.select('nte_1')
		await actions.copyNotes()
		const withoutFiles = mocks.invoke.mock.calls.filter(
			([command]) => command === 'clipboard_write_text',
		)

		await installWithAttachments(documentWith([PNG, PDF]))
		selection.select('nte_1')
		await actions.copyNotes()
		const all = mocks.invoke.mock.calls.filter(([command]) => command === 'clipboard_write_text')

		expect(all).toHaveLength(withoutFiles.length + 1)
		expect(all.at(-1)?.[1]).toEqual(withoutFiles.at(-1)?.[1])
		expect(JSON.stringify(all.at(-1)?.[1])).not.toContain(PNG.file)
	})

	/** AC16. The panel is 390 × 660 and fixed, so nothing added inside a note row
	 *  may widen the document — a filename is one long unbreakable token, which
	 *  is exactly the shape that does. */
	it('does not let a long filename widen the document', async () => {
		withAttachmentCommands()
		await mountPanel()
		await installWithAttachments(
			documentWith([{ ...PNG, name: `${'unbreakable-filename'.repeat(20)}.png` }]),
		)

		// happy-dom lays nothing out and reports zero for every box, so this
		// asserts the *mechanism* rather than the pixels. The filename has to
		// truncate, and every flex ancestor between it and the note row has to be
		// allowed below its content width — a flex item defaults to
		// `min-width: auto`, which is what lets one unbreakable token push the grid
		// wider than the panel and make the document scroll sideways.
		const name = [...document.querySelectorAll<HTMLElement>('[data-note-row] span')].find(
			(element) =>
				element.textContent?.trim().startsWith('unbreakable-filename') &&
				element.children.length === 0,
		)
		expect(name?.className).toContain('truncate')

		const row = document.querySelector<HTMLElement>('[data-note-row]')
		expect(row).not.toBeNull()
		for (let element = name?.parentElement; element && element !== row;) {
			expect(
				element.className,
				`${element.tagName}.${element.className} can be pushed wider than the panel`,
			).toMatch(/min-w-0|shrink-0/)
			element = element.parentElement
		}
	})

	// --- the context menu ---

	it('names the attachment action after what it will do, and disables it with none', async () => {
		withAttachmentCommands()
		await mountPanel()

		await installWithAttachments(SPACE)
		selection.select('nte_1')
		takeRow(noteRow('nte_1'))
		await settle(1)
		expect(actions.canOpenAttachment.value).toBe(false)

		await installWithAttachments(documentWith([PDF]))
		selection.select('nte_1')
		takeRow(noteRow('nte_1'))
		await settle(1)
		expect(actions.canOpenAttachment.value).toBe(true)
		// A non-image is revealed, never launched.
		expect(actions.attachmentActionLabel.value).toBe('Reveal in Explorer')

		await installWithAttachments(documentWith([PNG]))
		selection.select('nte_1')
		takeRow(noteRow('nte_1'))
		await settle(1)
		expect(actions.attachmentActionLabel.value).toBe('Open Attachment')
	})

	// --- switching space ---

	/** A pending attachment's blob lives in the *previous* space's assets
	 *  directory, so carrying the tray across a switch would show files that
	 *  cannot be attached and fail only when the user pressed Enter. */
	it('empties the pending tray when the space identity changes', async () => {
		withAttachmentCommands({ attach_paste: [PNG] })
		const wrapper = await mountPanel()
		await composerPaste(wrapper)
		expect(wrapper.text()).toContain('Attached 1 file')

		await installWithAttachments({ ...SPACE, id: 'spc_other' })

		expect(wrapper.text()).not.toContain('Attached')
	})

	/**
	 * A response for the previous space must not publish into the new one's
	 * cache.
	 *
	 * The old space's blob is genuinely absent from the new space's assets
	 * directory, so what a late response writes is `missing` — and the card it
	 * marks unavailable is a perfectly present attachment, which stays that way
	 * until the next switch.
	 */
	it('discards a preview response issued before a space switch', async () => {
		let release: ((value: ArrayBuffer) => void) | undefined
		withAttachmentCommands({
			attachment_thumb: () =>
				new Promise<ArrayBuffer>((resolve) => {
					release = resolve
				}),
		})
		await mountPanel()
		await installWithAttachments(documentWith([PNG]))

		const thumbRequests = () =>
			mocks.invoke.mock.calls.filter(([command]) => command === 'attachment_thumb').length

		// The request is in flight; the switch revokes the cache under it.
		expect(release).toBeDefined()
		expect(thumbRequests()).toBe(1)
		attachments.clearPreviews()
		release?.(THUMB_BYTES)
		await settle(3)

		// It published nothing — not stuck on a stale answer about a space nobody
		// is looking at.
		expect(attachments.previewFor(PNG.file).state).toBe('loading')
		// And "back to asking" is asserted rather than inferred from the state
		// above, which a card that had simply given up would also report. The
		// revoke bumps the epoch the card watches, so it issues a second request.
		expect(thumbRequests()).toBe(2)
	})

	/** Two hundred notes carrying ten attachments each is two thousand image
	 *  decodes if nothing bounds them. The ceiling that matters is not the cost
	 *  of one decode but how many are asked for at once. */
	it('bounds how many previews are decoding at once', async () => {
		let inFlight = 0
		let peak = 0
		const finish: (() => void)[] = []
		withAttachmentCommands({
			attachment_thumb: () => {
				inFlight++
				peak = Math.max(peak, inFlight)
				return new Promise<ArrayBuffer>((resolve) => {
					finish.push(() => {
						inFlight--
						resolve(new ArrayBuffer(0))
					})
				})
			},
		})
		await mountPanel()

		const many = Array.from({ length: 40 }, (_, index) => ({
			...PDF,
			id: `att_${index}`,
			file: `${index.toString(16).padStart(16, '0')}.pdf`,
		}))
		await installWithAttachments(documentWith(many))

		expect(peak).toBeGreaterThan(0)
		expect(peak).toBeLessThanOrEqual(4)

		// The queue drains rather than stalling: releasing what is in flight lets
		// the rest through.
		while (finish.length > 0) {
			finish.splice(0).forEach((done) => done())
			await settle(1)
		}
		expect(
			mocks.invoke.mock.calls.filter(([command]) => command === 'attachment_thumb'),
		).toHaveLength(many.length)
	})

	/** AC17's axe half, over a note carrying attachments and a populated tray at
	 *  the same time. */
	it('reports no axe violations with a populated tray and a note carrying files', async () => {
		withAttachmentCommands({ attach_paste: [PNG, PDF] })
		const wrapper = await mountPanel()
		await installWithAttachments(documentWith([PNG, PDF]), { attach_paste: [PNG, PDF] })
		await composerPaste(wrapper)

		const results = await axe.run(document.body, {
			rules: { 'color-contrast': { enabled: false } },
		})

		expect(
			results.violations.map((violation) => `${violation.id}: ${violation.nodes.length} node(s)`),
		).toEqual([])
	}, 30_000)
})
