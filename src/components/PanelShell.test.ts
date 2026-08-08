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
import { useNoteDrag } from '@/composables/useNoteDrag'
import { useNoteEditor } from '@/composables/useNoteEditor'
import { useNoteList } from '@/composables/useNoteList'
import { useNoteSearch } from '@/composables/useNoteSearch'
import { useSections } from '@/composables/useSections'
import { flushReveal, noteRow, sectionRow, takeRow, useSelection } from '@/composables/useSelection'
import { useImageViewer } from '@/composables/useImageViewer'
import { useSettings } from '@/composables/useSettings'
import { useSpace } from '@/composables/useSpace'
import { useSpaces } from '@/composables/useSpaces'
import { useStatusMessage } from '@/composables/useStatusMessage'
import type { Space, StoreStatus } from '@/composables/useSpace'

const actions = useNoteActions()
const drag = useNoteDrag()
const interaction = useInteractionMode()
const editor = useNoteEditor()
const list = useNoteList()
const search = useNoteSearch()
const sections = useSections()
const selection = useSelection()
const settings = useSettings()
const viewer = useImageViewer()
const space = useSpace()
const spaces = useSpaces()
const status = useStatusMessage()

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

/** `SPACE` after `merge_notes` over both of `sec_a`'s notes: one note, keeping
 *  the first id, in the section they were already in. */
const MERGED: Space = {
	...SPACE,
	notes: [
		{
			...SPACE.notes[0]!,
			body: `${SPACE.notes[0]!.body}\n\n${SPACE.notes[1]!.body}`,
		},
	],
}

const SHORTCUTS = {
	capture: 'Shift Shift',
	summon: 'Ctrl+Shift+Space',
	defaults: { capture: 'Shift Shift', summon: 'Ctrl+Shift+Space' },
	summonRegistered: true,
	summonError: null,
	captureRegistered: true,
	captureError: null,
	captureFallback: null,
}

/**
 * What `get_settings` answers, as a mutable object so a test can change a
 * preference and re-pull.
 *
 * `useSettings` is initialised by `App`, not by this component, so a test that
 * needs a non-default preference has to ask for the refresh itself — see the
 * double-click cases.
 */
let settingsPayload: Record<string, unknown> = {}

function defaultSettings() {
	return {
		recents: [],
		activeSpace: 0,
		panelPosition: null,
		shortcuts: {},
		theme: 'system',
		sounds: false,
		motion: 'auto',
		insertionPoint: 'bottom',
		doubleClick: 'copy',
		alwaysOnTop: true,
		showCreated: false,
	}
}

/** The store as every test finds it. Named so a test that replaces it can put it
 *  back — see the teardown below. */
async function baseInvoke(command: string) {
	if (command === 'get_active_space') return SPACE
	if (command === 'get_status') return STATUS
	if (command === 'get_settings') return settingsPayload
	if (command === 'get_shortcut_state') return SHORTCUTS
	if (command === 'get_autostart_enabled') return false
	if (command === 'clipboard_write_text') return null
	// Task-013's zero-focus paste. The text branch is a capture, so it reaches
	// `add_note` rather than `submit_entry`; the other branch asks `attach_paste`
	// what the clipboard holds, and an empty list is its "there was text, or
	// nothing" answer.
	if (command === 'add_note') return { space: SPACE, noteId: 'nte_1' }
	if (command === 'attach_paste') return []
	// The two notes of `sec_a` become one, keeping the first id — which is what
	// `merge_notes` does, and what makes the survivor's row disappear when that
	// section happens to be collapsed.
	if (command === 'merge_notes') return MERGED
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
	// `restoreMocks` reaches spies, not the call history of a bare `vi.fn()` — so
	// without this a link-click assertion counts the previous test's clicks too.
	mocks.openUrl.mockClear()
	settingsPayload = defaultSettings()
	mocks.invoke.mockImplementation(baseInvoke)
	// **`all`, not the panel's default.** `SPACE`'s second note is done, and the
	// default view hides done notes — so every case in this file that is about
	// something else (the grid, the menus, copy, drag, the editor) would be
	// asserting against a one-note list for a reason it never mentions. The done
	// filter's own block puts the default back and is where the three states are
	// exercised.
	list.setDoneFilter('all')
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
	// Module-scoped for the same reason, and with two ways to break the next test:
	// a done filter left on empties its list, and a sort left set withdraws every
	// drag handle it goes looking for.
	list.reset()
	selection.clear()
	// Interaction mode belongs on that list for the same reason and was missing
	// from it: with a row still in the mode, the grid's key handler declines every
	// press but Tab, so a later test's arrow keys move nothing and fail with focus
	// simply sitting where it started.
	interaction.exit()
	// And so does the image viewer, whose overlay would otherwise still be up in
	// the next test — declining every chord and swallowing the Escape ladder.
	viewer.close()
	// The action-error band is module-scoped for the same reason everything above
	// it is. A message left standing takes the status line's place, so the next
	// test's "Copied 2 notes" is simply not on screen.
	space.clearActionError('list')
	// And so is the toast, which now outlives the action that wrote it by five
	// seconds — so without this a message from one test is still on screen in the
	// next, and its timer is still pending when the worker tears down.
	status.clear()

	// The *document* is module-scoped too, and `initialize()` is memoised — so a
	// second mount does not re-pull it and a test that installed a different one
	// hands it to every test after it. Restoring here rather than asking each such
	// test to remember is the difference between one line and a class of failures
	// that only show up in file order.
	settingsPayload = defaultSettings()
	mocks.invoke.mockImplementation(baseInvoke)
	await space.refresh()
	// `useSettings` is module-scoped too, and a test that stored a non-default
	// preference would otherwise hand it to every test after it.
	await settings.refresh()

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

	/**
	 * One ring per row, and the pair this stops is the one a user hits by accident.
	 *
	 * The selection ring is inset and the focus ring used to be pushed out to
	 * `-outline-offset-4` so both could be seen at once. `:focus-visible` does not
	 * match a row focused by the click that selected it — but the next keypress
	 * re-evaluates that, so pressing Shift alone drew a second outline inside the
	 * first, around a row nothing had happened to.
	 */
	it('draws no focus ring on a row that is already showing the selection ring', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await settle(2)

		const selected = wrapper.get(`[data-row-id="${noteRow('nte_1')}"]`)
		expect(selected.classes()).toContain('ring-accent-ring')
		expect(selected.classes('focus-ring')).toBe(false)

		// The unselected row keeps it: that is the case where focus and selection
		// genuinely differ, and the only ring it can wear.
		const other = wrapper.get(`[data-row-id="${noteRow('nte_2')}"]`)
		expect(other.classes()).toContain('focus-ring')
	})
})

describe('Ctrl+Arrow', () => {
	/**
	 * The missing half of discontiguous keyboard selection. `Ctrl+Space` toggles
	 * the focused note without disturbing the rest, but every way of *reaching*
	 * another note replaced the selection on arrival — so the two could not be
	 * combined and the discontiguous case was pointer-only.
	 */
	it('moves the roving focus without changing the selection, and composes with Ctrl+Space', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await settle(2)

		await wrapper
			.get(`[data-row-id="${noteRow('nte_1')}"]`)
			.trigger('keydown', { key: 'ArrowDown', ctrlKey: true })
		await settle(2)

		expect(selection.focusedId.value).toBe(noteRow('nte_2'))
		expect(selection.selectedIds.value).toEqual(['nte_1'])

		await wrapper
			.get(`[data-row-id="${noteRow('nte_2')}"]`)
			.trigger('keydown', { key: ' ', ctrlKey: true })
		await settle(2)

		expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])
	})

	it('leaves the plain form selecting, and lets Shift win when both are held', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await settle(2)

		await wrapper
			.get(`[data-row-id="${noteRow('nte_1')}"]`)
			.trigger('keydown', { key: 'ArrowDown' })
		await settle(2)
		expect(selection.selectedIds.value).toEqual(['nte_2'])

		// Ctrl+Shift+Arrow extends. A focus-only move would have left the selection
		// at `nte_2` alone, so the range is what proves which branch ran.
		await wrapper
			.get(`[data-row-id="${noteRow('nte_2')}"]`)
			.trigger('keydown', { key: 'ArrowUp', shiftKey: true, ctrlKey: true })
		await settle(2)
		expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])
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

describe('note links', () => {
	const HREF = 'https://example.com/docs'

	/** Mounted first and handed the document after, as the other cases here do. */
	async function linkInPanel() {
		const wrapper = await mountPanel()
		await installDocument({
			...SPACE,
			notes: [{ ...SPACE.notes[0]!, body: `see [the docs](${HREF})` }],
		})
		await settle(3)

		const link = wrapper.find('.note-prose a[href]')
		expect(link.attributes('href')).toBe(HREF)
		return link
	}

	/**
	 * `preventDefault` is half the guarantee, not a detail: `openUrl` on its own
	 * would open the page in the browser *and* navigate the panel to it, which
	 * replaces the app with a web page and has no way back.
	 */
	it('opens a clicked link in the OS browser instead of navigating the WebView', async () => {
		const link = await linkInPanel()

		const event = new MouseEvent('click', { bubbles: true, cancelable: true })
		link.element.dispatchEvent(event)

		expect(mocks.openUrl).toHaveBeenCalledWith(HREF)
		expect(event.defaultPrevented).toBe(true)
	})

	// Middle-click fires `auxclick` and reaches no `click` handler at all, so this
	// path was uncovered while the plain and Ctrl-clicks were handled.
	it('routes a middle-click the same way', async () => {
		const link = await linkInPanel()

		const event = new MouseEvent('auxclick', { button: 1, bubbles: true, cancelable: true })
		link.element.dispatchEvent(event)

		expect(mocks.openUrl).toHaveBeenCalledWith(HREF)
		expect(event.defaultPrevented).toBe(true)
	})

	it('leaves the right button to the context menu', async () => {
		const link = await linkInPanel()

		const event = new MouseEvent('auxclick', { button: 2, bubbles: true, cancelable: true })
		link.element.dispatchEvent(event)

		expect(mocks.openUrl).not.toHaveBeenCalled()
		expect(event.defaultPrevented).toBe(false)
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

describe('the header drag region', () => {
	/**
	 * The property is invisible in a screenshot and easy to undo.
	 *
	 * Tauri reads `data-tauri-drag-region` off the element the mousedown actually
	 * lands on. The header used to delegate that to Copper's `c` mark, because the
	 * field and the two buttons left it almost no bare area of its own; with the
	 * mark gone the header's own padding is the whole grab handle, so the attribute
	 * has to be on the header and nothing inside it may claim the same role and
	 * quietly become the only draggable pixel again.
	 */
	it('is the header itself, with no control standing in for it', async () => {
		const wrapper = await mountPanel()
		const header = wrapper.get('header')

		expect(header.attributes('data-tauri-drag-region')).toBeDefined()
		// A descendant selector, so this is anything *inside* the header claiming to
		// be the drag handle.
		expect(wrapper.find('header [data-tauri-drag-region]').exists()).toBe(false)
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

	/** Task-014. The query is a character sequence now, so a match no longer has
	 *  to be a word the note contains. */
	it('matches characters spread across the body', async () => {
		const wrapper = await mountPanel()
		await typeQuery(wrapper, 'fnote')

		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(1)
		expect(wrapper.find('[data-row-id="n:nte_1"]').exists()).toBe(true)
	})

	/**
	 * Task-014's ranking, and the two halves of the decision it records: the best
	 * match rises **inside** its section, and the sections themselves do not move.
	 */
	it('ranks matches within a section without reordering the sections', async () => {
		const ranked: Space = {
			...SPACE,
			notes: [
				// Written so the three scores are unambiguous: a scattered match, a
				// contiguous one that does not start a word, and a contiguous one that
				// does. The last is the highest of the three and is in the *second*
				// section, which is what makes this test about ordering rather than
				// about scoring.
				{
					...SPACE.notes[0]!,
					id: 'nte_1',
					section: 'sec_a',
					body: 'silently reordering the arguments',
				},
				{ ...SPACE.notes[0]!, id: 'nte_2', section: 'sec_a', body: 'a resort' },
				{ ...SPACE.notes[0]!, id: 'nte_3', section: 'sec_b', body: 'sort by date' },
			],
		}
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'get_active_space') return ranked
			return baseInvoke(command)
		})
		const wrapper = await mountPanel()
		await space.refresh()
		await settle(2)

		await typeQuery(wrapper, 'sort')

		// `sec_a` holds the two notes it held before, with the tighter match first —
		// and `sec_b`, whose only match scores highest of the three, has not jumped
		// above it.
		expect(selection.rowIds.value).toEqual(['s:sec_a', 'n:nte_2', 'n:nte_1', 's:sec_b', 'n:nte_3'])
	})
})

describe('the always-on-top pin', () => {
	/** The header control and the settings row are the same state, so the header
	 *  has to read the pulled value rather than a local default. */
	async function mountWith(alwaysOnTop: boolean) {
		settingsPayload = { ...defaultSettings(), alwaysOnTop }
		const wrapper = await mountPanel()
		await settings.refresh()
		await settle(2)
		return wrapper
	}

	it('shows the stored state and toggles it through the Rust command', async () => {
		const wrapper = await mountWith(true)

		const pin = wrapper.find('[aria-label="Keep on top: on"]')
		expect(pin.exists()).toBe(true)
		expect(pin.attributes('aria-pressed')).toBe('true')

		settingsPayload = { ...defaultSettings(), alwaysOnTop: false }
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'set_always_on_top') return settingsPayload
			return baseInvoke(command)
		})

		await pin.trigger('click')
		await settle(2)

		// Its own command rather than the generic settings patch: this preference has
		// a native side, and Rust applies the band before persisting it.
		expect(mocks.invoke).toHaveBeenCalledWith('set_always_on_top', { enabled: false })
		expect(wrapper.find('[aria-label="Keep on top: off"]').attributes('aria-pressed')).toBe('false')
	})

	it('renders unpinned from a settings file that says so', async () => {
		const wrapper = await mountWith(false)

		expect(wrapper.find('[aria-label="Keep on top: off"]').exists()).toBe(true)
	})

	/**
	 * The settings row renders its failure inline, beside the control. This one is
	 * a 32-pixel button in a header with no such slot, so a refused write would
	 * flip nothing and explain nothing — the user would be left pressing a pin that
	 * does not stick. It borrows the panel's one error band instead.
	 */
	it('reports a refused toggle in the status band rather than silently', async () => {
		const wrapper = await mountWith(true)
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'set_always_on_top') {
				throw {
					kind: 'persist',
					message: "Copper couldn't save the always-on-top setting: disk full",
				}
			}
			return baseInvoke(command)
		})

		await wrapper.find('[aria-label="Keep on top: on"]').trigger('click')
		await settle(3)

		// Rust's own sentence, not one invented here: it names which half failed.
		expect(wrapper.text()).toContain("Copper couldn't save the always-on-top setting: disk full")
		// And the control still shows the state that is actually live.
		expect(wrapper.find('[aria-label="Keep on top: on"]').exists()).toBe(true)
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

	/**
	 * Task-014 ranks a section's rows by score; `actionableNoteIds` is what an
	 * *action* targets and its contract is the document's order. Letting the
	 * ranking reach it would make a multi-note copy come out in whatever order the
	 * query happened to score them, which is a silent change to the clipboard's
	 * contents for a search the user has since cleared.
	 */
	it('copies in document order even while a search has reordered the rows', async () => {
		const ranked: Space = {
			...SPACE,
			notes: [
				{ ...SPACE.notes[0]!, id: 'nte_1', section: 'sec_a', body: 'a resort' },
				{ ...SPACE.notes[0]!, id: 'nte_2', section: 'sec_a', body: 'sort by date' },
			],
		}
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'get_active_space') return ranked
			return baseInvoke(command)
		})
		const wrapper = await mountPanel()
		await space.refresh()
		await settle(2)

		await wrapper.find('#panel-search').setValue('sort')
		await settle()

		// The rows really are ranked, so this is not vacuous.
		expect(selection.rowIds.value).toEqual(['s:sec_a', 'n:nte_2', 'n:nte_1'])
		// ...and the order an action sees is not.
		expect(selection.actionableNoteIds.value).toEqual(['nte_1', 'nte_2'])

		selection.selectAll()
		await actions.copyNotes()

		expect(mocks.invoke).toHaveBeenCalledWith('clipboard_write_text', {
			text: 'a resort\n\nsort by date',
		})
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

/**
 * The two Enter matrices, which are deliberate inverses of each other: the
 * composer is a capture line where the most frequent action must not cost a
 * chord, and a note body is a document where a newline must not.
 */
describe('the Enter matrix', () => {
	it('submits the composer on a bare Enter and leaves both modified forms to the field', async () => {
		const wrapper = await mountPanel()
		const composer = wrapper.find('#composer')
		await composer.setValue('captured')

		for (const modifier of [{ shiftKey: true }, { ctrlKey: true }]) {
			await composer.trigger('keydown', { key: 'Enter', ...modifier })
			await settle(2)
		}
		// Neither one submits, and neither is prevented — the newline is Chromium's
		// `InsertNewline`, which is what keeps the field's own undo stack intact.
		expect(mocks.invoke).not.toHaveBeenCalledWith('submit_entry', expect.anything())

		await composer.trigger('keydown', { key: 'Enter' })
		await settle(3)
		expect(mocks.invoke).toHaveBeenCalledWith('submit_entry', {
			body: 'captured',
			attachments: [],
		})
	})

	it('leaves both bare and Shift+Enter to the field inside the inline editor', async () => {
		const wrapper = await mountPanel()
		editor.beginEdit(SPACE, SPACE.notes[0]!)
		await wrapper.vm.$nextTick()

		const field = wrapper.find('textarea[aria-label="Edit note"]')
		await field.setValue('first line')
		await field.trigger('keydown', { key: 'Enter' })
		await field.trigger('keydown', { key: 'Enter', shiftKey: true })
		await settle(3)

		expect(mocks.invoke).not.toHaveBeenCalledWith('edit_note', expect.anything())
		// Still open: a newline is not a save, so the session survives both presses.
		expect(editor.session.value).not.toBeNull()
	})

	/**
	 * Ctrl+Enter is two things by context — `CHORDS.openInEditor` starts the
	 * `$EDITOR` handoff from a focused card — and inside the editor it may only be
	 * one of them. The press is stopped at the textarea rather than left to the
	 * shell's text-surface guard, which `Ctrl+K` has already been made an exception
	 * to once.
	 */
	it('saves on Ctrl+Enter without also starting the external handoff', async () => {
		const wrapper = await mountPanel()
		editor.beginEdit(SPACE, SPACE.notes[0]!)
		await wrapper.vm.$nextTick()

		const field = wrapper.find('textarea[aria-label="Edit note"]')
		await field.setValue('edited body')
		await field.trigger('keydown', { key: 'Enter', ctrlKey: true })
		await settle(3)

		expect(mocks.invoke).toHaveBeenCalledWith('edit_note', { id: 'nte_1', body: 'edited body' })
		expect(mocks.invoke).not.toHaveBeenCalledWith('editor_open_note', expect.anything())
	})

	/**
	 * The conflict card's buttons are inside the editor and are not a text surface,
	 * so a Ctrl+Enter from one of them reaches the shell's chord layer — and a
	 * handoff forked off an uncommitted draft is a second writer over the same
	 * body, which is what the conflict state exists to prevent.
	 */
	it('declines the handoff chord for the note the inline editor is holding', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		editor.beginEdit(SPACE, SPACE.notes[0]!)
		await settle(2)

		await wrapper.trigger('keydown', { key: 'Enter', ctrlKey: true })
		await settle(3)

		expect(mocks.invoke).not.toHaveBeenCalledWith('editor_open_note', expect.anything())
		expect(wrapper.text()).toContain('Finish the inline edit first')
	})
})

describe('focus after a delete', () => {
	/** Three notes in one section, so "the next one" and "the previous one" name
	 *  different rows. `nte_3` is the done one, for the sweep below. */
	const THREE: Space = {
		...SPACE,
		notes: ['nte_1', 'nte_2', 'nte_3'].map((id, order) => ({
			id,
			section: 'sec_a',
			order,
			done: id === 'nte_3',
			body: id,
			created: '2026-08-05T00:00:00Z',
			updated: '2026-08-05T00:00:00Z',
		})),
	}

	/** The explicit `space.refresh()` is not ceremony — `initialize()` is memoised,
	 *  so mounting after installing a different store does not re-pull it. */
	async function mountWithThree() {
		mocks.invoke.mockImplementation(async (command: string, args?: { ids?: string[] }) => {
			if (command === 'get_active_space') return THREE
			if (command === 'delete_notes') {
				return { ...THREE, notes: THREE.notes.filter((note) => !args?.ids?.includes(note.id)) }
			}
			return baseInvoke(command)
		})
		const wrapper = await mountPanel()
		await space.refresh()
		await settle(3)
		return wrapper
	}

	function rowElementOf(wrapper: Awaited<ReturnType<typeof mountPanel>>, id: string) {
		return wrapper.get(`[data-row-id="${noteRow(id)}"]`).element
	}

	/** The keyboard path, where the row genuinely holds DOM focus — which is what
	 *  `takeRow` supplies and a bare `select` does not. */
	async function focusRowAndDelete(wrapper: Awaited<ReturnType<typeof mountPanel>>, id: string) {
		selection.select(id)
		takeRow(noteRow(id))
		await settle(2)
		expect(document.activeElement).toBe(rowElementOf(wrapper, id))

		await wrapper.get(`[data-row-id="${noteRow(id)}"]`).trigger('keydown', { key: 'Delete' })
		await settle(5)
	}

	it('lands on the next note in document order', async () => {
		const wrapper = await mountWithThree()
		await focusRowAndDelete(wrapper, 'nte_2')

		expect(selection.focusedId.value).toBe(noteRow('nte_3'))
		expect(document.activeElement).toBe(rowElementOf(wrapper, 'nte_3'))
	})

	it('falls back to the previous note when the deleted one was last', async () => {
		const wrapper = await mountWithThree()
		await focusRowAndDelete(wrapper, 'nte_3')

		expect(selection.focusedId.value).toBe(noteRow('nte_2'))
		expect(document.activeElement).toBe(rowElementOf(wrapper, 'nte_2'))
	})

	/**
	 * The path that had no focus at all. A delete from the context menu has DOM
	 * focus inside the portalled menu rather than on the row, so `restoreDom` —
	 * which moves focus only when the element that *had* it is gone — correctly
	 * decides nothing was lost, and the grid was left with its roving
	 * `tabindex="0"` on a row nothing was focused on.
	 */
	it('takes the row even when focus was never on one', async () => {
		const wrapper = await mountWithThree()
		selection.select('nte_2')
		await settle(2)
		;(wrapper.element as HTMLElement).focus()

		await actions.deleteNotes()
		await settle(5)

		expect(document.activeElement).toBe(rowElementOf(wrapper, 'nte_3'))
	})

	it('follows a done sweep that took the focused note with it', async () => {
		const wrapper = await mountWithThree()
		selection.select('nte_3')
		await settle(2)

		await actions.deleteDoneInActiveSection()
		await settle(5)

		expect(selection.focusedId.value).toBe(noteRow('nte_2'))
		expect(document.activeElement).toBe(rowElementOf(wrapper, 'nte_2'))
	})

	/** The complement, and the reason the move is conditional: a sweep pressed from
	 *  a button that removes notes elsewhere in the list must not pull focus off
	 *  that button. */
	it('leaves focus alone when the sweep did not touch the focused note', async () => {
		const wrapper = await mountWithThree()
		selection.select('nte_1')
		await settle(2)
		const root = wrapper.element as HTMLElement
		root.focus()

		await actions.deleteDoneInActiveSection()
		await settle(5)

		expect(document.activeElement).toBe(root)
	})
})

describe('the active-section chip', () => {
	function heading(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
		return wrapper.find('[data-slot="dropdown-menu-trigger"][title]')
	}

	it('shows the active section and how many notes are in it', async () => {
		const wrapper = await mountPanel()

		// `sec_a` holds both of the document's notes; `sec_b` holds none.
		expect(heading(wrapper).text()).toContain('Research')
		expect(heading(wrapper).text()).toContain('2')
		// The numeral is unambiguous to look at and meaningless read aloud, so the
		// spoken form carries the unit.
		expect(heading(wrapper).attributes('aria-label')).toBe(
			'Active section: Research, 2 notes. Switch section',
		)
	})

	it('counts what the section holds, not what a search left on screen', async () => {
		// The chip says where the next capture lands and what is already there.
		// Counting the filtered list would make the destination look emptier than it
		// is for as long as a query is active.
		const wrapper = await mountPanel()
		await wrapper.find('#panel-search').setValue('first')
		await settle(3)

		expect(selection.visibleNoteIds.value).toEqual(['nte_1'])
		expect(heading(wrapper).text()).toContain('2')
	})

	it('names the active section without touching the placeholder', async () => {
		const wrapper = await mountPanel()

		expect(heading(wrapper).text()).toContain('Research')
		// Task-004 acceptance criterion 3 stands: the placeholder names the *space*.
		expect(wrapper.find('#composer').attributes('placeholder')).toBe(
			'Add a note or a prompt (development)',
		)
	})

	it('sits under the search field, and nowhere else', async () => {
		// It began as a chip above the composer and moved: the active section is what
		// the list below it is *of*, and a label above a list is where a reader looks
		// for that. Moved, not copied — two controls saying the same thing is how one
		// of them ends up stale.
		const wrapper = await mountPanel()

		expect(wrapper.find('header').element.contains(heading(wrapper).element)).toBe(true)
		expect(wrapper.find('form[aria-label="Add a note"] [title]').exists()).toBe(false)
		expect(wrapper.findAll('[data-slot="dropdown-menu-trigger"][title]')).toHaveLength(1)
	})

	it('carries the full name in a title, so a truncated one is still readable', async () => {
		const wrapper = await mountPanel()
		const long = 'A section name long enough to need an ellipsis in a narrow panel'
		await installDocument({ ...SPACE, sections: [{ id: 'sec_a', name: long, order: 0 }] })
		await settle(3)

		expect(heading(wrapper).attributes('title')).toBe(long)
		expect(heading(wrapper).find('.truncate').exists()).toBe(true)
		// It updates when the active section changes — that is the whole reason it
		// exists, since the section's own header row scrolls out of view.
		expect(heading(wrapper).text()).toContain(long)
	})
})

/**
 * Task-014's fourth feature is a rename rather than a new surface: the recents
 * list was already the first group of the `...` menu, ordered by recency, with
 * the active entry marked and the whole group capped and scrolled. Only the word
 * changed — and it has now changed back. Task-014 renamed every visible label to
 * *project* while the format, the commands and the code kept saying "space";
 * living with the two words proved the split was the cost rather than the
 * feature, so the visible labels are "space" again and the whole product speaks
 * one language.
 */
describe('the spaces list in the menu', () => {
	const RECENTS = [
		{
			path: 'C:\\notes.copper',
			displayPath: 'C:\\notes.copper',
			key: 'c:\\notes.copper',
			name: 'development',
			active: true,
			availability: { state: 'available' as const },
		},
		{
			path: 'D:\\archive.copper',
			displayPath: 'D:\\archive.copper',
			key: 'd:\\archive.copper',
			name: 'archive',
			active: false,
			availability: {
				state: 'unavailable' as const,
				reason: 'drive-unavailable' as const,
				message: "The drive this space is on isn't connected.",
			},
		},
	]

	async function openMenu() {
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'list_recents') return RECENTS
			if (command === 'refresh_recents') return null
			return baseInvoke(command)
		})
		const wrapper = await mountPanel()
		await spaces.refresh()
		await wrapper.find('[aria-label="More actions"]').trigger('click')
		await settle(3)
		return document.querySelector<HTMLElement>('[data-slot="dropdown-menu-content"]')
	}

	it('lists every space by name, in recency order, without a submenu', async () => {
		const menu = await openMenu()

		expect(menu?.textContent).toContain('Spaces')
		expect(menu?.textContent).toContain('Open space…')
		expect(menu?.textContent).toContain('New space…')
		// Reachable directly rather than behind a nested trigger, which is what AC1
		// asks for.
		const rows = [...(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])].filter(
			(row) => row.textContent?.includes('.copper'),
		)
		expect(rows).toHaveLength(2)
		expect(rows[0]?.textContent).toContain('development')
		expect(rows[1]?.textContent).toContain('archive')
	})

	it('marks the open space and says so without relying on colour', async () => {
		const menu = await openMenu()

		const active = menu?.querySelector<HTMLElement>('[aria-current="true"]')
		expect(active?.textContent).toContain('development')
		expect(active?.textContent).toContain('active space')
	})

	/** A26 unchanged: an entry that is not on disk still says why, still shows its
	 *  last-known path, and stays clickable — clicking it is the retry after the
	 *  drive comes back, and Rust refuses it with the probe's own sentence. */
	it('shows an unavailable space with its cause and its path, still selectable', async () => {
		const menu = await openMenu()

		const rows = [...(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])]
		const dead = rows.find((row) => row.textContent?.includes('archive'))
		expect(dead?.textContent).toContain('D:\\archive.copper')
		expect(dead?.textContent).toContain("The drive this space is on isn't connected.")
		expect(dead?.getAttribute('data-disabled')).toBeNull()

		dead?.click()
		await settle(2)
		expect(mocks.invoke).toHaveBeenCalledWith('activate_space', { path: 'D:\\archive.copper' })
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

describe('scrolling to a section', () => {
	/**
	 * Choosing a section is choosing a place to be, so the list goes there rather
	 * than leaving the reader to find it — and it lands *at* the heading, with the
	 * section below it, which is what `start` buys over `nearest`.
	 *
	 * Every path funnels through `setActiveSection`: the switcher, the section
	 * menu's `Make active section`, and a click on the heading itself. `clientHeight`
	 * is installed by hand because happy-dom lays nothing out and the reveal
	 * declines a region with no height.
	 *
	 * `sec_a` rather than `sec_b`, and the reason is a rule worth naming: `sec_b`
	 * holds no notes here, so its heading is the *last* row — and the reveal hands
	 * the last row back to `pinToBottom`, which re-asserts the bottom every frame
	 * while the list settles instead of landing once and being left behind. The
	 * outcome there is the same heading on screen by a better mechanism; this
	 * asserts the path that scrolls.
	 */
	it('lands at the heading of the section that was switched to', async () => {
		const wrapper = await mountPanel()
		Object.defineProperty(wrapper.get('[data-scroll-region]').element, 'clientHeight', {
			configurable: true,
			get: () => 120,
		})

		const seen: (ScrollIntoViewOptions | undefined)[] = []
		const heading = wrapper.get(`[data-row-id="${sectionRow('sec_a')}"]`).element
		heading.scrollIntoView = (options?: boolean | ScrollIntoViewOptions) => {
			seen.push(options as ScrollIntoViewOptions)
		}

		await space.setActiveSection('sec_a')
		await settle(4)

		expect(seen).toEqual([{ block: 'start' }])
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

	it('shows each section with its own note count', async () => {
		// The count is what makes one destination worth picking over another, and it
		// is the document's rather than the filtered list's — a query narrows what is
		// on screen, not what a section holds.
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		const rows = [...(content()?.querySelectorAll('[role="menuitem"]') ?? [])]
		expect(rows[0]?.textContent).toContain('Research')
		expect(rows[0]?.textContent).toContain('2 notes')
		// An empty section still says so rather than showing nothing.
		expect(rows[1]?.textContent).toContain('Inbox')
		expect(rows[1]?.textContent).toContain('0 notes')
	})

	it('offers one field that both filters and creates', async () => {
		// Not two inputs. A dedicated "new section" field beside the filter would
		// fork the keyboard path — two places for Enter to mean something — and
		// duplicate a creation route that already exists.
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		expect(content()?.querySelectorAll('input')).toHaveLength(1)
		const filter = content()!.querySelector<HTMLInputElement>('#section-filter')!
		expect(filter.placeholder).toBe('Filter or create a section…')
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

	it('puts the chevron at the far end of the row, past the separator rule', async () => {
		// The heading starts at the row's own left edge and the chevron finishes at
		// the other one, with the rule spanning the distance — so the two things a
		// section row can be grabbed by sit at its extremes. Asserted on DOM order
		// rather than on classes, because that is the thing the requirement is about
		// and the thing a later restyle could quietly undo.
		const wrapper = await mountPanel()
		const cell = wrapper.find('[data-row-id="s:sec_a"] [role="gridcell"]').element
		const children = [...cell.children]

		const heading = children.findIndex((child) => child.tagName === 'H2')
		const chevron = children.findIndex((child) => child.matches('button[aria-expanded]'))

		expect(heading).toBe(0)
		expect(chevron).toBe(children.length - 1)
		expect(chevron).toBeGreaterThan(heading + 1)
	})

	it('keeps the chevron at the far end once the section is collapsed', async () => {
		const wrapper = await mountPanel()
		await disclosure(wrapper, 'Research').trigger('click')
		await settle(3)

		const cell = wrapper.find('[data-row-id="s:sec_a"] [role="gridcell"]').element
		const children = [...cell.children]
		const chevron = children.findIndex((child) => child.matches('button[aria-expanded]'))

		expect(chevron).toBe(children.length - 1)
		expect(disclosure(wrapper, 'Research').attributes('aria-expanded')).toBe('false')
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

	/** The full-size read's answer. Unlike the thumbnail, this one has to carry a
	 *  whole PNG signature: `attachment_full` returns raw bytes and the frontend
	 *  recovers the type from them to build the `Blob`. */
	const FULL_BYTES = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0]).buffer

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

		const cards = document.querySelectorAll<HTMLElement>('[data-note-row] button[aria-label*=".p"]')
		expect(cards).toHaveLength(2)
		// Task-014 split the two destinations, and the label follows: something with
		// a picture is *viewed* in the panel, and everything else still *opens*
		// through the OS.
		expect(cards[0]?.getAttribute('aria-label')).toContain(`View ${PNG.name}`)
		expect(cards[0]?.textContent).toContain(PNG.name)
		expect(cards[0]?.querySelector('img')).not.toBeNull()
		// A file with no preview is not a broken image: it renders a glyph and
		// stays enabled, because the blob is there.
		expect(cards[1]?.getAttribute('aria-label')).toContain(`Open ${PDF.name}`)
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
		// A `.pdf`, so this stays about the *gesture*: an attachment with no picture
		// still goes to the OS on a double-click, which is task-011's behaviour
		// unchanged. The image half is task-014's viewer, below.
		withAttachmentCommands()
		await mountPanel()
		await installWithAttachments(documentWith([PDF]))

		const card = document.querySelector<HTMLElement>('button[aria-label^="Open"]')
		card?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
		await settle(2)
		expect(mocks.invoke).not.toHaveBeenCalledWith('attachment_open', expect.anything())

		card?.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))
		await settle(2)
		expect(mocks.invoke).toHaveBeenCalledWith('attachment_open', { file: PDF.file })
	})

	// --- the in-panel image viewer (task-014) ---

	/** The thumbnail bytes plus a full-size read, which is what makes a card
	 *  *viewable* rather than merely present. */
	function withImagePreview(overrides: Record<string, unknown> = {}) {
		withAttachmentCommands({
			attachment_thumb: (args?: Record<string, unknown>) =>
				args?.file === PNG.file ? THUMB_BYTES : new ArrayBuffer(0),
			attachment_full: () => FULL_BYTES,
			...overrides,
		})
	}

	async function openViewer() {
		const card = document.querySelector<HTMLElement>('button[aria-label^="View"]')
		expect(card, 'no viewable attachment card rendered').not.toBeNull()
		card!.focus()
		card!.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))
		await settle(3)
		return card!
	}

	function viewer() {
		return document.querySelector<HTMLElement>('[role="dialog"][aria-modal="true"]')
	}

	it('opens the in-panel viewer for an image rather than the OS one', async () => {
		withImagePreview()
		await mountPanel()
		await installWithAttachments(documentWith([PNG]))

		await openViewer()

		expect(mocks.invoke).toHaveBeenCalledWith('attachment_full', { file: PNG.file })
		// The OS route is still there — it moved to `Space` — but a double-click must
		// not take both.
		expect(mocks.invoke).not.toHaveBeenCalledWith('attachment_open', expect.anything())
		expect(viewer()?.querySelector('img')).not.toBeNull()
	})

	it('keeps the OS viewer on Space, so task-011 does not lose its route', async () => {
		withImagePreview()
		await mountPanel()
		await installWithAttachments(documentWith([PNG]))

		const card = document.querySelector<HTMLElement>('button[aria-label^="View"]')
		card?.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }))
		await settle(2)

		expect(mocks.invoke).toHaveBeenCalledWith('attachment_open', { file: PNG.file })
		expect(viewer()).toBeNull()
	})

	/** The viewer is hand-rolled, so `inOverlay` does not see it and it needs a
	 *  rung of its own — one that fires above every other level of the ladder. */
	it('closes on Escape before any other rung, and hands focus back', async () => {
		withImagePreview()
		const wrapper = await mountPanel()
		await installWithAttachments(documentWith([PNG]))
		selection.select('nte_1')
		search.query.value = 'first'
		await settle(2)

		const card = await openViewer()
		expect(viewer()).not.toBeNull()

		await wrapper.trigger('keydown', { key: 'Escape' })
		await settle(2)

		expect(viewer()).toBeNull()
		// Every rung below it is untouched: one press closed one thing.
		expect(search.query.value).toBe('first')
		expect(selection.selectedIds.value).toEqual(['nte_1'])
		expect(mocks.invoke).not.toHaveBeenCalledWith('hide_panel')
		// Focus goes back where the press came from, not to the body — which is an
		// ancestor of the panel root and therefore outside the ladder entirely.
		expect(document.activeElement).toBe(card)
	})

	it('owns the keyboard while it is up, exactly as an open menu does', async () => {
		withImagePreview()
		const wrapper = await mountPanel()
		await installWithAttachments(documentWith([PNG]))
		selection.select('nte_1')

		await openViewer()
		await wrapper.trigger('keydown', { key: 'Delete' })
		await settle(2)

		expect(mocks.invoke).not.toHaveBeenCalledWith('delete_notes', expect.anything())
		expect(viewer()).not.toBeNull()
	})

	it('reports a refused read rather than showing an empty sheet', async () => {
		withImagePreview({
			attachment_full: () => {
				throw { kind: 'invalid', message: 'that image is 1.0 GB and the limit is 10.0 MB' }
			},
		})
		await mountPanel()
		await installWithAttachments(documentWith([PNG]))

		await openViewer()

		expect(viewer()?.textContent).toContain('the limit is 10.0 MB')
		expect(viewer()?.querySelector('img')).toBeNull()
	})

	/** A space switch revokes every object URL, this one included — so an open
	 *  viewer would be left holding a blob nothing can decode. */
	it('closes when the preview cache is revoked under it', async () => {
		withImagePreview()
		await mountPanel()
		await installWithAttachments(documentWith([PNG]))

		await openViewer()
		expect(viewer()).not.toBeNull()

		attachments.clearPreviews()
		await settle(2)

		expect(viewer()).toBeNull()
	})

	/**
	 * The same revocation, with the overlay **not mounted** — which is the case a
	 * component-scoped watcher cannot see.
	 *
	 * The tray's `open-settings` and the menu's Settings item both unmount
	 * `PanelShell`, so a space opened from Explorer while the settings view was
	 * up revoked the blob with nothing listening. Coming back then remounted the
	 * overlay over a URL that no longer resolves.
	 */
	it('does not come back over a blob revoked while the settings view was up', async () => {
		withImagePreview()
		await mountPanel()
		await installWithAttachments(documentWith([PNG]))
		await openViewer()

		// The settings view's door: this tree goes away entirely.
		panel?.unmount()
		panel = null
		attachments.clearPreviews()
		await settle(2)

		withImagePreview()
		await mountPanel()
		await settle(2)

		expect(viewer()).toBeNull()
	})

	/**
	 * Closing must never leave focus on `document.body`. It is an *ancestor* of the
	 * panel root, so a press there reaches neither the Escape ladder nor any chord
	 * — the panel becomes mouse-only with nothing saying why.
	 */
	it('hands focus back to something real when the invoking card has gone', async () => {
		withImagePreview()
		const wrapper = await mountPanel()
		await installWithAttachments(documentWith([PNG]))
		await openViewer()

		// What a space switch does to the element focus was going to return to:
		// the list is replaced and the button the user pressed is detached.
		await installWithAttachments({ ...SPACE, id: 'spc_2', notes: [] })
		await settle(2)

		await wrapper.trigger('keydown', { key: 'Escape' })
		await settle(2)

		expect(viewer()).toBeNull()
		expect(document.activeElement).not.toBe(document.body)
		expect(document.activeElement).not.toBeNull()
		// Inside the panel root, which is the property that actually matters.
		expect(
			(document.activeElement as HTMLElement | null)?.closest('[data-panel-root]'),
		).not.toBeNull()
	})

	/** Rust gating on the sniffed type says the bytes *begin* like an image, not
	 *  that they are a whole one. A decode that fails must say so rather than
	 *  leaving a broken glyph on a dark sheet. */
	it('reports an image the WebView cannot decode', async () => {
		withImagePreview()
		await mountPanel()
		await installWithAttachments(documentWith([PNG]))
		await openViewer()

		const image = viewer()?.querySelector('img')
		expect(image).not.toBeNull()
		image!.dispatchEvent(new Event('error'))
		await settle(2)

		expect(viewer()?.textContent).toContain('could not be displayed')
		expect(viewer()?.querySelector('img')).toBeNull()
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

describe('reordering', () => {
	/**
	 * A store that actually reorders, so a second press sees the result of the
	 * first. That is the whole subject here — every bug in this area was about what
	 * the *next* press does.
	 */
	function installReorderingStore() {
		let current: Space = { ...SPACE, notes: SPACE.notes.map((note) => ({ ...note })) }
		const calls: { id: string; section: string; index: number }[] = []

		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
			if (command === 'get_active_space') return current
			if (command !== 'reorder_note') return baseInvoke(command)

			const { id, section, index } = args as { id: string; section: string; index: number }
			calls.push({ id, section, index })

			const moved = current.notes.find((note) => note.id === id)
			if (!moved) return current
			const rest = current.notes.filter((note) => note.id !== id)
			const target = rest.filter((note) => note.section === section)
			target.splice(index, 0, { ...moved, section })
			const others = rest.filter((note) => note.section !== section)

			current = {
				...current,
				notes: [...others, ...target].map((note, at) => ({ ...note, order: at })),
			}
			return current
		})

		return {
			calls,
			/** Note ids in the order the document holds them, for one section. */
			order: (section: string) =>
				current.notes.filter((note) => note.section === section).map((note) => note.id),
		}
	}

	describe('Alt+Arrow', () => {
		/**
		 * **The bug this exists for, root-caused live in WebView2.**
		 *
		 * Alt+Arrow reordering worked exactly once and then went dead. The grid is a
		 * descendant of the shell, so it sees a press first, and its `ArrowDown` case
		 * tested no modifier: it `preventDefault`ed Alt+ArrowDown and moved the
		 * roving target, and the shell's chord layer — whose first line declines a
		 * press that is already prevented — never ran at all. The one position it did
		 * work from was a control *inside* a row, because the grid's guard
		 * early-returns for a button target. The reorder that followed then put focus
		 * back on the row itself, where this handler could swallow every press after
		 * it.
		 *
		 * So the measurement is not "does one press reorder" — one always did, from
		 * the right starting position. It is whether the press after it does.
		 */
		it('reorders repeatedly from a focused row, not once from a lucky one', async () => {
			const wrapper = await mountPanel()
			const store = installReorderingStore()

			const row = wrapper.find('[data-row-id="n:nte_1"]').element as HTMLElement
			row.click()
			row.focus()
			await settle()

			await wrapper
				.find('[data-row-id="n:nte_1"]')
				.trigger('keydown', { key: 'ArrowDown', altKey: true })
			await settle(4)

			expect(store.calls).toHaveLength(1)
			expect(store.order('sec_a')).toEqual(['nte_2', 'nte_1'])
			// Focus lands back on the moved row — both halves of the roving target, so
			// the press after this one is seen at all.
			expect(selection.focusedId.value).toBe('n:nte_1')
			expect((document.activeElement as HTMLElement | null)?.dataset.rowId).toBe('n:nte_1')

			// The press that used to do nothing: it arrives at the row the previous
			// reorder just focused, which is exactly where the grid used to eat it.
			await wrapper
				.find('[data-row-id="n:nte_1"]')
				.trigger('keydown', { key: 'ArrowDown', altKey: true })
			await settle(4)

			expect(store.calls).toHaveLength(2)
			// Past the end of its own section it carries on into the next one, which
			// is what a drag does too.
			expect(store.order('sec_a')).toEqual(['nte_2'])
			expect(store.order('sec_b')).toEqual(['nte_1'])
		})

		it('leaves an unmodified arrow to the grid, which still moves the target', async () => {
			// The guard is "Alt belongs to the shell", not "arrows are off limits".
			const wrapper = await mountPanel()
			const store = installReorderingStore()

			const row = wrapper.find('[data-row-id="n:nte_1"]')
			;(row.element as HTMLElement).click()
			await row.trigger('keydown', { key: 'ArrowDown' })
			await settle()

			expect(store.calls).toEqual([])
			expect(selection.focusedId.value).toBe('n:nte_2')
		})

		it('lands at the bottom of a collapsed section it travels up into', async () => {
			// The comment above the code promises "entering from below lands at the
			// bottom", and the index was counted off the *visible* walk — which
			// publishes an empty list for a collapsed section. So an Alt+Up into a
			// folded section put the note at the top, which is the opposite of the
			// direction it was travelling.
			const wrapper = await mountPanel()
			const store = installReorderingStore()

			// Get nte_1 into the second section first, so there is a section above it.
			const row = wrapper.find('[data-row-id="n:nte_1"]').element as HTMLElement
			row.click()
			row.focus()
			await settle()
			for (const _ of [0, 1]) {
				await wrapper
					.find('[data-row-id="n:nte_1"]')
					.trigger('keydown', { key: 'ArrowDown', altKey: true })
				await settle(4)
			}
			expect(store.order('sec_b')).toEqual(['nte_1'])
			expect(store.order('sec_a')).toEqual(['nte_2'])

			// Folded through the state rather than by clicking its chevron: a click
			// there deliberately takes the roving target onto the header row, which
			// would leave this test asserting that a focused *header* reorders nothing.
			sections.setCollapsed('sec_a', true)
			await settle(3)

			await wrapper
				.find('[data-row-id="n:nte_1"]')
				.trigger('keydown', { key: 'ArrowUp', altKey: true })
			await settle(4)

			// Index 1, after the note already there — not 0, which is what the empty
			// published list produced.
			expect(store.calls.at(-1)).toEqual({ id: 'nte_1', section: 'sec_a', index: 1 })
			expect(store.order('sec_a')).toEqual(['nte_2', 'nte_1'])
		})

		it('does not collapse a multi-note selection as a side effect', async () => {
			// Unlike a drag, this is a keyboard action on the *focused* note. Nudging
			// one note is not a reason to throw away the others the user picked.
			const wrapper = await mountPanel()
			installReorderingStore()
			selection.select('nte_1')
			selection.extendTo('nte_2')

			await wrapper
				.find('[data-row-id="n:nte_2"]')
				.trigger('keydown', { key: 'ArrowDown', altKey: true })
			await settle(4)

			expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])
		})
	})

	describe('pointer drag', () => {
		/**
		 * happy-dom lays nothing out, so every rect is zero and a drop would resolve
		 * to the top of the first section whatever the pointer did. The geometry is
		 * real product behaviour and only the environment is missing — the same
		 * argument the Web Animations stub at the top of this file makes — so the
		 * boxes are supplied here rather than the composable weakened to cope
		 * without them.
		 *
		 * `sec_a` spans 0–100 with its two notes at 20–60 and 64–100; `sec_b` spans
		 * 124–200 and holds nothing.
		 */
		const BOXES: Record<string, [number, number]> = {
			'[data-note-list]': [0, 400],
			'[data-scroll-region]': [0, 400],
			'[data-section-id="sec_a"]': [0, 100],
			'[data-section-id="sec_b"]': [124, 200],
			'[data-row-id="n:nte_1"]': [20, 60],
			'[data-row-id="n:nte_2"]': [64, 100],
			'[data-section-row]': [0, 16],
		}

		/**
		 * How far the region has been scrolled, subtracted from the list root's own
		 * top exactly as a real scroll would move it. A test sets this and fires a
		 * `scroll` event to move the content under a pointer that is holding still.
		 */
		let scrolledBy = 0

		function stubLayout() {
			const proto = Element.prototype as unknown as Record<string, unknown>
			const real = proto.getBoundingClientRect
			proto.getBoundingClientRect = function (this: Element) {
				const entry = Object.entries(BOXES).find(([selector]) => this.matches(selector))
				const [top, bottom] = entry?.[1] ?? [0, 0]
				// The scroll region itself is the viewport and does not move; everything
				// inside it does.
				const shift = this.matches('[data-scroll-region]') ? 0 : scrolledBy
				return {
					top: top - shift,
					bottom: bottom - shift,
					left: 0,
					right: 390,
					width: 390,
					height: bottom - top,
				}
			}
			return () => {
				proto.getBoundingClientRect = real
				scrolledBy = 0
			}
		}

		/** Pointer capture is part of the gesture and part of no DOM happy-dom
		 *  implements. Only the three calls the drag makes are needed. */
		function stubPointerCapture() {
			const proto = Element.prototype as unknown as Record<string, unknown>
			const held = new Set<number>()
			proto.setPointerCapture = (id: number) => held.add(id)
			proto.hasPointerCapture = (id: number) => held.has(id)
			proto.releasePointerCapture = (id: number) => held.delete(id)
			return () => {
				for (const name of ['setPointerCapture', 'hasPointerCapture', 'releasePointerCapture']) {
					Reflect.deleteProperty(proto, name)
				}
			}
		}

		function pointer(name: string, clientY: number, overrides: Record<string, unknown> = {}) {
			const event = new Event(name, { bubbles: true, cancelable: true })
			return Object.assign(event, {
				pointerId: 1,
				pointerType: 'mouse',
				button: 0,
				// A real primary pointer carries this, and the drag requires it — a pen
				// barrel button and a second finger are both non-primary or non-zero.
				isPrimary: true,
				clientX: 300,
				clientY,
				...overrides,
			})
		}

		let restore: (() => void)[] = []

		beforeEach(() => {
			restore = [stubLayout(), stubPointerCapture()]
		})

		afterEach(() => {
			for (const undo of restore.splice(0)) undo()
		})

		function gripOf(wrapper: Awaited<ReturnType<typeof mountPanel>>, rowId: string) {
			const grip = wrapper.find(`[data-row-id="${rowId}"] [data-drag-handle]`)
			expect(grip.exists(), `no drag handle on ${rowId}`).toBe(true)
			return grip.element as HTMLElement
		}

		function draggingRow() {
			return document.querySelector('[data-note-row][data-dragging]')
		}

		it('drags a note past its neighbour and commits the index that lands it there', async () => {
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			await settle()

			// The list is not reordered while the drag runs — the row is translated
			// and a line painted where it would land — so the DOM still holds the old
			// order at this point. That is what lets the commit hand over a section and
			// an index instead of reading a mutated DOM back.
			expect(draggingRow()).not.toBeNull()
			expect(
				wrapper.findAll('[data-row-id^="n:"]').map((row) => row.attributes('data-row-id')),
			).toEqual(['n:nte_1', 'n:nte_2'])

			window.dispatchEvent(pointer('pointerup', 90))
			await settle(4)

			expect(store.calls).toEqual([{ id: 'nte_1', section: 'sec_a', index: 1 }])
			expect(store.order('sec_a')).toEqual(['nte_2', 'nte_1'])
			// Nothing of the gesture is left on the row.
			expect(draggingRow()).toBeNull()
		})

		it('carries a note into another section in one gesture', async () => {
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			// Into the empty second section, which has no row to compare against —
			// only the band it occupies.
			window.dispatchEvent(pointer('pointermove', 150))
			window.dispatchEvent(pointer('pointerup', 150))
			await settle(4)

			expect(store.calls).toEqual([{ id: 'nte_1', section: 'sec_b', index: 0 }])
			expect(store.order('sec_b')).toEqual(['nte_1'])
		})

		it('holds a press that has barely moved, so the grip stays clickable', async () => {
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 43))
			await settle()
			expect(draggingRow()).toBeNull()

			window.dispatchEvent(pointer('pointerup', 43))
			await settle(3)

			expect(store.calls).toEqual([])
		})

		it('commits nothing for a drag that ends where it started', async () => {
			// A drag that changed nothing must not push an undo entry.
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 50))
			window.dispatchEvent(pointer('pointerup', 50))
			await settle(4)

			expect(store.calls).toEqual([])
		})

		it('abandons the drag on Escape without touching the document', async () => {
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			selection.select('nte_1')
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			await settle()
			expect(draggingRow()).not.toBeNull()

			window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
			await settle(3)

			expect(store.calls).toEqual([])
			expect(draggingRow()).toBeNull()
			// The press is consumed by the drag rather than falling through to the
			// shell's ladder, which would have cleared the selection underneath it.
			expect(selection.selectedIds.value).toEqual(['nte_1'])

			// And the gesture is genuinely over: a later pointerup commits nothing.
			window.dispatchEvent(pointer('pointerup', 90))
			await settle(3)
			expect(store.calls).toEqual([])
		})

		it('abandons the drag when the pointer stream is cancelled', async () => {
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			window.dispatchEvent(pointer('pointercancel', 90))
			await settle(3)

			expect(store.calls).toEqual([])
			expect(draggingRow()).toBeNull()
		})

		it('withdraws the handle while a search is active', async () => {
			// A filtered list is a subset of its section, so an index read off it is
			// not the index `reorder_note` takes.
			const wrapper = await mountPanel()
			await wrapper.find('#panel-search').setValue('first')
			await settle(3)

			expect(wrapper.find('[data-drag-handle]').exists()).toBe(false)
		})

		it('refuses a non-primary button, so a pen barrel does not reorder', async () => {
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			const grip = gripOf(wrapper, 'n:nte_1')

			// A pen's barrel button and its eraser both arrive as a `pointerdown` that
			// is not button 0. Testing `pointerType === 'mouse'` first let them through.
			grip.dispatchEvent(pointer('pointerdown', 40, { pointerType: 'pen', button: 2 }))
			window.dispatchEvent(pointer('pointermove', 90))
			await settle()
			expect(draggingRow()).toBeNull()

			window.dispatchEvent(pointer('pointerup', 90))
			await settle(3)
			expect(store.calls).toEqual([])
		})

		it('ends the gesture when the window loses focus', async () => {
			// The one way a `pointerup` never arrives at all: an alt-tab, or a click
			// that raises another window, delivers the release somewhere else. Left
			// running, the row keeps its transform and its raised stacking order,
			// `isDragging` stays true so auto-animate never comes back, and the
			// auto-scroll loop keeps asking for frames.
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			await settle()
			expect(draggingRow()).not.toBeNull()

			window.dispatchEvent(new Event('blur'))
			await settle(2)

			expect(draggingRow()).toBeNull()
			expect(drag.isDragging.value).toBe(false)
			expect(
				wrapper
					.findAll('[data-note-row]')
					.filter((row) => (row.element as HTMLElement).style.transform),
			).toHaveLength(0)

			// The release that finally arrives belongs to nothing and commits nothing.
			window.dispatchEvent(pointer('pointerup', 90))
			await settle(3)
			expect(store.calls).toEqual([])
		})

		it('recomputes the drop target when the list scrolls under a still pointer', async () => {
			// The drop is a function of where the pointer is *in the content*, so it
			// has to be recomputed when the content moves and not only when the pointer
			// does. A wheel or a trackpad during a drag otherwise leaves the indicator
			// pointing at the row that used to be there.
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			await settle()
			expect(drag.dropTarget.value?.sectionId).toBe('sec_a')

			// The content scrolls up by 60px while the pointer holds still, which puts
			// the same screen position into the second section's band.
			scrolledBy = 60
			wrapper.find('[data-scroll-region]').element.dispatchEvent(new Event('scroll'))
			await settle()

			expect(drag.dropTarget.value?.sectionId).toBe('sec_b')

			window.dispatchEvent(pointer('pointerup', 90))
			await settle(4)
			expect(store.calls).toEqual([{ id: 'nte_1', section: 'sec_b', index: 0 }])
		})

		it('abandons the drag when a document arrives mid-gesture', async () => {
			// Every row position the drop resolves against was measured once, against a
			// list that has now changed underneath the pointer. Committing on those
			// numbers would move the note somewhere nobody pointed at.
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			await settle()
			expect(draggingRow()).not.toBeNull()

			// A capture landing from the global hotkey, or an edit to the file on disk.
			await installDocument({
				...SPACE,
				notes: [{ ...SPACE.notes[0]!, body: 'changed underneath the drag' }, SPACE.notes[1]!],
			})
			await settle(3)

			expect(draggingRow()).toBeNull()
			expect(drag.isDragging.value).toBe(false)

			window.dispatchEvent(pointer('pointerup', 90))
			await settle(3)
			expect(store.calls).toEqual([])
		})

		it('settles the list animation before it measures a single row', async () => {
			// auto-animate is mid-FLIP for 150ms after any list change, and an animated
			// row reports its *transformed* box — so a drag begun just after a capture
			// landed would measure rows at positions they are still travelling away
			// from. The stand-down watcher cannot be relied on for this: it runs
			// asynchronously off the same flag the drag sets.
			const wrapper = await mountPanel()
			installReorderingStore()

			const finish = vi.fn()
			const proto = Element.prototype as unknown as Record<string, unknown>
			proto.getAnimations = () => [{ playState: 'running', finish }]
			restore.push(() => Reflect.deleteProperty(proto, 'getAnimations'))

			const grip = gripOf(wrapper, 'n:nte_1')
			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			await settle()

			expect(finish).toHaveBeenCalled()

			window.dispatchEvent(pointer('pointerup', 90))
			await settle(3)
		})

		it('lands the grid a tab stop when a note is dropped into a collapsed section', async () => {
			// `select` points the roving target at the dropped note unconditionally,
			// but a collapsed destination renders no row for it — which would leave the
			// grid with `tabindex="0"` on nothing and unreachable by Tab. The
			// destination's header is the honest fallback, exactly as a `Move to ▸`
			// into a folded section already does.
			const wrapper = await mountPanel()
			const store = installReorderingStore()

			const inbox = wrapper.find(
				'button[aria-label="Collapse Inbox"], button[aria-label="Expand Inbox"]',
			)
			await inbox.trigger('click')
			await settle(3)

			const grip = gripOf(wrapper, 'n:nte_1')
			grip.dispatchEvent(pointer('pointerdown', 40))
			// Into the collapsed second section's band.
			window.dispatchEvent(pointer('pointermove', 150))
			window.dispatchEvent(pointer('pointerup', 150))
			await settle(4)

			expect(store.calls).toEqual([{ id: 'nte_1', section: 'sec_b', index: 0 }])
			// The note is still selected — collapse folds a row away, it never
			// unselects — but the roving target went somewhere that exists.
			expect(selection.selectedIds.value).toEqual(['nte_1'])
			expect(wrapper.find('[data-row-id="n:nte_1"]').exists()).toBe(false)
			expect(wrapper.findAll('[data-row-id][tabindex="0"]').length).toBeGreaterThan(0)
			expect(selection.focusedId.value).toBe('s:sec_b')
		})

		it('does not select the row when a cancelled drag releases', async () => {
			// Escape ends the drag, but the pointer is still down: the release that
			// follows is a `click` on the grip by every definition the browser has, and
			// the grip sits inside a row whose own click selects. Cancelling a reorder
			// and then selecting the note anyway is not what Escape means.
			const wrapper = await mountPanel()
			installReorderingStore()
			selection.select('nte_2')
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			await settle()

			window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
			await settle(2)

			// The click the browser synthesises from the down/up pair, which the
			// existing Escape test never fired.
			window.dispatchEvent(pointer('pointerup', 90))
			grip.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
			await settle(2)

			expect(selection.selectedIds.value).toEqual(['nte_2'])
		})
	})
})

/**
 * Task-013 feature 1. `Ctrl+V` with nothing focused captures the clipboard, and
 * the listener sits on `document` — `document.body` is an *ancestor* of the panel
 * root, so a press delivered there would never bubble down to it.
 */
describe('zero-focus paste', () => {
	/** A `paste` whose `clipboardData` is stubbed rather than built from a real
	 *  `DataTransfer`: only `getData('text/plain')` is read, and the two branches
	 *  are "there was text" and "there was not". */
	function paste(text: string) {
		const event = new Event('paste', { bubbles: true, cancelable: true })
		Object.defineProperty(event, 'clipboardData', {
			value: { getData: () => text },
		})
		return event
	}

	function calls(command: string) {
		return mocks.invoke.mock.calls.filter((call) => call[0] === command)
	}

	it('captures clipboard text as a note in the active section', async () => {
		await mountPanel()

		document.body.dispatchEvent(paste('pasted text'))
		await settle(3)

		// `add_note`, not `submit_entry`: a paste is a capture, so a body that is
		// entirely `# Heading` has to become a note rather than a section directive.
		expect(calls('add_note')).toHaveLength(1)
		expect(calls('add_note')[0]?.[1]).toEqual({ body: 'pasted text', section: null })
		expect(calls('submit_entry')).toHaveLength(0)
	})

	it('leaves a `# Heading` a note rather than a section directive', async () => {
		await mountPanel()

		document.body.dispatchEvent(paste('# Research'))
		await settle(3)

		expect(calls('add_note')[0]?.[1]).toEqual({ body: '# Research', section: null })
	})

	it('does not move focus into the composer or scroll to it', async () => {
		const wrapper = await mountPanel()
		takeRow(noteRow('nte_2'))
		await settle(2)

		document.body.dispatchEvent(paste('pasted text'))
		await settle(3)

		// The note lands silently: the roving target stays where the user left it,
		// unlike a composer submit, which follows what it just created.
		expect(selection.focusedId.value).toBe(noteRow('nte_2'))
		expect(document.activeElement).not.toBe(wrapper.find('[data-composer]').element)
	})

	it('routes a clipboard with no text to the attachment ingest', async () => {
		await mountPanel()

		document.body.dispatchEvent(paste(''))
		await settle(3)

		expect(calls('attach_paste')).toHaveLength(1)
		expect(calls('add_note')).toHaveLength(0)
	})

	it('is a silent no-op on an empty clipboard', async () => {
		const wrapper = await mountPanel()

		// `attach_paste` answers with an empty list, which is its "text, or nothing"
		// signal — and with no text either there is nothing to do.
		document.body.dispatchEvent(paste(''))
		await settle(3)

		expect(calls('add_note')).toHaveLength(0)
		expect(wrapper.find('[role="alert"]').text()).toBe('')
	})

	it('declines while a text surface has focus, so the native paste runs', async () => {
		const wrapper = await mountPanel()

		// The composer, the inline editor, the search field and both rename fields
		// all resolve through the same predicate; the composer is the one that
		// already has a `paste` handler of its own.
		wrapper.find('[data-composer]').element.dispatchEvent(paste('typed into the field'))
		wrapper.find('[data-search]').element.dispatchEvent(paste('a query'))
		await settle(3)

		expect(calls('add_note')).toHaveLength(0)
	})

	it('whitespace alone is not text', async () => {
		await mountPanel()

		document.body.dispatchEvent(paste('   \n\t '))
		await settle(3)

		// The store would refuse an empty body anyway; asking `attach_paste` instead
		// is what makes a stray whitespace clipboard silent rather than an error.
		expect(calls('add_note')).toHaveLength(0)
		expect(calls('attach_paste')).toHaveLength(1)
	})
})

/**
 * Task-013 feature 3. Double-clicking a note body runs the action the setting
 * names — and must not fire from the row's controls, the grip, or a drag.
 */
describe('the double-click action', () => {
	function bodyOf(wrapper: Awaited<ReturnType<typeof mountPanel>>, rowId: string) {
		return wrapper.get(`[data-row-id="${rowId}"] .note-prose`)
	}

	async function useEdit() {
		settingsPayload = { ...defaultSettings(), doubleClick: 'edit' }
		await settings.refresh()
	}

	it('copies the note by default', async () => {
		const wrapper = await mountPanel()
		const body = bodyOf(wrapper, 'n:nte_1')

		await body.trigger('click')
		await body.trigger('dblclick')
		await settle(3)

		const written = mocks.invoke.mock.calls.filter((call) => call[0] === 'clipboard_write_text')
		expect(written).toHaveLength(1)
		expect(written[0]?.[1]).toEqual({ text: 'first note' })
		expect(editor.editingNoteId.value).toBeNull()
	})

	it('opens the inline editor when the setting says edit', async () => {
		const wrapper = await mountPanel()
		await useEdit()
		const body = bodyOf(wrapper, 'n:nte_1')

		await body.trigger('click')
		await body.trigger('dblclick')
		await settle(3)

		expect(editor.editingNoteId.value).toBe('nte_1')
		expect(
			mocks.invoke.mock.calls.filter((call) => call[0] === 'clipboard_write_text'),
		).toHaveLength(0)
	})

	it('collapses the word the gesture selected rather than declining because of it', async () => {
		// `.note-prose` is `select-text`, so a body double-click always leaves a
		// non-empty selection by the time `dblclick` fires — reading
		// `getSelection()` as a guard would suppress the feature exactly where it is
		// meant to work.
		const wrapper = await mountPanel()
		const body = bodyOf(wrapper, 'n:nte_1')
		const range = document.createRange()
		range.selectNodeContents(body.element)
		window.getSelection()?.addRange(range)

		await body.trigger('click')
		await body.trigger('dblclick')
		await settle(3)

		expect(
			mocks.invoke.mock.calls.filter((call) => call[0] === 'clipboard_write_text'),
		).toHaveLength(1)
		expect(window.getSelection()?.toString()).toBe('')
	})

	it('does not fire from the completion box or the grip', async () => {
		const wrapper = await mountPanel()
		const row = wrapper.get('[data-row-id="n:nte_1"]')

		await row.get('button').trigger('dblclick')
		await row.get('[data-drag-handle]').trigger('dblclick')
		await settle(3)

		expect(
			mocks.invoke.mock.calls.filter((call) => call[0] === 'clipboard_write_text'),
		).toHaveLength(0)
	})
})

/**
 * Task-013 feature 4. Three scopes, one renderer — so what these assert is the
 * *scope resolution*: which sections and which notes each affordance hands over.
 * The formatting itself is `noteMarkdown.test.ts`.
 */
describe('copy as Markdown', () => {
	// The second note opens a fence, which cannot follow a list marker on the same
	// line without ceasing to be one — so it sits under a bare marker. See
	// `noteMarkdown.test.ts`, which parses that form back to prove it survives.
	const RESEARCH = [
		'# Research',
		'- [ ] first note',
		'- [x]',
		'',
		'  ```js',
		'  const a = 1',
		'  ```',
	].join('\n')

	function copied() {
		const written = mocks.invoke.mock.calls.filter((call) => call[0] === 'clipboard_write_text')
		return (written.at(-1)?.[1] as { text: string } | undefined)?.text ?? null
	}

	it('copies the whole document, empty sections included', async () => {
		await mountPanel()

		await actions.copyDocumentAsMarkdown()
		await settle(2)

		// `Inbox` holds nothing and still gets its heading: the scope is the
		// document, and a section that is in scope is in the output.
		expect(copied()).toBe(`${RESEARCH}\n\n# Inbox`)
	})

	/** AC5. A "copy all" that quietly copied a filtered subset would be the one
	 *  export nobody could trust. */
	it('ignores an active search', async () => {
		const wrapper = await mountPanel()
		await wrapper.find('#panel-search').setValue('const')
		await settle(3)
		expect(search.resultCount.value).toBe(1)

		await actions.copyDocumentAsMarkdown()
		await settle(2)

		expect(copied()).toBe(`${RESEARCH}\n\n# Inbox`)
	})

	it('copies one section, whole, from the section menu', async () => {
		await mountPanel()

		await actions.copySectionAsMarkdown('sec_a')
		await settle(2)

		expect(copied()).toBe(RESEARCH)
	})

	it('writes nothing for a section holding no notes', async () => {
		await mountPanel()

		await actions.copySectionAsMarkdown('sec_b')
		await settle(2)

		// A heading on its own is not worth replacing the clipboard with — the
		// same rule every other empty copy follows. Its heading still appears in
		// the document-wide copy above, where there are notes to carry it.
		expect(copied()).toBeNull()
	})

	/** AC8/AC9. A selection copy groups by the sections its notes came from, and
	 *  a section contributing nothing is dropped rather than emitted empty. */
	it('groups a selection under the sections its notes came from', async () => {
		await mountPanel()
		selection.selectAll()

		await actions.copySelectionAsMarkdown()
		await settle(2)

		expect(copied()).toBe(RESEARCH)
	})

	it('copies a single selected note under its own heading', async () => {
		await mountPanel()
		selection.select('nte_1')

		await actions.copySelectionAsMarkdown()
		await settle(2)

		expect(copied()).toBe('# Research\n- [ ] first note')
	})

	/** AC16. Folding a section away never narrows what an action targets, so a
	 *  select-all-then-copy over a collapsed section still carries its notes. */
	it('reaches a collapsed section through select-all', async () => {
		const wrapper = await mountPanel()
		await wrapper
			.find('button[aria-label="Collapse Research"], button[aria-label="Expand Research"]')
			.trigger('click')
		await settle(3)
		expect(wrapper.find('[data-row-id="n:nte_1"]').exists()).toBe(false)

		selection.selectAll()
		await actions.copySelectionAsMarkdown()
		await settle(2)

		expect(copied()).toBe(RESEARCH)
	})

	it('writes nothing at all when there is nothing selected', async () => {
		await mountPanel()
		selection.clear()
		selection.focusRow(null)

		await actions.copySelectionAsMarkdown()
		await settle(2)

		expect(copied()).toBeNull()
	})
})

/**
 * Merging inside a collapsed section, which is reachable in two gestures now that
 * the section menu can select one: `Select all` on a folded section, then the
 * merge chord.
 */
describe('merge and the roving target', () => {
	it('holds the section header when the survivor has no row of its own', async () => {
		const wrapper = await mountPanel()
		await wrapper
			.find('button[aria-label="Collapse Research"], button[aria-label="Expand Research"]')
			.trigger('click')
		await settle(3)

		// What the section context menu's `Select all` does. Both notes are selected
		// even though neither has a row — collapse folds rows away, it never narrows
		// what an action targets.
		selection.selectSection('sec_a')
		await settle(2)
		expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])

		wrapper.element.dispatchEvent(
			new KeyboardEvent('keydown', { key: 'M', ctrlKey: true, shiftKey: true, bubbles: true }),
		)
		await settle(4)

		expect(selection.selectedIds.value).toEqual(['nte_1'])
		// The survivor is still inside the folded section, so it has no row at all —
		// pointing the roving target at one would leave the grid with `tabindex="0"`
		// on nothing and unreachable by Tab.
		expect(wrapper.find('[data-row-id="n:nte_1"]').exists()).toBe(false)
		expect(selection.focusedId.value).toBe('s:sec_a')
		expect(wrapper.findAll('[data-row-id][tabindex="0"]').length).toBe(1)
	})

	it('holds the survivor itself when its section is open', async () => {
		const wrapper = await mountPanel()
		selection.selectSection('sec_a')
		await settle(2)

		wrapper.element.dispatchEvent(
			new KeyboardEvent('keydown', { key: 'M', ctrlKey: true, shiftKey: true, bubbles: true }),
		)
		await settle(4)

		expect(selection.focusedId.value).toBe(noteRow('nte_1'))
		expect(wrapper.findAll('[data-row-id][tabindex="0"]').length).toBe(1)
	})
})

/**
 * AC12 at the panel, not at the renderer: `noteMarkdown.test.ts` proves the same
 * input formats identically, and this proves the three scopes actually resolve to
 * the same input when they should.
 */
describe('the three copy scopes agree byte for byte', () => {
	/** Every section holding notes, so the whole-document and select-all scopes
	 *  cover exactly the same ground — with `sec_b` empty they legitimately differ
	 *  by its heading, which is the documented empty-section rule and a different
	 *  question. */
	async function installFilledDocument() {
		const filled: Space = {
			...SPACE,
			notes: [
				...SPACE.notes,
				{
					id: 'nte_3',
					section: 'sec_b',
					order: 0,
					done: true,
					body: 'a note in the second section',
					created: '2026-08-05T00:00:00Z',
					updated: '2026-08-05T00:00:00Z',
				},
			],
		}
		mocks.invoke.mockImplementation(async (command: string) =>
			command === 'get_active_space' ? filled : baseInvoke(command),
		)
		await space.refresh()
	}

	function copied() {
		const written = mocks.invoke.mock.calls.filter((call) => call[0] === 'clipboard_write_text')
		return (written.at(-1)?.[1] as { text: string } | undefined)?.text ?? null
	}

	it('produces identical text from the menu action and from select-all', async () => {
		await mountPanel()
		await installFilledDocument()

		await actions.copyDocumentAsMarkdown()
		await settle(2)
		const fromMenu = copied()

		selection.selectAll()
		await actions.copySelectionAsMarkdown()
		await settle(2)
		const fromSelection = copied()

		expect(fromSelection).toBe(fromMenu)
		// Not vacuously equal: both carry both sections, both done states, and the
		// fence-bearing note's block form.
		expect(fromMenu).toContain('# Research')
		expect(fromMenu).toContain('# Inbox')
		expect(fromMenu).toContain('- [ ] first note')
		expect(fromMenu).toContain('- [x] a note in the second section')
		expect(fromMenu).toContain('  ```js')
	})

	it('produces the same text again as the two sections copied one at a time', async () => {
		await mountPanel()
		await installFilledDocument()

		await actions.copyDocumentAsMarkdown()
		await settle(2)
		const whole = copied()

		await actions.copySectionAsMarkdown('sec_a')
		await settle(2)
		const research = copied()

		await actions.copySectionAsMarkdown('sec_b')
		await settle(2)
		const inbox = copied()

		// The sections are joined by exactly one blank line, so the single-section
		// scope is the document scope restricted — not a second formatting.
		expect(`${research}\n\n${inbox}`).toBe(whole)
	})
})

/**
 * Task-013 feature 2 at the panel. Placement itself is Rust's — these assert the
 * frontend does nothing of its own about it, which is design decision 11.
 */
describe('top insertion', () => {
	/** `SPACE` with a freshly pasted note leading `sec_a`, which is what the store
	 *  returns once `insertionPoint` is `top`. */
	const TOP_FIRST: Space = {
		...SPACE,
		notes: [
			{
				id: 'nte_3',
				section: 'sec_a',
				order: 0,
				done: false,
				body: 'pasted at the top',
				created: '2026-08-06T00:00:00Z',
				updated: '2026-08-06T00:00:00Z',
			},
			{ ...SPACE.notes[0]!, order: 1 },
			{ ...SPACE.notes[1]!, order: 2 },
		],
	}

	async function mountWithTopInsertion() {
		settingsPayload = { ...defaultSettings(), insertionPoint: 'top' }
		mocks.invoke.mockImplementation(async (command: string) =>
			command === 'add_note' ? { space: TOP_FIRST, noteId: 'nte_3' } : baseInvoke(command),
		)
		const wrapper = await mountPanel()
		await settings.refresh()
		return wrapper
	}

	function paste(text: string) {
		const event = new Event('paste', { bubbles: true, cancelable: true })
		Object.defineProperty(event, 'clipboardData', { value: { getData: () => text } })
		return event
	}

	it('renders a pasted note as the first row of its section', async () => {
		const wrapper = await mountWithTopInsertion()
		takeRow(noteRow('nte_2'))
		await settle(2)

		document.body.dispatchEvent(paste('pasted at the top'))
		await settle(4)

		const rows = wrapper.findAll('[data-row-id^="n:"]').map((row) => row.attributes('data-row-id'))
		expect(rows).toEqual([noteRow('nte_3'), noteRow('nte_1'), noteRow('nte_2')])
	})

	/**
	 * Decision 11 left the frontend with no `{ kind: 'top' }` anchor and no
	 * `pinToTop`, so nothing carried the reader to a note inserted above them — the
	 * scroll restore held them exactly where they were, looking at a list whose new
	 * first row was off screen.
	 *
	 * That is now what the reveal is for, and a top insertion is the case that
	 * needs it most: the bottom pin cannot help, since the note is at the other end.
	 * `clientHeight` is installed by hand because happy-dom lays nothing out and the
	 * reveal refuses to scroll a region with no height — a real hidden panel is the
	 * reason that refusal exists.
	 */
	it('scrolls a note inserted above the reader into view once there is a list to scroll', async () => {
		const wrapper = await mountWithTopInsertion()
		takeRow(noteRow('nte_2'))
		await settle(2)

		// Pasted into a list with no layout, which is what a hidden panel is: the
		// reveal cannot land yet and keeps the request rather than spending it.
		document.body.dispatchEvent(paste('pasted at the top'))
		await settle(4)

		const seen: (ScrollIntoViewOptions | undefined)[] = []
		const landed = wrapper.get(`[data-row-id="${noteRow('nte_3')}"]`).element
		landed.scrollIntoView = (options?: boolean | ScrollIntoViewOptions) => {
			seen.push(options as ScrollIntoViewOptions)
		}
		Object.defineProperty(wrapper.get('[data-scroll-region]').element, 'clientHeight', {
			configurable: true,
			get: () => 120,
		})

		// What the panel becoming visible does.
		flushReveal()

		expect(seen).toEqual([{ block: 'nearest' }])
		// Focus is still deliberately left alone — a paste is a capture, not a
		// composition, and scrolling to a note is not the same as taking the keyboard
		// to it.
		expect(selection.focusedId.value).toBe(noteRow('nte_2'))
	})
})

// --- task-016: done filtering, per-section sort, creation dates ---------------

/**
 * Done notes in **both** sections, so the bulk delete's scope is observable
 * rather than assumed: `SPACE` has one done note and it is in the active
 * section, which every wrong scope would also produce.
 */
const DONE_IN_BOTH: Space = {
	...SPACE,
	notes: [
		{ ...SPACE.notes[0]!, done: true },
		{ ...SPACE.notes[1]!, body: 'second note', done: true },
		{
			id: 'nte_3',
			section: 'sec_a',
			order: 2,
			done: false,
			body: 'still to do',
			created: '2026-08-06T00:00:00Z',
			updated: '2026-08-06T00:00:00Z',
		},
		{
			id: 'nte_4',
			section: 'sec_b',
			order: 0,
			done: true,
			body: 'done elsewhere',
			created: '2026-08-07T00:00:00Z',
			updated: '2026-08-07T00:00:00Z',
		},
	],
}

describe('the done filter', () => {
	/** The file's outer hook opens every other test in `all`; these are the cases
	 *  that are about the filter, so they start where the panel really does. */
	beforeEach(() => {
		list.reset()
	})

	/**
	 * Mounts against a document with done notes in both sections, and records what
	 * `delete_notes` was asked to remove.
	 *
	 * The explicit `space.refresh()` is not ceremony: `initialize()` is memoised,
	 * so mounting after installing a different store does **not** re-pull it and
	 * the panel would render `SPACE` — where the only done note happens to be in
	 * the active section, which is precisely the case that cannot tell a correct
	 * scope from a wrong one.
	 */
	async function mountWithDoneInBoth() {
		const calls: string[][] = []
		mocks.invoke.mockImplementation(async (command: string, args?: { ids?: string[] }) => {
			if (command === 'get_active_space') return DONE_IN_BOTH
			if (command === 'delete_notes') {
				calls.push(args?.ids ?? [])
				return {
					...DONE_IN_BOTH,
					notes: DONE_IN_BOTH.notes.filter((entry) => !args?.ids?.includes(entry.id)),
				}
			}
			return baseInvoke(command)
		})
		const wrapper = await mountPanel()
		await space.refresh()
		await settle(3)
		return { wrapper, calls }
	}

	function renderedRows(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
		return wrapper.findAll('[data-note-row]').map((row) => row.attributes('data-row-id'))
	}

	/**
	 * AC1 / AC2, and the behaviour change: the panel opens on what is left to do.
	 *
	 * `nte_2` is `SPACE`'s only done note, so each of the three states is a
	 * different list and the walk through them is observable in one case.
	 */
	it('cycles through hiding done, done only, and everything', async () => {
		const wrapper = await mountPanel()
		const button = wrapper.get('[data-done-filter]')

		expect(renderedRows(wrapper)).toEqual([noteRow('nte_1')])

		await button.trigger('click')
		await settle(3)
		expect(renderedRows(wrapper)).toEqual([noteRow('nte_2')])

		await button.trigger('click')
		await settle(3)
		expect(renderedRows(wrapper)).toEqual([noteRow('nte_1'), noteRow('nte_2')])

		// Round to where it started, so every view is one press from every other.
		await button.trigger('click')
		await settle(3)
		expect(renderedRows(wrapper)).toEqual([noteRow('nte_1')])
	})

	/**
	 * The visible label is the state the *next* press produces — the reverse of
	 * `SortControl` beside it, because this filter's state is the list itself
	 * while a sort's is invisible. The accessible name says both, and ends with the
	 * visible text so it contains it.
	 */
	it('labels the view a press would produce, and names the one in effect', async () => {
		const wrapper = await mountPanel()
		const button = wrapper.get('[data-done-filter]')

		// One done note in `SPACE`, and the count rides on this offer alone.
		expect(button.text()).toContain('Done 1')
		expect(button.attributes('aria-label')).toBe('Unfinished notes only · press for Done 1')

		await button.trigger('click')
		await settle(2)
		expect(button.text()).toContain('All')
		expect(button.attributes('aria-label')).toBe('Done notes only · press for All')

		await button.trigger('click')
		await settle(2)
		expect(button.text()).toContain('Todo')
		expect(button.attributes('aria-label')).toBe('All notes · press for Todo')
	})

	/** Three states are not a toggle, and `aria-pressed` on one would announce
	 *  something false in at least one of them. */
	it('claims no pressed state', async () => {
		const wrapper = await mountPanel()
		expect(wrapper.get('[data-done-filter]').attributes('aria-pressed')).toBeUndefined()
	})

	/** AC10. The chip and the filter share a row that is not the search field's,
	 *  so neither can move the field — the structural guarantee, asserted rather
	 *  than trusted. */
	it('sits outside the search field’s row, beside the active-section chip', async () => {
		const wrapper = await mountPanel()
		const filter = wrapper.get('[data-done-filter]').element
		const field = wrapper.get('#panel-search').element

		expect(filter.parentElement?.parentElement?.contains(field)).toBe(false)
	})

	/** AC5. Nothing to purge, nothing to press. */
	it('offers the delete only inside the done view', async () => {
		const wrapper = await mountPanel()
		expect(wrapper.find('[data-delete-done]').exists()).toBe(false)

		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)
		expect(wrapper.find('[data-delete-done]').exists()).toBe(true)
	})

	/**
	 * At rest it is a trash icon and nothing else, so the accessible name is the
	 * only name it has — and it has to carry the scope, since this deletes the
	 * active section's done notes while the view behind it is document-wide.
	 */
	it('rests as an icon whose accessible name names the count and the section', async () => {
		const { wrapper } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		const button = wrapper.get('[data-delete-done]')
		expect(button.text()).toBe('')
		expect(button.attributes('aria-label')).toBe('Delete 2 done notes in Research')
	})

	/** AC6. One press asks, the second acts — and the first press must not delete
	 *  anything, which is the whole point of the confirmation. */
	it('asks before deleting, and the first press deletes nothing', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		const button = wrapper.get('[data-delete-done]')
		await button.trigger('click')
		await settle(2)

		expect(calls).toEqual([])
		// The label names the scope as well as the count — see the section-naming
		// case below for why the two cannot be left to be inferred from each other.
		expect(button.text()).toContain('Delete 2 in Research?')

		await button.trigger('click')
		await settle(3)
		expect(calls).toHaveLength(1)
	})

	/** AC9. `nte_4` is done and in `sec_b`, and must survive. */
	it('deletes the active section’s done notes and no others', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(4)

		// AC7's other half: one call, not one per note. The store pushes one
		// snapshot per `mutate`, so one call is one Ctrl+Z — the depth itself is
		// asserted in `store_fs.rs`, which is the only side that can see it.
		expect(calls).toHaveLength(1)
		expect(calls[0]).toEqual(['nte_1', 'nte_2'])
	})

	/** The undo affordance is the message, exactly as the singular delete's is. */
	it('says how to undo', async () => {
		const { wrapper } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(4)

		// The chord no longer has to be spelled in the sentence: the pill carries the
		// button that performs the same single step.
		expect(wrapper.text()).toContain('Deleted 2 done notes')
		expect(wrapper.get('[data-toast-action]').text()).toBe('Undo')
	})

	/**
	 * The default view narrows, so it is refused for the reason the done view is:
	 * an index read off a list missing every finished note is not the index
	 * `reorder_note` counts. This is the sharpest cost of the new default — manual
	 * reordering now asks for the `All` view first, and the message says so.
	 */
	it('refuses both reorder paths in the default view as well', async () => {
		const wrapper = await mountPanel()
		expect(wrapper.find('[data-drag-handle]').exists()).toBe(false)

		takeRow(noteRow('nte_1'))
		await settle(2)
		await wrapper.get('[role="grid"]').trigger('keydown', { key: 'ArrowDown', altKey: true })
		await settle(3)

		expect(mocks.invoke).not.toHaveBeenCalledWith('reorder_note', expect.anything())
		expect(wrapper.text()).toContain('Show all notes to reorder them.')

		// And it comes back on `All`, which is two presses away.
		list.setDoneFilter('all')
		await settle(2)
		expect(wrapper.find('[data-drag-handle]').exists()).toBe(true)
	})

	/**
	 * The other emptiness, and the reason the copy could not stay one sentence:
	 * the done view is empty when nothing is finished, the default view when
	 * everything is. Saying the wrong one states the exact opposite of the truth.
	 */
	it('explains a default view with nothing left to do', async () => {
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'get_active_space') {
				return { ...SPACE, notes: SPACE.notes.map((entry) => ({ ...entry, done: true })) }
			}
			return baseInvoke(command)
		})
		const wrapper = await mountPanel()
		await space.refresh()
		await settle(3)

		expect(wrapper.text()).toContain('Everything here is done.')
		// Not "No notes yet" as well: the space has notes, and a filter is why none
		// of them is on screen.
		expect(wrapper.text()).not.toContain('No notes yet')
	})

	/** The done view with nothing in it says so rather than going blank. */
	it('explains an empty done view instead of rendering nothing', async () => {
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'get_active_space') {
				return { ...SPACE, notes: SPACE.notes.map((entry) => ({ ...entry, done: false })) }
			}
			return baseInvoke(command)
		})
		const wrapper = await mountPanel()
		await space.refresh()
		await settle(3)

		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(3)

		expect(wrapper.text()).toContain('Nothing is done yet.')
	})

	/**
	 * The confirming label names the **section**, because the count and the view
	 * can legitimately disagree: the filter shows done notes document-wide, the
	 * delete takes the active section's alone (AC9). A bare "Delete 2?" over a list
	 * of three done notes reads as a bug, or is believed.
	 */
	it('names the section it would delete from, not just the count', async () => {
		const { wrapper } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		// Three done notes are on screen; only the two in `Research` are in scope.
		expect(wrapper.findAll('[data-note-row]')).toHaveLength(3)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(2)

		expect(wrapper.get('[data-delete-done]').text()).toContain('Delete 2 in Research?')
	})

	/**
	 * The confirmation re-arms on **which** notes it would delete, not how many.
	 *
	 * A count is not an identity, and the gap is reachable: marking one note done
	 * while another is unmarked leaves the total unchanged over a different set. A
	 * confirmation armed before that landed would delete notes it never offered.
	 */
	it('disarms when an equal-count set swaps underneath it', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(2)
		expect(wrapper.get('[data-delete-done]').text()).toContain('Delete 2 in Research?')

		// Still two done notes in `sec_a`, but a different two: `nte_1` is no longer
		// done and `nte_3` now is.
		mocks.invoke.mockImplementation(async (command: string, args?: { ids?: string[] }) => {
			if (command === 'get_active_space') {
				return {
					...DONE_IN_BOTH,
					notes: DONE_IN_BOTH.notes.map((entry) => {
						if (entry.id === 'nte_1') return { ...entry, done: false }
						if (entry.id === 'nte_3') return { ...entry, done: true }
						return entry
					}),
				}
			}
			if (command === 'delete_notes') {
				calls.push(args?.ids ?? [])
				return DONE_IN_BOTH
			}
			return baseInvoke(command)
		})
		await space.refresh()
		await settle(3)

		// The offer went away rather than silently re-aiming, so the next press arms
		// against the new set instead of deleting the old one. Disarmed is the
		// icon-only state, which is what having no label at all asserts here.
		expect(wrapper.get('[data-delete-done]').text()).toBe('')
		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(2)
		expect(calls).toEqual([])
	})

	/**
	 * A held Enter must not arm and confirm inside one hold. The browser
	 * synthesises a click from every repeat of the keydown, so without refusing the
	 * repeat the second one confirms a deletion the user never pressed twice.
	 */
	it('refuses the repeat of a held activation key', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		const button = wrapper.get('[data-delete-done]')
		// The first press arms, as an ordinary one does.
		await button.trigger('click')
		await settle(2)
		expect(button.text()).toContain('Delete 2 in Research?')

		// The key is still down. The repeat is declined at the source, so no click is
		// generated from it and nothing is confirmed.
		const repeat = new KeyboardEvent('keydown', {
			key: 'Enter',
			repeat: true,
			bubbles: true,
			cancelable: true,
		})
		button.element.dispatchEvent(repeat)
		await settle(2)

		expect(repeat.defaultPrevented).toBe(true)
		expect(calls).toEqual([])
		expect(button.text()).toContain('Delete 2 in Research?')
	})

	/** The second click of a double-click is the same gesture as the first, aimed
	 *  at a label that changed halfway through it. */
	it('does not let a double-click arm and confirm in one gesture', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		const button = wrapper.get('[data-delete-done]')
		await button.trigger('click', { detail: 1 })
		await settle(2)
		await button.trigger('click', { detail: 2 })
		await settle(3)

		expect(calls).toEqual([])
		expect(button.text()).toContain('Delete 2 in Research?')

		// A deliberate separate press still works.
		await button.trigger('click', { detail: 1 })
		await settle(3)
		expect(calls).toHaveLength(1)
	})
})

describe('sort', () => {
	/** AC14. The grip is the pointer's only path to a reorder, and an index read
	 *  off a permuted list means nothing — so it withdraws, exactly as it does
	 *  under a search. */
	it('withdraws the drag handle while a sort is active', async () => {
		const wrapper = await mountPanel()
		expect(wrapper.find('[data-drag-handle]').exists()).toBe(true)

		list.setSort('newest')
		await settle(2)

		expect(wrapper.find('[data-drag-handle]').exists()).toBe(false)
	})

	/** AC14's keyboard half, and AC15: refused with a message that names the
	 *  control which gives reordering back, then permitted again on Manual. */
	it('refuses Alt+Arrow while sorted and allows it again on Manual', async () => {
		const wrapper = await mountPanel()
		list.setSort('oldest')
		takeRow(noteRow('nte_1'))
		await settle(2)

		await wrapper.get('[role="grid"]').trigger('keydown', { key: 'ArrowDown', altKey: true })
		await settle(3)

		expect(mocks.invoke).not.toHaveBeenCalledWith('reorder_note', expect.anything())
		expect(wrapper.text()).toContain('Set the sort to Manual to reorder notes.')

		list.setSort('manual')
		await settle(2)
		expect(wrapper.find('[data-drag-handle]').exists()).toBe(true)
	})

	/**
	 * The mode is on the control itself, which is what a document-wide setting in
	 * the header buys over a per-section submenu: nothing has to be opened to read
	 * it, and there is no per-section marker to keep in step with it.
	 *
	 * The label is the state and it is blank on Manual — the order most lists are
	 * in is not worth a word — so the accessible name is what carries the mode when
	 * there is no visible text, and what says the press is about sorting at all.
	 */
	it('names the mode in effect and cycles through the three', async () => {
		const wrapper = await mountPanel()
		const button = wrapper.get('[data-sort-mode]')

		expect(button.text()).toBe('')
		expect(button.attributes('aria-label')).toContain('Manual order')

		await button.trigger('click')
		await settle(2)
		expect(list.sortMode.value).toBe('oldest')
		expect(button.text()).toContain('Oldest')

		await button.trigger('click')
		await settle(2)
		expect(list.sortMode.value).toBe('newest')
		expect(button.text()).toContain('Newest')

		// Round to where it started, so every mode is one press from every other.
		await button.trigger('click')
		await settle(2)
		expect(list.sortMode.value).toBe('manual')
		expect(button.text()).toBe('')
	})

	/** The section headers carry no sort marker any more: the mode is one
	 *  document-wide fact stated once in the header, and repeating it per section
	 *  would be the same sentence as many times as there are sections. */
	it('says nothing about the sort on the section headers', async () => {
		const wrapper = await mountPanel()
		list.setSort('newest')
		await settle(2)

		expect(wrapper.text()).not.toContain('Sorted newest first')
	})

	/**
	 * The done filter is the third reason reordering is refused, and it was missed
	 * when the filter was added.
	 *
	 * The reason is the search branch's, verbatim: a done-only list omits every
	 * unfinished note between two done ones, so an index read off the rendered rows
	 * is not the index `reorder_note` takes. Both paths have to refuse — the grip
	 * for the pointer, `reorderBlocked` for the keyboard and for anything that
	 * slips past the grip.
	 */
	it('refuses both reorder paths while the done filter is on', async () => {
		const wrapper = await mountPanel()
		list.setDoneFilter('done')
		await settle(3)

		// The pointer path: no handle, so there is nothing to start a drag from.
		expect(wrapper.find('[data-drag-handle]').exists()).toBe(false)

		// The keyboard path.
		takeRow(noteRow('nte_2'))
		await settle(2)
		await wrapper.get('[role="grid"]').trigger('keydown', { key: 'ArrowDown', altKey: true })
		await settle(3)
		expect(mocks.invoke).not.toHaveBeenCalledWith('reorder_note', expect.anything())
		expect(wrapper.text()).toContain('Show all notes to reorder them.')

		// And the commit itself, in case a drag is ever started some other way.
		await actions.finishDrag('nte_2', 'sec_a', 0)
		await settle(3)
		expect(mocks.invoke).not.toHaveBeenCalledWith('reorder_note', expect.anything())
	})

	/**
	 * `positionOf` answers in **document** coordinates, which is what its contract
	 * always said and what `finishDrag`'s no-op check needs — `useNoteDrag` counts
	 * the destination index over the whole section, so a position taken from the
	 * rendered rows compares two different coordinate systems.
	 *
	 * Collapse is the condition this is observable under. Every *other* way the
	 * rendered rows can disagree with the document — a query, the done filter, a
	 * non-manual sort — is refused outright by `reorderBlocked` before the no-op
	 * check runs, so the defect was unreachable rather than absent. A collapsed
	 * section is not refused, and it publishes an empty note list: reading the
	 * position off `visibleGroups` there returned -1, which never equals the index,
	 * so a drag that changed nothing went to the store and pushed an undo entry the
	 * user then had to press Ctrl+Z to get rid of.
	 */
	it('treats a no-op drag inside a collapsed section as a no-op', async () => {
		const wrapper = await mountPanel()
		sections.setCollapsed('sec_a', true)
		await settle(2)
		expect(wrapper.findAll('[data-note-row]')).toHaveLength(0)

		// `nte_2` is already at index 1 of `sec_a` in the document, so this asks for
		// the position it already holds.
		await actions.finishDrag('nte_2', 'sec_a', 1)
		await settle(3)

		expect(mocks.invoke).not.toHaveBeenCalledWith('reorder_note', expect.anything())
	})
})

describe('creation dates', () => {
	async function mountWithDates() {
		settingsPayload = { ...defaultSettings(), showCreated: true }
		const wrapper = await mountPanel()
		await settings.refresh()
		await settle(2)
		return wrapper
	}

	/** AC18. The setting ships off, so an upgrade shows exactly the cards it
	 *  showed before. */
	it('shows nothing by default', async () => {
		const wrapper = await mountPanel()
		expect(wrapper.find('time').exists()).toBe(false)
	})

	/** AC19, and the placement decision: below the body, last in the column. */
	it('shows a date under each note when the setting is on', async () => {
		const wrapper = await mountWithDates()

		const stamps = wrapper.findAll('time')
		expect(stamps).toHaveLength(2)
		// The machine-readable half is the stored instant, not the formatted text.
		expect(stamps[0]!.attributes('datetime')).toBe('2026-08-05T00:00:00Z')
		expect(stamps[0]!.text()).toContain('2026')
	})

	/**
	 * AC20. The store keeps `created` a plain string so a hand-edited value cannot
	 * make the document unloadable, which means an unreadable one reaches the card.
	 * It renders nothing — not a dash, which would claim the note has no date, and
	 * certainly not a substituted one.
	 */
	it('renders no line at all for a date it cannot read', async () => {
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'get_active_space') {
				return {
					...SPACE,
					notes: [{ ...SPACE.notes[0]!, created: 'yesterday afternoon' }, SPACE.notes[1]!],
				}
			}
			return baseInvoke(command)
		})
		const wrapper = await mountWithDates()
		await space.refresh()
		await settle(3)

		// The readable one still shows; the broken one contributes nothing.
		expect(wrapper.findAll('time')).toHaveLength(1)
		expect(wrapper.text()).not.toContain('yesterday afternoon')
	})
})

// --- the status toast --------------------------------------------------------

describe('the status toast', () => {
	/**
	 * The pill overlays the last rows of the list for five seconds after every
	 * action, so it must not eat presses aimed at what is underneath it. The band,
	 * the pill and everything in it are click-through; the action button re-enables
	 * pointer events for itself alone, being the only part that does anything.
	 */
	it('is click-through except for its action button', async () => {
		const wrapper = await mountPanel()
		status.setMessage('Copied 1 note', { label: 'Undo', run: () => {} })
		await wrapper.vm.$nextTick()

		const pill = wrapper.get('[data-status-toast]')
		expect(pill.classes()).not.toContain('pointer-events-auto')
		expect(pill.element.parentElement?.className).toContain('pointer-events-none')
		expect(wrapper.get('[data-toast-action]').classes()).toContain('pointer-events-auto')
	})

	/** One pill, whatever happened. A second message takes the surface rather than
	 *  opening a second one under it. */
	it('replaces the message rather than stacking a second pill', async () => {
		const wrapper = await mountPanel()
		status.setMessage('Copied 1 note')
		await wrapper.vm.$nextTick()
		status.setMessage('Copied 3 notes')
		await wrapper.vm.$nextTick()

		expect(wrapper.findAll('[data-status-toast]')).toHaveLength(1)
		expect(wrapper.get('[data-status-toast]').text()).toContain('Copied 3 notes')
	})

	/**
	 * The timer replaced clearing on the next user action, and the two cannot
	 * coexist: an `Undo` button would be gone before the pointer reached it, since
	 * moving toward it is a user action somewhere.
	 */
	it('lasts five seconds rather than until the next keypress', async () => {
		const wrapper = await mountPanel()
		vi.useFakeTimers()
		try {
			status.setMessage('Copied 1 note')
			await wrapper.vm.$nextTick()
			expect(wrapper.find('[data-status-toast]').exists()).toBe(true)

			await wrapper.trigger('keydown', { key: 'Escape' })
			expect(wrapper.find('[data-status-toast]').exists()).toBe(true)

			vi.advanceTimersByTime(5000)
			await wrapper.vm.$nextTick()
			expect(wrapper.find('[data-status-toast]').exists()).toBe(false)
		} finally {
			vi.useRealTimers()
		}
	})

	/**
	 * Marking a note done in the default view is an action whose only visible
	 * result is a row leaving the list, which is exactly why the toast carries the
	 * way back. One press of `Undo` is one store step — the same one `Ctrl+Z`
	 * takes — and a batch is already one step, so it restores all of it.
	 */
	it('reports a note moved to Done and offers one undo step', async () => {
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'set_notes_done') {
				return { ...SPACE, notes: SPACE.notes.map((entry) => ({ ...entry, done: true })) }
			}
			return baseInvoke(command)
		})
		// The panel's real default, which the file's outer hook trades for `all`.
		list.reset()
		const wrapper = await mountPanel()
		// The fixture's done note is already hidden here.
		expect(wrapper.findAll('[data-note-row]')).toHaveLength(1)

		selection.select('nte_1')
		await settle(2)
		await actions.toggleDone()
		await settle(4)

		expect(wrapper.findAll('[data-note-row]')).toHaveLength(0)
		expect(wrapper.get('[data-status-toast]').text()).toContain('Moved 1 note to Done')

		await wrapper.get('[data-toast-action]').trigger('click')
		await settle(3)

		expect(mocks.invoke).toHaveBeenCalledWith('undo')
		// The offer is spent, so the pill does not stay up inviting a second press
		// at a step that has already been taken.
		expect(wrapper.find('[data-toast-action]').exists()).toBe(false)
	})

	/** The reverse direction reports too, and for the same reason: in the done view
	 *  it is unmarking that makes a row disappear. */
	it('reports a note moved out of Done', async () => {
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'set_notes_done') {
				return { ...SPACE, notes: SPACE.notes.map((entry) => ({ ...entry, done: false })) }
			}
			return baseInvoke(command)
		})
		const wrapper = await mountPanel()
		list.setDoneFilter('done')
		await settle(2)

		selection.select('nte_2')
		await settle(2)
		await actions.toggleDone()
		await settle(4)

		expect(wrapper.get('[data-status-toast]').text()).toContain('Moved 1 note out of Done')
	})

	/**
	 * The row that vanishes takes focus with it, and it is handed on the same way a
	 * delete hands it on — through the row reconciliation already chose, rather
	 * than through a second rule about where focus goes.
	 */
	it('hands focus on when the marked note leaves the default view', async () => {
		const THREE: Space = {
			...SPACE,
			notes: ['nte_1', 'nte_2', 'nte_3'].map((id, order) => ({
				id,
				section: 'sec_a',
				order,
				done: false,
				body: id,
				created: '2026-08-05T00:00:00Z',
				updated: '2026-08-05T00:00:00Z',
			})),
		}
		mocks.invoke.mockImplementation(async (command: string, args?: { ids?: string[] }) => {
			if (command === 'get_active_space') return THREE
			if (command === 'set_notes_done') {
				return {
					...THREE,
					notes: THREE.notes.map((entry) =>
						args?.ids?.includes(entry.id) ? { ...entry, done: true } : entry,
					),
				}
			}
			return baseInvoke(command)
		})
		// The panel's real default, which the file's outer hook trades for `all`.
		list.reset()
		const wrapper = await mountPanel()
		await space.refresh()
		await settle(3)

		selection.select('nte_2')
		takeRow(noteRow('nte_2'))
		await settle(2)
		await actions.toggleDone()
		await settle(5)

		expect(selection.focusedId.value).toBe(noteRow('nte_3'))
		expect(document.activeElement).toBe(wrapper.get(`[data-row-id="${noteRow('nte_3')}"]`).element)
	})
})
