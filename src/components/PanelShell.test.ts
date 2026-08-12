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
import { usePalette } from '@/composables/usePalette'
import { useSections } from '@/composables/useSections'
import { flushReveal, noteRow, sectionRow, takeRow, useSelection } from '@/composables/useSelection'
import { useImageViewer } from '@/composables/useImageViewer'
import { useSettings } from '@/composables/useSettings'
import { useSpace } from '@/composables/useSpace'
import { useSpaces } from '@/composables/useSpaces'
import { useStatusMessage } from '@/composables/useStatusMessage'
import type { MarkdownFormat, NoteSelection, Space, StoreStatus } from '@/composables/useSpace'

const actions = useNoteActions()
const drag = useNoteDrag()
const interaction = useInteractionMode()
const editor = useNoteEditor()
const list = useNoteList()
const palette = usePalette()
const search = useNoteSearch()
const sections = useSections()
const selection = useSelection()
const settings = useSettings()
const viewer = useImageViewer()
const space = useSpace()
const spaces = useSpaces()
const status = useStatusMessage()

// happy-dom implements no Web Animations API. The list's enter and leave hooks
// skip animation entirely when `el.animate` is missing, so the suite would pass
// without this — but with the stub in place the hooks run their real path, which
// is the product behaviour and only the environment is missing.
//
// It has to *finish*, not merely exist: the `<TransitionGroup>` holds a removed
// row in the DOM until its leave animation reports done, so a stub that never
// fires `finish` leaves every filtered-out row on screen forever.
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

/** Task-026's shipped default: off, unconfigured, nothing to report. The Share
 *  section renders from this, and the note context menu's **Send to my other
 *  device** stays disabled under it. */
const SHARE_CONFIG = {
	enabled: false,
	relayUrl: '',
	role: 'first',
	tokenSet: false,
	secretSet: false,
	configured: false,
	lastError: null,
}

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
	summonFallback: null,
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
		enterKey: 'submit',
		alwaysOnTop: true,
		showCreated: false,
	}
}

/**
 * What `render_notes_markdown` answers, since task-024 moved both the Markdown
 * rendering and the resolution of a selection into Rust.
 *
 * **The text is the arguments, JSON-encoded — never a rendering.** The
 * formatting contract lives in `copper-core/tests/markdown.rs` now, byte for
 * byte over the corpus the deleted `noteMarkdown.test.ts` used to hold, and a
 * mock that reimplemented it here would only assert this file's copy of it.
 * What the copy tests assert instead are the two things still owned on this
 * side: the `NoteSelection` each affordance resolves to, and that whatever came
 * back reached `clipboard_write_text` unaltered.
 *
 * `count` is resolved against the applied document, because `writeCopy` gates
 * on it — a copy of no notes writes nothing and says nothing — and that gate is
 * a frontend rule these tests still own. Counting which notes a selection names
 * is not formatting them, and the mock has to agree with the real command about
 * it for the gate to mean anything.
 */
function renderedNotes(args: Record<string, unknown> | undefined) {
	if (!args) {
		throw new Error(
			'render_notes_markdown reached a mock that drops its arguments — forward them as ' +
				'baseInvoke(command, args)',
		)
	}
	return {
		text: JSON.stringify(args),
		count: selectedNotes(args.selection as NoteSelection).length,
	}
}

/**
 * The resolver's rules, as far as *counting* goes — including its two refusals,
 * so a scope naming something the document does not hold fails here as it does
 * in Rust rather than quietly answering zero.
 */
function selectedNotes(selection: NoteSelection) {
	const notes = space.space.value?.notes ?? []
	if (selection.kind === 'document') return notes

	if (selection.kind === 'section') {
		if (!space.sections.value.some((section) => section.id === selection.id)) {
			throw { kind: 'not-found', message: `no such section: ${selection.id}` }
		}
		return notes.filter((note) => note.section === selection.id)
	}

	const missing = selection.ids.find((id) => !notes.some((note) => note.id === id))
	if (missing !== undefined) throw { kind: 'not-found', message: `no such note: ${missing}` }
	return notes.filter((note) => selection.ids.includes(note.id))
}

/** The store as every test finds it. Named so a test that replaces it can put it
 *  back — see the teardown below. */
async function baseInvoke(command: string, args?: Record<string, unknown>) {
	if (command === 'get_active_space') return SPACE
	if (command === 'get_status') return STATUS
	if (command === 'get_settings') return settingsPayload
	// The list header's controls remember themselves through this on every
	// change; the store's answer is the merged file.
	if (command === 'update_settings') {
		settingsPayload = { ...settingsPayload, ...(args?.patch as Record<string, unknown>) }
		return settingsPayload
	}
	if (command === 'get_shortcut_state') return SHORTCUTS
	if (command === 'get_autostart_enabled') return false
	if (command === 'get_share_config') return SHARE_CONFIG
	if (command === 'render_notes_markdown') return renderedNotes(args)
	if (command === 'clipboard_write_text') return null
	// Task-013's zero-focus paste. The text branch is a capture, so it reaches
	// `add_note` rather than `submit_entry`; the other branch asks `attach_paste`
	// what the clipboard holds, and an empty list is its "there was text, or
	// nothing" answer.
	if (command === 'add_note') return { space: SPACE, noteId: 'nte_1' }
	// The split half of a list paste: one command for the whole batch.
	if (command === 'add_notes') return { space: SPACE, noteIds: ['nte_1'] }
	if (command === 'attach_paste') return []
	// The two notes of `sec_a` become one, keeping the first id — which is what
	// `merge_notes` does, and what makes the survivor's row disappear when that
	// section happens to be collapsed.
	if (command === 'merge_notes') return MERGED
	if (command === 'hide_panel') return null
	if (command === 'minimize_panel') return null
	if (command === 'quit_app') return null
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
	// Pinned wide open rather than trusted to the default, which has changed once
	// already: `SPACE`'s second note is done, and under a narrowing default every
	// case in this file that is about something else (the grid, the menus, copy,
	// drag, the editor) would be asserting against a one-note list for a reason it
	// never mentions. The done filter's own block is where the three states are
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
	// drag handle it goes looking for. `hydrate(null)` is the defaults — the
	// space-switch reset this used to lean on is gone with the controls' memory.
	list.hydrate(null)
	selection.clear()
	// Interaction mode belongs on that list for the same reason and was missing
	// from it: with a row still in the mode, the grid's key handler declines every
	// press but Tab, so a later test's arrow keys move nothing and fail with focus
	// simply sitting where it started.
	interaction.exit()
	// And so does the image viewer, whose overlay would otherwise still be up in
	// the next test — declining every chord and swallowing the Escape ladder.
	viewer.close()
	// The command palette is on that list for exactly the same reason, and it is
	// the one a stray `Ctrl+K` leaves behind: it declines every chord through the
	// shell's overlay guard, so the next test's arrow keys and Delete do nothing
	// and fail with the panel simply sitting there.
	palette.close()
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
 * the post-`nextTick` DOM restore, the re-render, and the list's leave
 * animation — which only takes a removed row back out of the DOM on `finish`.
 *
 * A macrotask per turn rather than `nextTick`, because several of those steps
 * are promises chained behind an `invoke`, and a flush does not reach the end of
 * them.
 */
async function settle(turns = 4) {
	for (let i = 0; i < turns; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

/** Sonner keeps a dismissed toast in the DOM for its 200ms exit animation
 *  (`TIME_BEFORE_UNMOUNT`), so an absence assertion after a dismissal has to
 *  outwait real time — `settle`'s zero-delay turns never reach it. */
async function toastGone() {
	await new Promise((resolve) => setTimeout(resolve, 250))
}

async function mountPanel() {
	panel = mount(PanelShell, { attachTo: document.body })
	await settle(6)
	return panel as ReturnType<typeof mount<typeof PanelShell>>
}

/**
 * Every request a copy affordance made, in order — which notes it asked for and
 * in which of the three renderings.
 *
 * This is the assertion the copy tests are built on. Since task-024 the panel
 * decides only *which* notes a gesture means; the text is `copper_core::markdown`'s,
 * and asserting it here would restate a contract `copper-core/tests/markdown.rs`
 * already holds.
 */
function renderCalls() {
	return mocks.invoke.mock.calls
		.filter((call) => call[0] === 'render_notes_markdown')
		.map((call) => call[1] as { selection: NoteSelection; format: MarkdownFormat })
}

function lastRender() {
	return renderCalls().at(-1) ?? null
}

/** The last text written to the clipboard, or null when nothing was. */
function copied() {
	const written = mocks.invoke.mock.calls.filter((call) => call[0] === 'clipboard_write_text')
	return (written.at(-1)?.[1] as { text: string } | undefined)?.text ?? null
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

describe('the grid tab order', () => {
	it('puts every row in the tab order and none of their controls', async () => {
		const wrapper = await mountPanel()

		// The 2026-08-11 model, reversing sections-only: every row is a permanent
		// sequential stop, so Tab walks bands and notes alike.
		for (const row of wrapper.findAll('[data-section-row]')) {
			expect(row.attributes('tabindex')).toBe('0')
		}
		for (const row of wrapper.findAll('[data-note-row]')) {
			expect(row.attributes('tabindex')).toBe('0')
		}

		// The rows are the whole order only if every interactive descendant stays
		// out of it — F2 is the way in to those.
		for (const button of wrapper.find('[role="grid"]').findAll('button')) {
			expect(button.attributes('tabindex')).toBe('-1')
		}
	})

	it('moves Tab like an arrow, ring and focus together', async () => {
		// Focus and selection move together on every plain traversal, Tab now
		// included: a Tab onto a note without the ring would leave two rows
		// claiming to be where the user is — measured live 2026-08-11, when the
		// grid left this move to the browser and a flag that a microtask
		// checkpoint killed before the focus move it was waiting for.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await settle(2)

		const grid = wrapper.get('[role="grid"]').element
		grid.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }))
		await settle(2)

		expect(selection.focusedId.value).toBe(noteRow('nte_2'))
		expect(selection.selectedIds.value).toEqual(['nte_2'])

		grid.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true }))
		await settle(2)

		expect(selection.focusedId.value).toBe(noteRow('nte_1'))
		expect(selection.selectedIds.value).toEqual(['nte_1'])
	})

	it('clears the selection when Tab lands on a section band', async () => {
		// The other half of `landOn`'s rule, applied to the same landing.
		const wrapper = await mountPanel()
		selection.select('nte_2')
		await settle(2)

		wrapper
			.get('[role="grid"]')
			.element.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }))
		await settle(2)

		expect(selection.focusedId.value).toBe('s:sec_b')
		expect(selection.selectedIds.value).toEqual([])
	})

	it('leaves an edge Tab to the browser, selection intact', async () => {
		// A press at either end is how Tab leaves the grid, so it must stay
		// unprevented — and the selection survives it, as it survives a click
		// outside the list.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		takeRow(sectionRow('sec_a'))
		await settle(2)

		const press = new KeyboardEvent('keydown', {
			key: 'Tab',
			shiftKey: true,
			bubbles: true,
			cancelable: true,
		})
		const proceeded = wrapper.get('[role="grid"]').element.dispatchEvent(press)
		await settle(2)

		expect(proceeded).toBe(true)
		expect(selection.focusedId.value).toBe('s:sec_a')
		expect(selection.selectedIds.value).toEqual(['nte_1'])
	})

	it('leaves the selection alone when focus arrives without a Tab', async () => {
		// Ctrl+Arrow's quiet `syncDomFocus` and a plain click land on a row
		// through the same focusin; neither may write the selection there — the
		// first is quiet by design, the second selects in its own handler.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await settle(2)

		wrapper
			.get(`[data-row-id="${noteRow('nte_2')}"]`)
			.element.dispatchEvent(new FocusEvent('focusin', { bubbles: true }))
		await settle(2)

		expect(selection.focusedId.value).toBe(noteRow('nte_2'))
		expect(selection.selectedIds.value).toEqual(['nte_1'])
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
		expect(selected.classes('focus-inset')).toBe(false)

		// The unselected row keeps it: that is the case where focus and selection
		// genuinely differ, and the only ring it can wear. `focus-inset` — the
		// same crisp 2px accent outline the section bands and checkbox wear, one
		// focus language everywhere (2026-08-10; the halo it replaced read as a
		// muddy band, and the "washed-out crisp edge" that justified the halo was
		// the stuck outline-color transition, since fixed).
		const other = wrapper.get(`[data-row-id="${noteRow('nte_2')}"]`)
		expect(other.classes()).toContain('focus-inset')
	})
})

describe('the section delete confirm', () => {
	function confirmPopover() {
		return document.querySelector<HTMLElement>('[data-slot="popover-content"]')
	}

	function pressDelete(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
		wrapper
			.get('[role="grid"]')
			.element.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }))
	}

	it('asks on a bare Delete and lands focus on Cancel', async () => {
		const wrapper = await mountPanel()
		takeRow(sectionRow('sec_a'))
		await settle(2)

		pressDelete(wrapper)
		await settle(3)

		const popover = confirmPopover()
		expect(popover?.textContent).toContain('Delete “Research” and its 2 notes?')
		// The first press deletes nothing — that is the point of asking.
		expect(mocks.invoke).not.toHaveBeenCalledWith('delete_section', expect.anything())
		// The safe control takes the autofocus, so a held or doubled Delete can
		// never land on the destructive one.
		expect(document.activeElement).toBe(popover?.querySelector('[data-section-delete-cancel]'))
	})

	/** Ctrl+D is Delete's alias, and the section half answers it too — an alias
	 *  that deleted notes but not sections would be two keys that agree everywhere
	 *  except on a heading. */
	it('asks on Ctrl+D exactly as on a bare Delete', async () => {
		const wrapper = await mountPanel()
		takeRow(sectionRow('sec_a'))
		await settle(2)

		wrapper
			.get('[role="grid"]')
			.element.dispatchEvent(
				new KeyboardEvent('keydown', { key: 'd', ctrlKey: true, bubbles: true }),
			)
		await settle(3)

		expect(confirmPopover()?.textContent).toContain('Delete “Research” and its 2 notes?')
		expect(mocks.invoke).not.toHaveBeenCalledWith('delete_section', expect.anything())
	})

	it('deletes the section and its notes on the confirming press', async () => {
		const wrapper = await mountPanel()
		takeRow(sectionRow('sec_a'))
		await settle(2)
		pressDelete(wrapper)
		await settle(3)

		mocks.invoke.mockImplementationOnce(async () => ({
			...SPACE,
			activeSection: 'sec_b',
			sections: SPACE.sections.filter((section) => section.id !== 'sec_a'),
			notes: [],
		}))
		confirmPopover()!.querySelector<HTMLElement>('[data-section-delete-confirm]')!.click()
		await settle(4)

		expect(mocks.invoke).toHaveBeenCalledWith('delete_section', { id: 'sec_a' })
		expect(confirmPopover()).toBeNull()
		// One undo covers the section and its notes together.
		expect(wrapper.text()).toContain('Deleted “Research” and 2 notes')
		expect(wrapper.get('[data-sonner-toast] [data-action]').text()).toBe('Undo')
		// The header row died with the section; the DOM half of focus follows the
		// roving target to the nearest surviving row.
		expect(selection.focusedId.value).toBe('s:sec_b')
	})

	it('moves between Cancel and Delete on the arrow keys, cycling', async () => {
		const wrapper = await mountPanel()
		takeRow(sectionRow('sec_a'))
		await settle(2)
		pressDelete(wrapper)
		await settle(3)

		const popover = confirmPopover()!
		const arrow = (key: string) =>
			document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }))

		// Focus opens on Cancel; one arrow reaches the other offer, in either
		// direction — the popover's two controls are otherwise Tab-only.
		arrow('ArrowRight')
		expect(document.activeElement).toBe(popover.querySelector('[data-section-delete-confirm]'))
		arrow('ArrowRight')
		expect(document.activeElement).toBe(popover.querySelector('[data-section-delete-cancel]'))
		arrow('ArrowLeft')
		expect(document.activeElement).toBe(popover.querySelector('[data-section-delete-confirm]'))
		// Arrows move focus only; nothing was pressed.
		expect(mocks.invoke).not.toHaveBeenCalledWith('delete_section', expect.anything())
	})

	it('cancels back to the row the question was asked from', async () => {
		const wrapper = await mountPanel()
		takeRow(sectionRow('sec_a'))
		await settle(2)
		pressDelete(wrapper)
		await settle(3)

		confirmPopover()!.querySelector<HTMLElement>('[data-section-delete-cancel]')!.click()
		await settle(3)

		expect(confirmPopover()).toBeNull()
		expect(mocks.invoke).not.toHaveBeenCalledWith('delete_section', expect.anything())
		expect(document.activeElement).toBe(wrapper.get('[data-row-id="s:sec_a"]').element)
	})

	it('leaves Delete to the selection a focused header holds', async () => {
		// `selectSection` parks focus on the header with the section's notes
		// selected precisely so the next action takes them (the target rule); the
		// confirm claims only the bare header, where Delete used to be a no-op.
		const wrapper = await mountPanel()
		mocks.invoke.mockImplementation(async (command: string, args?: { ids?: string[] }) => {
			if (command === 'delete_notes') {
				return { ...SPACE, notes: SPACE.notes.filter((note) => !args?.ids?.includes(note.id)) }
			}
			return baseInvoke(command)
		})
		selection.selectSection('sec_a')
		await settle(2)

		pressDelete(wrapper)
		await settle(4)

		expect(confirmPopover()).toBeNull()
		expect(mocks.invoke).toHaveBeenCalledWith('delete_notes', { ids: ['nte_1', 'nte_2'] })
	})

	it('refuses the last section with a message, not a question', async () => {
		const wrapper = await mountPanel()
		await installDocument({ ...SPACE, sections: [SPACE.sections[0]!] })
		await settle(2)
		takeRow(sectionRow('sec_a'))
		await settle(2)

		pressDelete(wrapper)
		await settle(3)

		expect(confirmPopover()).toBeNull()
		expect(mocks.invoke).not.toHaveBeenCalledWith('delete_section', expect.anything())
		expect(wrapper.text()).toContain('The last section cannot be deleted.')
	})

	it('withdraws the question when the offer changes underneath it', async () => {
		const wrapper = await mountPanel()
		takeRow(sectionRow('sec_a'))
		await settle(2)
		pressDelete(wrapper)
		await settle(3)
		expect(confirmPopover()).not.toBeNull()

		// A capture landing in the section mid-confirm: the count in the question
		// is no longer the count the press would delete.
		await installDocument({
			...SPACE,
			notes: [
				...SPACE.notes,
				{ ...SPACE.notes[0]!, id: 'nte_new', order: 2, body: 'landed mid-confirm' },
			],
		})
		await settle(3)

		expect(confirmPopover()).toBeNull()
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
	it('keeps its placeholder static, leaving the space name to the section chip', async () => {
		const wrapper = await mountPanel()

		expect(wrapper.find('#composer').attributes('placeholder')).toBe('Add a note or a prompt…')
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
		expect(labels).toContain('Keep your version')
		expect(labels).toContain('Use the version on disk')

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

describe('the section menu’s done purge', () => {
	async function openMenuOn(wrapper: Awaited<ReturnType<typeof mountPanel>>, sectionId: string) {
		// At the gridcell, not the row: the menu trigger listens there — the row
		// element must stay bare so `TransitionGroup` can animate it — and a
		// dispatch at the parent never reaches a descendant's listener. A real
		// right-click always lands inside the cell, which fills the row.
		await wrapper.find(`[data-row-id="s:${sectionId}"] [role="gridcell"]`).trigger('contextmenu')
		await settle()
		const content = document.querySelector<HTMLElement>('[data-slot="context-menu-content"]')
		expect(content, 'the section menu did not open').not.toBeNull()
		return content!
	}

	function doneRow(menu: HTMLElement) {
		return [...menu.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((entry) =>
			entry.textContent?.includes('Delete done notes'),
		)
	}

	it('deletes this section’s done notes, named with their count', async () => {
		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
			if (command === 'delete_notes') {
				const { ids } = args as { ids: string[] }
				return { ...SPACE, notes: SPACE.notes.filter((note) => !ids.includes(note.id)) }
			}
			return baseInvoke(command, args)
		})
		const wrapper = await mountPanel()
		const item = doneRow(await openMenuOn(wrapper, 'sec_a'))
		expect(item?.textContent).toContain('Delete done notes (1)')

		item!.click()
		await settle(4)

		// The right-clicked section’s done note and nothing else — one command,
		// one undo step, and the same toast the header control’s purges carry.
		expect(mocks.invoke).toHaveBeenCalledWith('delete_notes', { ids: ['nte_2'] })
		expect(wrapper.text()).toContain('Deleted 1 done note')
		expect(wrapper.get('[data-sonner-toast] [data-action]').text()).toBe('Undo')
	})

	it('disables the offer when the section has nothing done', async () => {
		// Disabled rather than hidden, like every other row here, so the menu
		// keeps its shape and the section delete below cannot inherit a press
		// aimed at this row from memory. Bare, with no count: zero is what the
		// disabled state already says.
		const wrapper = await mountPanel()
		const item = doneRow(await openMenuOn(wrapper, 'sec_b'))

		expect(item?.textContent).toContain('Delete done notes')
		expect(item?.textContent).not.toContain('(')
		expect(item?.getAttribute('data-disabled')).not.toBeNull()
	})
})

describe('the header drag region', () => {
	/**
	 * The property is invisible in a screenshot and easy to undo.
	 *
	 * Tauri reads `data-tauri-drag-region` off the element the mousedown actually
	 * lands on — which is why the attribute sits on three elements, not one. The
	 * header's own padding is the outer grab handle; the two row containers are
	 * the gaps *between* controls, where a press lands on the row and not on the
	 * header behind it. Before they carried it, every gap in the header was dead
	 * to dragging (user report, 2026-08-11). What may never carry it is a
	 * control: a drag region swallows the pointer events of whatever it is on,
	 * so an attribute on a button is a button that cannot be clicked.
	 */
	it('is the header and its bare rows, never a control', async () => {
		const wrapper = await mountPanel()
		const header = wrapper.get('header')

		expect(header.attributes('data-tauri-drag-region')).toBeDefined()
		// Both rows: the search row's gaps and the chip row's, the strip included.
		expect(wrapper.findAll('header div[data-tauri-drag-region]').length).toBeGreaterThanOrEqual(3)
		// No control claims it — that is the half that makes them clickable.
		expect(wrapper.find('header :is(button, input, a)[data-tauri-drag-region]').exists()).toBe(
			false,
		)
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

		// A capture arriving mid-search still has to land in the section the chip
		// names — the chip, not the placeholder, is where that promise lives now.
		expect(mocks.invoke).not.toHaveBeenCalledWith('set_active_section', expect.anything())
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

		// The cell, not the row — the trigger listens there. See `openMenuOn`.
		await wrapper.find('[data-row-id="n:nte_1"] [role="gridcell"]').trigger('contextmenu')
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

		await wrapper.find('[data-row-id="n:nte_1"] [role="gridcell"]').trigger('contextmenu')
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

describe('the window controls', () => {
	it('minimizes through the Rust command, not a JS window call', async () => {
		const wrapper = await mountPanel()

		await wrapper.find('[aria-label="Minimize"]').trigger('click')

		expect(mocks.invoke).toHaveBeenCalledWith('minimize_panel')
	})

	it('quits through the Rust command from the overflow menu', async () => {
		const wrapper = await mountPanel()

		await wrapper.find('[aria-label="More actions"]').trigger('click')
		await settle(3)
		const item = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((row) =>
			row.textContent?.includes('Quit Copper'),
		)
		expect(item, 'the Quit Copper item is missing').toBeTruthy()
		item!.click()
		await settle(2)

		expect(mocks.invoke).toHaveBeenCalledWith('quit_app')
	})
})

/**
 * Two hops since task-024, and the tests below assert one each: the panel asks
 * `render_notes_markdown` for the notes it targeted, and writes whatever comes
 * back through the Rust clipboard module. The Markdown itself is
 * `copper-core/tests/markdown.rs`'s.
 */
describe('copy', () => {
	it('asks the renderer for the targeted notes and writes back what it answers', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')

		await wrapper.trigger('keydown', { key: 'c', ctrlKey: true })
		await settle(2)

		// The scope: `Ctrl+C` targets the selection, by id, as raw bodies.
		expect(lastRender()).toEqual({ selection: { kind: 'ids', ids: ['nte_1'] }, format: 'bodies' })
		// The plumbing: the answer reached the clipboard unaltered. Nothing on this
		// side reads, trims or re-joins it.
		expect(copied()).toBe(JSON.stringify(lastRender()))
	})

	/**
	 * Task-014 ranks a section's rows by score; `actionableNoteIds` is what an
	 * *action* targets and its contract is the document's order. Letting the
	 * ranking reach it would make a multi-note copy come out in whatever order the
	 * query happened to score them, which is a silent change to the clipboard's
	 * contents for a search the user has since cleared.
	 *
	 * Still a frontend test after task-024, and deliberately: *producing* the
	 * canonical id list is `targetIds()`'s job even though rendering from it moved
	 * to Rust. `src-tauri/tests/markdown.rs` asserts the other half — that a
	 * scrambled list would render identically anyway — so the property now holds
	 * at both ends.
	 */
	it('names the notes in document order even while a search has reordered the rows', async () => {
		const ranked: Space = {
			...SPACE,
			notes: [
				{ ...SPACE.notes[0]!, id: 'nte_1', section: 'sec_a', body: 'a resort' },
				{ ...SPACE.notes[0]!, id: 'nte_2', section: 'sec_a', body: 'sort by date' },
			],
		}
		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
			if (command === 'get_active_space') return ranked
			return baseInvoke(command, args)
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
		await settle(2)

		expect(lastRender()?.selection).toEqual({ kind: 'ids', ids: ['nte_1', 'nte_2'] })
	})

	/** `Copy as list` is a second format over the same scope, and its own chord.
	 *  Nothing else in the suite would notice if the two swapped. */
	it('asks for the list format from its own chord', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')

		await wrapper.trigger('keydown', { key: 'C', ctrlKey: true, shiftKey: true })
		await settle(4)

		expect(lastRender()).toEqual({ selection: { kind: 'ids', ids: ['nte_1'] }, format: 'list' })
		expect(copied()).toBe(JSON.stringify(lastRender()))
	})

	/**
	 * The count is the command's, not this side's, which is the whole reason it
	 * travels back with the text.
	 *
	 * The two can only differ in the app when the document moved between the
	 * gesture resolving its targets and the render resolving them again — a CLI
	 * write, a `$EDITOR` write-back, a git checkout. Here they are made to differ
	 * on purpose, because a panel that recomputed the number locally would pass
	 * every other test in this file.
	 */
	it('reports the count the command answered with rather than one of its own', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
			if (command === 'render_notes_markdown') return { text: 'rendered', count: 3 }
			return baseInvoke(command, args)
		})

		await actions.copyNotes()
		await settle(2)

		expect(copied()).toBe('rendered')
		expect(wrapper.text()).toContain('Copied 3 notes')
	})

	/**
	 * A selection naming a note the document no longer holds is refused outright
	 * rather than silently narrowed — a copy that dropped a note would put fewer
	 * notes on the clipboard than the message claims. The panel's part is to say
	 * so and leave whatever was on the clipboard alone.
	 */
	it('says so and keeps the clipboard when the render is refused', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
			if (command === 'render_notes_markdown') {
				throw { kind: 'not-found', message: 'no such note: nte_1' }
			}
			return baseInvoke(command, args)
		})

		await actions.copyNotes()
		await settle(2)

		expect(copied()).toBeNull()
		expect(wrapper.text()).toContain("Couldn't copy those notes.")
	})

	it('sends the whole selection as one request and confirms the count', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		selection.extendTo('nte_2')

		await wrapper.trigger('keydown', { key: 'c', ctrlKey: true })
		await settle(4)

		// One request for both notes, not one per note.
		expect(renderCalls()).toEqual([
			{ selection: { kind: 'ids', ids: ['nte_1', 'nte_2'] }, format: 'bodies' },
		])
		expect(copied()).toBe(JSON.stringify(lastRender()))
		// Singular and plural are separate whole strings, never `note(s)` — and the
		// number is the count the command answered with, not one recomputed here
		// from a document snapshot that may have moved.
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
		await settle(2)

		// Nothing is rendered either: the chord declines before it resolves a scope,
		// so the browser's own copy is the only thing that ran.
		expect(renderCalls()).toHaveLength(0)
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
		await field.trigger('keydown', { key: 'Enter' })
		await settle(3)

		// Editing a body must never be able to delete the note being edited.
		expect(mocks.invoke).toHaveBeenCalledWith('edit_note', { id: 'nte_1', body: '# Research' })
		expect(mocks.invoke).not.toHaveBeenCalledWith('submit_entry', expect.anything())
	})
})

/**
 * One matrix for both multi-line fields, chosen by the `Enter key` setting
 * (user ruling 2026-08-11). Under the shipped `submit`: Enter submits, and
 * Shift+Enter and Ctrl+Enter give a newline — Shift's is the browser's own,
 * Ctrl's is inserted by hand because Chromium maps that chord to nothing.
 * Under `newline` the bare press and the Ctrl chord swap.
 */
describe('the Enter matrix', () => {
	it('submits the composer on a bare Enter and gives both modified forms a newline', async () => {
		const wrapper = await mountPanel()
		const composer = wrapper.find('#composer')
		await composer.setValue('captured')

		// Shift's newline is Chromium's `InsertNewline` — declined, not inserted —
		// which is what keeps the field's own undo stack intact.
		await composer.trigger('keydown', { key: 'Enter', shiftKey: true })
		await settle(2)
		expect(mocks.invoke).not.toHaveBeenCalledWith('submit_entry', expect.anything())

		// Ctrl has no browser mapping, so its newline lands in the field by hand.
		await composer.trigger('keydown', { key: 'Enter', ctrlKey: true })
		await settle(2)
		expect(mocks.invoke).not.toHaveBeenCalledWith('submit_entry', expect.anything())
		expect((composer.element as HTMLTextAreaElement).value).toContain('\n')

		await composer.setValue('captured')
		await composer.trigger('keydown', { key: 'Enter' })
		await settle(3)
		expect(mocks.invoke).toHaveBeenCalledWith('submit_entry', {
			body: 'captured',
			attachments: [],
		})
	})

	it('saves the inline editor on a bare Enter and leaves Shift+Enter to the field', async () => {
		const wrapper = await mountPanel()
		editor.beginEdit(SPACE, SPACE.notes[0]!)
		await wrapper.vm.$nextTick()

		const field = wrapper.find('textarea[aria-label="Edit note"]')
		await field.setValue('first line')
		await field.trigger('keydown', { key: 'Enter', shiftKey: true })
		await settle(3)
		// Still open: a newline is not a save, so the session survives the press.
		expect(mocks.invoke).not.toHaveBeenCalledWith('edit_note', expect.anything())
		expect(editor.session.value).not.toBeNull()

		await field.trigger('keydown', { key: 'Enter' })
		await settle(3)
		expect(mocks.invoke).toHaveBeenCalledWith('edit_note', { id: 'nte_1', body: 'first line' })
	})

	/**
	 * Ctrl+Enter is two things by context — `CHORDS.openInEditor` starts the
	 * `$EDITOR` handoff from a focused card — and inside the editor it may only be
	 * one of them. The press is stopped at the textarea rather than left to the
	 * shell's text-surface guard, which `Ctrl+K` has already been made an exception
	 * to once.
	 */
	it('gives Ctrl+Enter a newline in the editor without starting the external handoff', async () => {
		const wrapper = await mountPanel()
		editor.beginEdit(SPACE, SPACE.notes[0]!)
		await wrapper.vm.$nextTick()

		const field = wrapper.find('textarea[aria-label="Edit note"]')
		await field.setValue('edited body')
		await field.trigger('keydown', { key: 'Enter', ctrlKey: true })
		await settle(3)

		expect(mocks.invoke).not.toHaveBeenCalledWith('edit_note', expect.anything())
		expect((field.element as HTMLTextAreaElement).value).toContain('\n')
		expect(mocks.invoke).not.toHaveBeenCalledWith('editor_open_note', expect.anything())
		expect(editor.session.value).not.toBeNull()
	})

	/** The inverse mode: the bare press and the Ctrl chord swap, in both fields.
	 *  Shift+Enter stays a newline, so no choice loses an action. */
	it('swaps the pair under the newline setting', async () => {
		settingsPayload = { ...defaultSettings(), enterKey: 'newline' }
		const wrapper = await mountPanel()
		await settings.refresh()
		await settle(2)

		const composer = wrapper.find('#composer')
		await composer.setValue('captured')
		await composer.trigger('keydown', { key: 'Enter' })
		await settle(2)
		// Declined: the bare press is the browser's newline now.
		expect(mocks.invoke).not.toHaveBeenCalledWith('submit_entry', expect.anything())

		await composer.trigger('keydown', { key: 'Enter', ctrlKey: true })
		await settle(3)
		expect(mocks.invoke).toHaveBeenCalledWith('submit_entry', {
			body: 'captured',
			attachments: [],
		})

		editor.beginEdit(SPACE, SPACE.notes[0]!)
		await wrapper.vm.$nextTick()
		const field = wrapper.find('textarea[aria-label="Edit note"]')
		await field.setValue('edited body')
		await field.trigger('keydown', { key: 'Enter' })
		await settle(2)
		expect(mocks.invoke).not.toHaveBeenCalledWith('edit_note', expect.anything())

		await field.trigger('keydown', { key: 'Enter', ctrlKey: true })
		await settle(3)
		expect(mocks.invoke).toHaveBeenCalledWith('edit_note', { id: 'nte_1', body: 'edited body' })
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

	/** Ctrl+D is Delete's alias (user ruling 2026-08-12) — the same chord entry,
	 *  so the same shell handler and the same landing. */
	it('answers Ctrl+D exactly as Delete', async () => {
		const wrapper = await mountWithThree()
		selection.select('nte_2')
		takeRow(noteRow('nte_2'))
		await settle(2)

		await wrapper
			.get(`[data-row-id="${noteRow('nte_2')}"]`)
			.trigger('keydown', { key: 'd', ctrlKey: true })
		await settle(5)

		expect(mocks.invoke).toHaveBeenCalledWith('delete_notes', { ids: ['nte_2'] })
		expect(document.activeElement).toBe(rowElementOf(wrapper, 'nte_3'))
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
	// By its accessible name rather than by `[title]`: since the 2026-08 tooltip
	// sweep every icon-only control carries a title, so "the titled trigger" no
	// longer names one element.
	function heading(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
		return wrapper.find('[data-slot="dropdown-menu-trigger"][aria-label^="Active section"]')
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
		// Task-004's criterion 3 put the space name here; the 2026-08-08 review
		// moved it wholly into the chip, so the placeholder stays static however
		// the section or space changes.
		expect(wrapper.find('#composer').attributes('placeholder')).toBe('Add a note or a prompt…')
	})

	it('sits under the search field, and nowhere else', async () => {
		// It began as a chip above the composer and moved: the active section is what
		// the list below it is *of*, and a label above a list is where a reader looks
		// for that. Moved, not copied — two controls saying the same thing is how one
		// of them ends up stale.
		const wrapper = await mountPanel()

		expect(wrapper.find('header').element.contains(heading(wrapper).element)).toBe(true)
		expect(
			wrapper.find('form[aria-label="Add a note"] [aria-label^="Active section"]').exists(),
		).toBe(false)
		expect(wrapper.findAll('[aria-label^="Active section"]')).toHaveLength(1)
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

describe('a pinned section heading', () => {
	/**
	 * The heading of the section being read stays at the top of the region, so the
	 * answer to "which section am I in" survives a long one.
	 *
	 * It is a CSS declaration and happy-dom lays nothing out, so the classes are
	 * the only place the pin can be asserted — and they are worth asserting,
	 * because two of them are chosen against numbers that live in other files. A
	 * dropped `z-1` is a heading behind the rows it is supposed to cover; a `z`
	 * raised past `NoteList`'s indicator or `NoteCard`'s carried row hides the two
	 * things a drag needs visible.
	 *
	 * **`section-band`, and the class is the assertion.** The band paints the
	 * panel's own `--surface` as a gradient that dissolves over its last 8px, so a
	 * heading reads as part of the list it heads rather than as an opaque bar laid
	 * across it — the ruling that replaced `bg-surface-solid`, accepting a faint
	 * ghost of covered text in exchange for the colour match and the soft edge.
	 * Nothing about that is visible to a layout-free DOM, so the class is where it
	 * has to be caught.
	 */
	it('pins itself above the rows it covers and below the carried row', async () => {
		const wrapper = await mountPanel()
		const heading = wrapper.get(`[data-row-id="${sectionRow('sec_a')}"]`)

		expect(heading.classes()).toEqual(
			expect.arrayContaining(['sticky', 'top-0', 'z-1', 'section-band']),
		)
	})

	it('says done over total after the name, and nothing beside an empty section', async () => {
		// `1/2`: nte_2 is done and nte_1 is not. The empty section shows no count
		// at all — `0/0` beside every fresh heading is a mark that says nothing,
		// the same rule the section menu's delete count follows.
		const wrapper = await mountPanel()

		const research = wrapper.get(`[data-row-id="${sectionRow('sec_a')}"]`)
		expect(research.find('[data-section-counts]').text()).toBe('1/2')
		expect(research.text()).toContain('2 notes, 1 done')

		const inbox = wrapper.get(`[data-row-id="${sectionRow('sec_b')}"]`)
		expect(inbox.find('[data-section-counts]').exists()).toBe(false)
	})

	it('keeps the count current as notes are marked done', async () => {
		// Live off the document, not read once: the band is on screen while the
		// completion circle changes what it counts.
		const wrapper = await mountPanel()
		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
			if (command === 'set_notes_done') {
				const { ids, done } = args as { ids: string[]; done: boolean }
				return {
					...SPACE,
					notes: SPACE.notes.map((note) => (ids.includes(note.id) ? { ...note, done } : note)),
				}
			}
			return baseInvoke(command)
		})

		await space.setNotesDone(['nte_1'], true)
		await settle(3)

		const research = wrapper.get(`[data-row-id="${sectionRow('sec_a')}"]`)
		expect(research.find('[data-section-counts]').text()).toBe('2/2')
	})

	/**
	 * The same rule the reveal path follows, through the other entrance.
	 *
	 * A heading pinned to the top of the region is already where `block: 'nearest'`
	 * would put it, so an arrow key reaching it scrolls nothing and leaves the
	 * reader at the *end* of a section whose heading they just landed on. The
	 * section's rowgroup is the heading's un-pinned position, and landing that is
	 * what puts the section's own start on screen.
	 */
	it('lands the section when an arrow key reaches a heading that is pinned', async () => {
		const wrapper = await mountPanel()
		const heading = wrapper.get(`[data-row-id="${sectionRow('sec_a')}"]`).element
		const group = wrapper.get('[data-section-id="sec_a"]').element

		// Scrolled into the section: the heading has been pushed 120px down inside
		// its own group to stay on screen, which is the whole of the pinned test.
		group.getBoundingClientRect = (() => ({ top: -120, bottom: 200, height: 320 })) as () => DOMRect
		heading.getBoundingClientRect = (() => ({ top: 0, bottom: 24, height: 24 })) as () => DOMRect

		const seen: (ScrollIntoViewOptions | undefined)[] = []
		group.scrollIntoView = (options?: boolean | ScrollIntoViewOptions) => {
			seen.push(options as ScrollIntoViewOptions | undefined)
		}
		heading.scrollIntoView = () => {
			seen.push(undefined)
		}

		selection.select('nte_1')
		await wrapper.find(`[data-row-id="${noteRow('nte_1')}"]`).trigger('keydown', { key: 'ArrowUp' })
		await settle(3)

		expect(selection.focusedId.value).toBe(sectionRow('sec_a'))
		expect(seen).toEqual([{ block: 'start' }])
	})
})

describe('the section switcher', () => {
	function content() {
		return document.querySelector<HTMLElement>('[data-slot="dropdown-menu-content"]')
	}

	// The chip's accessible name, not `[title]` — every icon-only control has
	// carried a title since the 2026-08 tooltip sweep.
	function chip(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
		return wrapper.find('[data-slot="dropdown-menu-trigger"][aria-label^="Active section"]')
	}

	/**
	 * Opened through the state the chip's `open` is bound to rather than by a
	 * chord: task-019 gave `Ctrl+K` to the command palette, and the switcher's two
	 * surviving entry points are both pointer-driven.
	 *
	 * The composer is focused first because one case below is about what happens
	 * when it held the caret — and driving the controlled state is what keeps that
	 * case reachable. A real click on the chip moves focus to the trigger *before*
	 * the switcher opens, which is the other half of the contract and the one
	 * reka's own close-focus already handles.
	 */
	async function showSwitcher(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
		;(wrapper.find('#composer').element as HTMLTextAreaElement).focus()
		sections.openSwitcher('chip')
		await settle(3)
	}

	it('opens from the chip under the search field', async () => {
		const wrapper = await mountPanel()
		await chip(wrapper).trigger('pointerdown', { button: 0 })
		await chip(wrapper).trigger('click')
		await settle(3)

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

	it('shows each section with its done and total counts', async () => {
		// The counts are what make one destination worth picking over another, and
		// they are the document's rather than the filtered list's — a query narrows
		// what is on screen, not what a section holds.
		const wrapper = await mountPanel()
		await showSwitcher(wrapper)

		const rows = [...(content()?.querySelectorAll('[role="menuitem"]') ?? [])]
		expect(rows[0]?.textContent).toContain('Research')
		expect(rows[0]?.textContent).toContain('1/2')
		expect(rows[0]?.textContent).toContain('2 notes, 1 done')
		// An empty section still says so rather than showing nothing.
		expect(rows[1]?.textContent).toContain('Inbox')
		expect(rows[1]?.textContent).toContain('0/0')
	})

	it('offers one field that both filters and creates', async () => {
		// Not two inputs. A dedicated "new section" field beside the filter would
		// fork the keyboard path — two places for Enter to mean something — and
		// duplicate a creation route that already exists.
		const wrapper = await mountPanel()
		await showSwitcher(wrapper)

		expect(content()?.querySelectorAll('input')).toHaveLength(1)
		const filter = content()!.querySelector<HTMLInputElement>('#section-filter')!
		expect(filter.placeholder).toBe('Filter or create a section…')
	})

	/** The field and the rows are the popover's only two stops, and reka
	 *  connects them one way: arrows walk focus into its collection, and the
	 *  field — not being a menu item — is never walked back to, which left Tab
	 *  dead and the rows a one-way door. Tab cycles between the two stops, and
	 *  ArrowUp on the first row returns to the field instead of wrapping. */
	it('cycles focus between the filter field and the rows', async () => {
		const wrapper = await mountPanel()
		await showSwitcher(wrapper)

		const filter = content()!.querySelector<HTMLInputElement>('#section-filter')!
		const first = content()!.querySelector<HTMLElement>('[role="menuitem"]')!
		filter.focus()

		const press = (target: HTMLElement, key: string) =>
			target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }))

		press(filter, 'Tab')
		expect(document.activeElement).toBe(first)

		press(first, 'Tab')
		expect(document.activeElement).toBe(filter)

		press(filter, 'Tab')
		press(first, 'ArrowUp')
		expect(document.activeElement).toBe(filter)
	})

	/**
	 * `Ctrl+K` was this surface's for three tasks and is the command palette's
	 * now. The switcher kept both of its pointer routes, because the palette
	 * absorbs *switching* and not *creating* — but the chord opens the palette
	 * from every surface, including the composer, which is the one place the
	 * switcher's narrower exception used to let it through.
	 */
	it('no longer answers the chord, which belongs to the palette', async () => {
		const wrapper = await mountPanel()

		const composer = wrapper.find('#composer')
		;(composer.element as HTMLTextAreaElement).focus()
		await composer.trigger('keydown', { key: 'k', ctrlKey: true })
		await settle(3)

		expect(sections.switcherOpen.value).toBe(false)
		expect(palette.isOpen.value).toBe(true)
	})

	it('activates a section and gives the composer back its text and its caret', async () => {
		const wrapper = await mountPanel()
		const composer = wrapper.find('#composer').element as HTMLTextAreaElement
		await wrapper.find('#composer').setValue('half a thought')
		composer.focus()
		composer.setSelectionRange(4, 4)

		await showSwitcher(wrapper)
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
		await showSwitcher(wrapper)

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
		await showSwitcher(wrapper)

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
		await showSwitcher(wrapper)

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
		await showSwitcher(wrapper)

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
		await showSwitcher(wrapper)
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
		await showSwitcher(wrapper)
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
		// The same invariant the search filter has to hold: the roving target is
		// where the arrow keys resume, so it must always name a rendered row.
		const wrapper = await mountPanel()
		await disclosure(wrapper, 'Research').trigger('click')
		await settle(3)

		const rendered = wrapper.findAll('[data-row-id]').map((row) => row.attributes('data-row-id'))
		expect(selection.rowIds.value).toEqual(rendered)
		expect(selection.visibleNoteIds.value).toEqual([])
		expect(rendered).toContain(selection.focusedId.value)
		// Every rendered row is a Tab stop.
		expect(wrapper.findAll('[data-row-id][tabindex="0"]')).toHaveLength(
			wrapper.findAll('[data-row-id]').length,
		)
	})

	it('folds and unfolds on Space, and leaves activation to Enter', async () => {
		// The 2026-08-10 ruling: Space on a heading toggles the disclosure; making
		// the section active moved to Enter alone.
		const wrapper = await mountPanel()
		takeRow(sectionRow('sec_a'))
		await settle(2)

		const grid = wrapper.get('[role="grid"]')
		await grid.trigger('keydown', { key: ' ' })
		await settle(3)
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(0)
		expect(mocks.invoke).not.toHaveBeenCalledWith('set_active_section', expect.anything())

		await grid.trigger('keydown', { key: ' ' })
		await settle(3)
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(2)

		await grid.trigger('keydown', { key: 'Enter' })
		await settle(3)
		expect(mocks.invoke).toHaveBeenCalledWith('set_active_section', { id: 'sec_a' })
		// Enter did not also fold the section.
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(2)
	})

	it('unfolds a folded section when Enter makes it active', async () => {
		// The user ruling: choosing a section as the capture target implies
		// wanting to see it, so activation carries the unfold — on Enter and the
		// name click, never on Space, which stays a pure fold toggle.
		const wrapper = await mountPanel()
		takeRow(sectionRow('sec_a'))
		await settle(2)

		const grid = wrapper.get('[role="grid"]')
		await grid.trigger('keydown', { key: ' ' })
		await settle(3)
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(0)

		await grid.trigger('keydown', { key: 'Enter' })
		await settle(3)
		expect(mocks.invoke).toHaveBeenCalledWith('set_active_section', { id: 'sec_a' })
		expect(wrapper.findAll('[data-row-id^="n:"]')).toHaveLength(2)
	})

	it('hands focus back to the row when a click lands it on a row control', async () => {
		// Clicking the completion circle focuses the button, and the next keypress
		// re-grants `:focus-visible` — a keyboard ring on a control the user never
		// keyboard-navigated to. Outside F2 interaction mode the grid's focusin
		// handler returns focus to the row itself.
		const wrapper = await mountPanel()
		const circle = wrapper.get(`[data-row-id="${noteRow('nte_1')}"] [data-slot="checkbox"]`)
		;(circle.element as HTMLElement).focus()
		await circle.trigger('focusin')
		await settle(2)

		expect(selection.focusedId.value).toBe(noteRow('nte_1'))
		expect(document.activeElement).toBe(wrapper.get(`[data-row-id="${noteRow('nte_1')}"]`).element)
	})

	it('keeps the roving target in step with focus the grid did not move itself', async () => {
		// Tab and band clicks put DOM focus on a row without going through the
		// arrow handlers; the grid's focusin listener records the arrival so the
		// next arrow moves relative to the row the user actually sees focused.
		const wrapper = await mountPanel()
		takeRow(noteRow('nte_1'))
		await settle(2)

		// `trigger` dispatches the bubbling focusin the browser would send; the
		// grid's listener reads the row off the event target.
		await wrapper.get('[data-row-id="s:sec_b"]').trigger('focusin')
		await settle(2)

		expect(selection.focusedId.value).toBe('s:sec_b')
	})

	it('clears the selection when another note’s completion circle is pressed', async () => {
		// The hole the bubble-phase rule had: the circle’s `@click.stop` kept the
		// press from ever reaching the region handler, so acting on another note
		// left the old selection standing under its ring. The rule runs on
		// capture now, which sees the press before any control can swallow it.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await settle(2)

		await wrapper
			.get('[data-row-id="n:nte_2"] button[aria-label="Mark as not done"]')
			.trigger('click')
		await settle(3)

		expect(selection.selectedIds.value).toEqual([])
	})

	it('keeps the selection when the pressed circle belongs to a selected note', async () => {
		// A click anywhere on a selected note’s row is not “away” — the circle
		// deliberately ignores the selection for the toggle, and taking the
		// selection with it would make acting on your own selection destroy it.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		selection.extendTo('nte_2')
		await settle(2)

		await wrapper
			.get('[data-row-id="n:nte_2"] button[aria-label="Mark as not done"]')
			.trigger('click')
		await settle(3)

		expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])
	})

	it('leaves a modifier click to extend the selection rather than clearing it first', async () => {
		// Ctrl adds and Shift ranges; both are selection gestures, never “away”.
		// Running the capture-phase clear under them would empty the selection a
		// tick before the row’s own handler adds one note back to it.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await settle(2)

		await wrapper.get('[data-row-id="n:nte_2"]').trigger('click', { ctrlKey: true })
		await settle(2)

		expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])
	})

	it('clears the selection on a click that is not on a note', async () => {
		// The click-away rule: no note keeps its ring after the user clicks
		// somewhere else in the list — a section band and bare region both count.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await settle(2)

		await wrapper.get('[data-scroll-region]').trigger('click')
		await settle(2)

		expect(selection.selectedIds.value).toEqual([])
	})

	it('never destroys a selection it merely folded away', async () => {
		// By keyboard, deliberately: the pointer path — clicking the chevron — now
		// also clears the selection first, because any click in the list that is
		// not on a note is a click-away (the 2026-08-10 ruling on `PanelShell`'s
		// region handler). The fold-versus-selection invariant lives on.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		selection.extendTo('nte_2')
		// Ctrl+Arrow's traversal: up to the header without touching the selection.
		selection.moveFocusOnly(-2)
		expect(selection.focusedId.value).toBe('s:sec_a')

		await wrapper.get('[role="grid"]').trigger('keydown', { key: 'ArrowLeft' })
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
		// say so. Folded by keyboard, since the chevron's click is a click-away that
		// clears the selection first.
		const wrapper = await mountPanel()
		selection.select('nte_1')
		selection.extendTo('nte_2')
		selection.moveFocusOnly(-2)

		await wrapper.get('[role="grid"]').trigger('keydown', { key: 'ArrowLeft' })
		await settle(3)

		// The roving target sits on the header, so `focusedNoteId` is null — which
		// must not defeat a multi-select either.
		expect(selection.focusedNoteId.value).toBeNull()

		await wrapper.trigger('keydown', { key: 'c', ctrlKey: true })
		await settle(4)

		expect(lastRender()?.selection).toEqual({ kind: 'ids', ids: ['nte_1', 'nte_2'] })
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
		expect(wrapper.findAll('[data-row-id][tabindex="0"]')).toHaveLength(
			wrapper.findAll('[data-row-id]').length,
		)
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
			return baseInvoke(command, args)
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
		expect(cards[1]?.getAttribute('aria-disabled')).toBeNull()
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
		// `aria-disabled`, not `disabled`: the card must stay reachable so the
		// keyboard can hear the cause and the context menu can reach the folder.
		expect(card?.getAttribute('aria-disabled')).toBe('true')
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
		// The OS route is still there — it moved to the card's context menu — but a
		// double-click must not take both.
		expect(mocks.invoke).not.toHaveBeenCalledWith('attachment_open', expect.anything())
		expect(viewer()?.querySelector('img')).not.toBeNull()
	})

	it('activates identically on Space and Enter, as a button must', async () => {
		withImagePreview()
		await mountPanel()
		await installWithAttachments(documentWith([PNG]))

		// Task-011's OS route lives on the card's context menu now — Space keeping a
		// second meaning broke the one promise every button makes.
		const card = document.querySelector<HTMLElement>('button[aria-label^="View"]')
		card?.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }))
		await settle(2)

		expect(mocks.invoke).toHaveBeenCalledWith('attachment_full', { file: PNG.file })
		expect(mocks.invoke).not.toHaveBeenCalledWith('attachment_open', expect.anything())
		expect(viewer()).not.toBeNull()
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
		// Focus goes back toward where the press came from, not to the body — and
		// the grid's focusin rule then seats it on the card's row: outside F2
		// interaction mode, focus never rests on a control inside a row. (A viewer
		// opened *from* F2 keeps the card focused — interaction mode is the
		// exemption.)
		expect(document.activeElement).toBe(card.closest('[data-row-id]'))
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

	/**
	 * AC20. `Copy` and `Copy as list` are about bodies; a local file path means
	 * nothing to whatever the text is pasted into.
	 *
	 * Asserted against the *request* since task-024: a copy names notes, and an
	 * attachment is no part of naming one, so the same note produces the same
	 * request whether or not it carries files. That the renderer then leaves them
	 * out is `copper_core::markdown`'s own recorded rule.
	 */
	it('asks for the same notes whether or not they carry attachments', async () => {
		withAttachmentCommands()
		await mountPanel()

		await installWithAttachments(SPACE)
		selection.select('nte_1')
		await actions.copyNotes()
		const withoutFiles = renderCalls()

		await installWithAttachments(documentWith([PNG, PDF]))
		selection.select('nte_1')
		await actions.copyNotes()
		const all = renderCalls()

		expect(all).toHaveLength(withoutFiles.length + 1)
		expect(all.at(-1)).toEqual(withoutFiles.at(-1))
		expect(JSON.stringify(all.at(-1))).not.toContain(PNG.file)
		// And the request really did reach the clipboard, so this is a copy rather
		// than two identical no-ops.
		expect(copied()).toBe(JSON.stringify(lastRender()))
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
		expect(actions.attachmentActionLabel.value).toBe('Open attachment location')

		await installWithAttachments(documentWith([PNG]))
		selection.select('nte_1')
		takeRow(noteRow('nte_1'))
		await settle(1)
		expect(actions.attachmentActionLabel.value).toBe('Open attachment')
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
	 *
	 * It answers `reorder_notes`, which is the one command both move paths send
	 * now: a single note is a block of one, so the drag and Alt+Arrow tests all
	 * assert the same shape. The block lands whole at the after-removal index,
	 * which is the op's own arithmetic in miniature.
	 */
	function installReorderingStore(base: Space = SPACE) {
		let current: Space = { ...base, notes: base.notes.map((note) => ({ ...note })) }
		const calls: { ids: string[]; section: string; index: number }[] = []

		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
			if (command === 'get_active_space') return current
			if (command !== 'reorder_notes') return baseInvoke(command)

			const { ids, section, index } = args as { ids: string[]; section: string; index: number }
			calls.push({ ids, section, index })

			const carried = new Set(ids)
			const moved = current.notes.filter((note) => carried.has(note.id))
			const rest = current.notes.filter((note) => !carried.has(note.id))
			const target = rest.filter((note) => note.section === section)
			target.splice(index, 0, ...moved.map((note) => ({ ...note, section })))
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
			expect(store.calls.at(-1)).toEqual({ ids: ['nte_1'], section: 'sec_a', index: 1 })
			expect(store.order('sec_a')).toEqual(['nte_2', 'nte_1'])
		})

		it('carries the whole selection as one block when the focused note is in it', async () => {
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			selection.select('nte_1')
			selection.extendTo('nte_2')

			await wrapper
				.find('[data-row-id="n:nte_2"]')
				.trigger('keydown', { key: 'ArrowDown', altKey: true })
			await settle(4)

			// Both of sec_a's notes are selected, so there is nothing left to hop
			// inside it: the block crosses into the next section — together, in
			// document order, as one call and so one undo step.
			expect(store.calls).toEqual([{ ids: ['nte_1', 'nte_2'], section: 'sec_b', index: 0 }])
			expect(store.order('sec_b')).toEqual(['nte_1', 'nte_2'])
			// And the selection survives the move: the block is still what the user
			// picked, so nudging it is not a reason to collapse it.
			expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])
		})

		it('hops a selected block past the next remaining note in one call', async () => {
			const third: Space = {
				...SPACE,
				notes: [
					...SPACE.notes,
					{
						id: 'nte_3',
						section: 'sec_a',
						order: 2,
						done: false,
						body: 'third note',
						created: '2026-08-05T00:00:00Z',
						updated: '2026-08-05T00:00:00Z',
					},
				],
			}
			const wrapper = await mountPanel()
			const store = installReorderingStore(third)
			await space.refresh()
			await settle(3)

			selection.select('nte_1')
			selection.extendTo('nte_2')
			await wrapper
				.find('[data-row-id="n:nte_2"]')
				.trigger('keydown', { key: 'ArrowDown', altKey: true })
			await settle(4)

			// One step is one hop over the nearest note the block leaves behind, and
			// the index is counted with *both* carried notes removed: `[1, 2]` past
			// `3` is index 1 of `[3]`, never index 2 of the rendered rows.
			expect(store.calls).toEqual([{ ids: ['nte_1', 'nte_2'], section: 'sec_a', index: 1 }])
			expect(store.order('sec_a')).toEqual(['nte_3', 'nte_1', 'nte_2'])
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

			expect(store.calls).toEqual([{ ids: ['nte_1'], section: 'sec_a', index: 1 }])
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

			expect(store.calls).toEqual([{ ids: ['nte_1'], section: 'sec_b', index: 0 }])
			expect(store.order('sec_b')).toEqual(['nte_1'])
		})

		it('drops the whole selection together when the dragged note is part of it', async () => {
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			selection.select('nte_1')
			selection.extendTo('nte_2')
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 150))
			await settle()

			// The rest of the carried selection says so for the length of the
			// gesture, so the drop is not a surprise about what travels.
			expect(document.querySelector('[data-row-id="n:nte_2"][data-carried]')).not.toBeNull()

			window.dispatchEvent(pointer('pointerup', 150))
			await settle(4)

			// One call and so one undo step, the block in document order.
			expect(store.calls).toEqual([{ ids: ['nte_1', 'nte_2'], section: 'sec_b', index: 0 }])
			expect(store.order('sec_b')).toEqual(['nte_1', 'nte_2'])
			// The block stays selected — nothing collapses to the grabbed note.
			expect(selection.selectedIds.value).toEqual(['nte_1', 'nte_2'])
			expect(document.querySelector('[data-carried]')).toBeNull()
		})

		it('leaves the selection behind when the dragged note is not part of it', async () => {
			// The block rule is anchored on the dragged note, exactly as every other
			// action anchors on focus: a selection elsewhere is not what this
			// gesture names.
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			selection.select('nte_2')
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 150))
			await settle()
			expect(document.querySelector('[data-carried]')).toBeNull()

			window.dispatchEvent(pointer('pointerup', 150))
			await settle(4)

			expect(store.calls).toEqual([{ ids: ['nte_1'], section: 'sec_b', index: 0 }])
		})

		it('recognises a drop that reassembles the block where it already is as a no-op', async () => {
			// Both of sec_a's notes are selected and the first is dropped below the
			// second. The geometry answers index 1 — counted with only the *dragged*
			// note excluded — but with the whole block removed the destination is
			// index 0, which is where the block already sits. Nothing may reach the
			// store, and no undo entry may be pushed.
			const wrapper = await mountPanel()
			const store = installReorderingStore()
			selection.select('nte_1')
			selection.extendTo('nte_2')
			const grip = gripOf(wrapper, 'n:nte_1')

			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			window.dispatchEvent(pointer('pointerup', 90))
			await settle(4)

			expect(store.calls).toEqual([])
		})

		it("places an empty section's line from its group, not from a pinned heading", async () => {
			// The one measurement `position: sticky` can falsify. An empty section has
			// no row to sit beside, so the line goes just under its heading — and a
			// heading pinned to the top of the region is painted nowhere near the
			// section it belongs to. Read off its rect, the line for a drop into
			// `sec_b` would be drawn at the top of the list, across a section the note
			// is not going to.
			const wrapper = await mountPanel()
			installReorderingStore()
			const heading = wrapper.get('[data-section-id="sec_b"] [data-section-row]').element
			heading.getBoundingClientRect = (() => ({ top: 0, bottom: 16, height: 16 })) as () => DOMRect

			const grip = gripOf(wrapper, 'n:nte_1')
			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 150))
			await settle()

			// The group's own top plus the heading's height — 124 and 16 — plus the
			// 2px that keeps the line off the row edge.
			expect(drag.dropTarget.value).toMatchObject({ sectionId: 'sec_b', indicatorY: 142 })

			window.dispatchEvent(pointer('pointerup', 150))
			await settle(4)
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
			// `isDragging` stays true so the list animation never comes back, and
			// the auto-scroll loop keeps asking for frames.
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
			// The abandoned row is mid-settle — animating home rather than teleporting
			// — so its styles clear on `transitionend`, synthesised here because
			// happy-dom has no TransitionEvent and fires none of its own.
			const settling = document.querySelector<HTMLElement>('[data-note-row][data-settling]')
			const done = new Event('transitionend') as TransitionEvent
			Object.defineProperty(done, 'propertyName', { value: 'transform' })
			settling?.dispatchEvent(done)
			await settle(1)
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
			expect(store.calls).toEqual([{ ids: ['nte_1'], section: 'sec_b', index: 0 }])
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
			// The list is mid-motion for 150ms after any change, and an animated
			// row reports its *transformed* box — so a drag begun just after a capture
			// landed would measure rows at positions they are still travelling away
			// from. The gate cannot be relied on for this: it only stops animations
			// that have not started when the drag arms.
			const wrapper = await mountPanel()
			installReorderingStore()

			const finish = vi.fn()
			const proto = Element.prototype as unknown as Record<string, unknown>
			proto.getAnimations = () => [{ playState: 'running', finish, timeline: document.timeline }]
			restore.push(() => Reflect.deleteProperty(proto, 'getAnimations'))

			const grip = gripOf(wrapper, 'n:nte_1')
			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			await settle()

			expect(finish).toHaveBeenCalled()

			window.dispatchEvent(pointer('pointerup', 90))
			await settle(3)
		})

		it('leaves the section band’s scroll-driven row clip alone', async () => {
			// The band erases each row as it slides under the pinned heading, with a
			// `clip-path` keyframe on the row's own `view()` timeline. That animation is
			// geometry, not motion: it is permanently `running` and it never ends, so
			// the settle above swept it up and `finish()` parked every row at the
			// keyframe's end — `inset(100% 0 0 0)`, which is the row clipped away
			// entirely — with no scroll position that brings it back. One drag emptied
			// the list.
			const wrapper = await mountPanel()
			installReorderingStore()

			const finishFlip = vi.fn()
			const finishClip = vi.fn()
			const proto = Element.prototype as unknown as Record<string, unknown>
			proto.getAnimations = () => [
				{ playState: 'running', finish: finishFlip, timeline: document.timeline },
				// Any timeline that is not the document's is progress-based.
				{ playState: 'running', finish: finishClip, timeline: { currentTime: 0 } },
			]
			restore.push(() => Reflect.deleteProperty(proto, 'getAnimations'))

			const grip = gripOf(wrapper, 'n:nte_1')
			grip.dispatchEvent(pointer('pointerdown', 40))
			window.dispatchEvent(pointer('pointermove', 90))
			await settle()

			expect(finishFlip).toHaveBeenCalled()
			expect(finishClip).not.toHaveBeenCalled()

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

			expect(store.calls).toEqual([{ ids: ['nte_1'], section: 'sec_b', index: 0 }])
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

	/**
	 * Reordering works under the done filter (user ruling 2026-08-12): the filter
	 * hides notes but never reorders them, so a move anchors to its nearest
	 * *visible* neighbour and lands directly beside it in document order — the
	 * hidden notes in between stay exactly where they are.
	 */
	describe('under the done filter', () => {
		/** Four notes in one section, the two done ones placed so a bare count of
		 *  visible rows and the anchor rule disagree about where a move lands. In
		 *  the todo view only `nte_2` and `nte_4` are on screen. */
		const FILTERED: Space = {
			...SPACE,
			notes: ['nte_1', 'nte_2', 'nte_3', 'nte_4'].map((id, order) => ({
				id,
				section: 'sec_a',
				order,
				done: id === 'nte_1' || id === 'nte_3',
				body: id,
				created: '2026-08-05T00:00:00Z',
				updated: '2026-08-05T00:00:00Z',
			})),
		}

		async function mountFiltered() {
			const wrapper = await mountPanel()
			const store = installReorderingStore(FILTERED)
			await space.refresh()
			list.setDoneFilter('todo')
			await settle(3)
			return { wrapper, store }
		}

		it('keeps the drag handle on the narrowed view', async () => {
			const { wrapper } = await mountFiltered()
			expect(wrapper.find('[data-drag-handle]').exists()).toBe(true)
		})

		it('drops a note directly beside its visible neighbour, hidden notes staying put', async () => {
			const { store } = await mountFiltered()

			// The drop slot above `nte_2` — visible index 0. A bare count of 0
			// would land `nte_4` above the hidden `nte_1` too; the anchor rule
			// lands it directly before the row it was dropped against.
			await actions.finishDrag('nte_4', 'sec_a', 0)
			await settle(3)

			expect(store.calls).toEqual([{ ids: ['nte_4'], section: 'sec_a', index: 1 }])
			expect(store.order('sec_a')).toEqual(['nte_1', 'nte_4', 'nte_2', 'nte_3'])
		})

		it('hops Alt+Arrow past the visible neighbour, never past a hidden note', async () => {
			const { wrapper, store } = await mountFiltered()

			takeRow(noteRow('nte_2'))
			await settle(2)
			await wrapper.get('[role="grid"]').trigger('keydown', { key: 'ArrowDown', altKey: true })
			await settle(4)

			// One press hops past `nte_4`, the next *visible* note, landing
			// directly after it. Hopping the hidden `nte_3` — the old
			// document-count arithmetic — would have changed nothing on screen.
			expect(store.calls).toEqual([{ ids: ['nte_2'], section: 'sec_a', index: 3 }])
			expect(store.order('sec_a')).toEqual(['nte_1', 'nte_3', 'nte_4', 'nte_2'])
			expect(wrapper.text()).not.toContain('Show all notes to reorder them.')
		})

		it('treats a drop back into the same visible slot as a no-op', async () => {
			const { store } = await mountFiltered()

			// `nte_4`'s own slot: below `nte_2`, visible index 1. The document
			// offers an invisible move — across the hidden `nte_3` — but the
			// gesture never expressed one, so nothing may reach the store.
			await actions.finishDrag('nte_4', 'sec_a', 1)
			await settle(3)

			expect(store.calls).toEqual([])
		})

		it('crosses into the next section when the view shows nothing left to hop', async () => {
			const { wrapper, store } = await mountFiltered()

			takeRow(noteRow('nte_4'))
			await settle(2)
			await wrapper.get('[role="grid"]').trigger('keydown', { key: 'ArrowDown', altKey: true })
			await settle(4)

			// Below `nte_4` the todo view shows nothing left to hop, so the block
			// crosses into `sec_b`, entering at the top — exactly as it would with
			// no filter on.
			expect(store.calls).toEqual([{ ids: ['nte_4'], section: 'sec_b', index: 0 }])
			expect(store.order('sec_b')).toEqual(['nte_4'])
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
 * The list-paste question: a zero-focus paste whose clipboard is a flat
 * Markdown-style list is the one shape with two right answers, so the shell
 * asks — one note, or one note per item — and adds nothing until an offer is
 * pressed. Everything else about the paste path above is unchanged, including
 * the composer's own paste, which the text-surface guard never lets get here.
 */
describe('the list-paste question', () => {
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

	function question() {
		return document.querySelector<HTMLElement>('[data-list-paste]')
	}

	const LIST = '- Empty dishwasher\n- Take out trash\n- Defrost fridge'

	it('asks instead of adding, count on the split offer, focus on the safe one', async () => {
		await mountPanel()

		document.body.dispatchEvent(paste(LIST))
		await settle(3)

		const popover = question()
		expect(popover).not.toBeNull()
		expect(popover!.textContent).toContain('One note')
		expect(popover!.textContent).toContain('Separate notes')
		expect(popover!.textContent).toContain('3')
		// The first press adds nothing — that is the point of asking.
		expect(calls('add_note')).toHaveLength(0)
		expect(calls('add_notes')).toHaveLength(0)
		// The offer that means "what a paste always did" takes the autofocus, so
		// Enter answers the question without a pointer.
		expect(document.activeElement).toBe(popover!.querySelector('[data-paste-one-note]'))
	})

	it('pastes the clipboard verbatim as one note on the first offer', async () => {
		await mountPanel()
		document.body.dispatchEvent(paste(LIST))
		await settle(3)

		question()!.querySelector<HTMLElement>('[data-paste-one-note]')!.click()
		await settle(3)

		expect(question()).toBeNull()
		expect(calls('add_note')[0]?.[1]).toEqual({ body: LIST, section: null })
		expect(calls('add_notes')).toHaveLength(0)
	})

	it('splits into one note per item, markers stripped, on the second offer', async () => {
		await mountPanel()
		document.body.dispatchEvent(paste(LIST))
		await settle(3)

		question()!.querySelector<HTMLElement>('[data-paste-separate-notes]')!.click()
		await settle(3)

		expect(question()).toBeNull()
		// One command for the whole batch, so the split is one undo step.
		expect(calls('add_notes')).toHaveLength(1)
		expect(calls('add_notes')[0]?.[1]).toEqual({
			bodies: ['Empty dishwasher', 'Take out trash', 'Defrost fridge'],
			section: null,
		})
		expect(calls('add_note')).toHaveLength(0)
	})

	it('moves between the offers on the arrow keys', async () => {
		await mountPanel()
		document.body.dispatchEvent(paste(LIST))
		await settle(3)

		const popover = question()!
		const arrow = (key: string) =>
			document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }))

		// The offers stack vertically, so the vertical pair is the natural one —
		// but the helper takes both axes, so either works.
		arrow('ArrowDown')
		expect(document.activeElement).toBe(popover.querySelector('[data-paste-separate-notes]'))
		arrow('ArrowUp')
		expect(document.activeElement).toBe(popover.querySelector('[data-paste-one-note]'))
	})

	it('adds nothing when dismissed — the clipboard still holds the text', async () => {
		await mountPanel()
		document.body.dispatchEvent(paste(LIST))
		await settle(3)

		question()!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
		await settle(3)

		expect(question()).toBeNull()
		expect(calls('add_note')).toHaveLength(0)
		expect(calls('add_notes')).toHaveLength(0)
		// The press resolved at the overlay guard, not on the Escape ladder: the
		// question closing must not also hide the panel.
		expect(mocks.invoke).not.toHaveBeenCalledWith('hide_panel')
	})

	it('does not ask about text with structure beyond a flat list', async () => {
		await mountPanel()

		const structured = '# Chores\n- one\n- two'
		document.body.dispatchEvent(paste(structured))
		await settle(3)

		// A heading, nesting or prose means the split would destroy structure, so
		// the paste stays exactly what it always was: one captured note.
		expect(question()).toBeNull()
		expect(calls('add_note')[0]?.[1]).toEqual({ body: structured, section: null })
		expect(calls('add_notes')).toHaveLength(0)
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
		await settle(4)

		expect(renderCalls()).toEqual([
			{ selection: { kind: 'ids', ids: ['nte_1'] }, format: 'bodies' },
		])
		expect(copied()).toBe(JSON.stringify(lastRender()))
		expect(editor.editingNoteId.value).toBeNull()
	})

	it('opens the inline editor when the setting says edit', async () => {
		const wrapper = await mountPanel()
		await useEdit()
		const body = bodyOf(wrapper, 'n:nte_1')

		await body.trigger('click')
		await body.trigger('dblclick')
		await settle(4)

		expect(editor.editingNoteId.value).toBe('nte_1')
		expect(renderCalls()).toHaveLength(0)
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
 * *scope resolution*: which notes each affordance hands over, as the
 * `NoteSelection` it sends.
 *
 * The formatting is no longer asserted here at all. Task-024 moved it into
 * `copper_core::markdown`, where `copper-core/tests/markdown.rs` holds the whole
 * of the deleted `noteMarkdown.test.ts` corpus, and the section-and-note rules
 * each scope implies — an empty heading kept here and dropped there — are
 * asserted against the resolver in `src-tauri/tests/markdown.rs`. What is left
 * on this side is genuinely the panel's: naming the scope, and declining to
 * replace the clipboard with an empty result.
 */
describe('copy as Markdown', () => {
	it('asks for the whole document, empty sections included', async () => {
		await mountPanel()

		await actions.copyDocumentAsMarkdown()
		await settle(2)

		// `Inbox` holds nothing and is still in scope: the panel says "the document"
		// and the resolver keeps the empty section's heading, which is where that
		// rule is now tested.
		expect(lastRender()).toEqual({ selection: { kind: 'document' }, format: 'markdown' })
		expect(copied()).toBe(JSON.stringify(lastRender()))
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

		// The whole document, not the one matching note — and nothing about the
		// query travels with the request.
		expect(lastRender()).toEqual({ selection: { kind: 'document' }, format: 'markdown' })
	})

	it('copies one section, whole, from the section menu', async () => {
		await mountPanel()

		await actions.copySectionAsMarkdown('sec_a')
		await settle(2)

		expect(lastRender()).toEqual({
			selection: { kind: 'section', id: 'sec_a' },
			format: 'markdown',
		})
		expect(copied()).toBe(JSON.stringify(lastRender()))
	})

	it('writes nothing for a section holding no notes', async () => {
		await mountPanel()

		await actions.copySectionAsMarkdown('sec_b')
		await settle(2)

		// The scope is asked for, and the answer's count of zero is where the panel
		// stops. A heading on its own is not worth replacing the clipboard with —
		// the same rule every other empty copy follows, and the one part of this
		// that is still a frontend decision.
		expect(lastRender()?.selection).toEqual({ kind: 'section', id: 'sec_b' })
		expect(copied()).toBeNull()
	})

	/** AC8/AC9. A selection copy carries the notes an action targets; grouping
	 *  them under their own sections, and dropping the sections that contribute
	 *  nothing, is the resolver's half. */
	it('sends the targeted notes when the selection is every note', async () => {
		await mountPanel()
		selection.selectAll()

		await actions.copySelectionAsMarkdown()
		await settle(2)

		expect(lastRender()).toEqual({
			selection: { kind: 'ids', ids: ['nte_1', 'nte_2'] },
			format: 'markdown',
		})
	})

	it('sends a single selected note', async () => {
		await mountPanel()
		selection.select('nte_1')

		await actions.copySelectionAsMarkdown()
		await settle(2)

		expect(lastRender()?.selection).toEqual({ kind: 'ids', ids: ['nte_1'] })
		expect(copied()).toBe(JSON.stringify(lastRender()))
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

		expect(lastRender()?.selection).toEqual({ kind: 'ids', ids: ['nte_1', 'nte_2'] })
	})

	it('writes nothing at all when there is nothing selected', async () => {
		await mountPanel()
		selection.clear()
		selection.focusRow(null)

		await actions.copySelectionAsMarkdown()
		await settle(2)

		// An empty scope is still a well-formed question, and the answer's count of
		// zero is what keeps the clipboard alone.
		expect(lastRender()?.selection).toEqual({ kind: 'ids', ids: [] })
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
		// pointing the roving target at one would leave the arrows with nowhere to
		// resume from.
		expect(wrapper.find('[data-row-id="n:nte_1"]').exists()).toBe(false)
		expect(selection.focusedId.value).toBe('s:sec_a')
		expect(wrapper.findAll('[data-row-id][tabindex="0"]').length).toBe(
			wrapper.findAll('[data-row-id]').length,
		)
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
		expect(wrapper.findAll('[data-row-id][tabindex="0"]').length).toBe(
			wrapper.findAll('[data-row-id]').length,
		)
	})
})

/**
 * AC12 at the panel, not at the renderer.
 *
 * The byte-identical claim itself belongs to `copper-core/tests/markdown.rs`
 * since task-024: there is one renderer, so the same notes cannot come out as
 * two texts, and that is asserted where the renderer is. What that claim rests
 * on is the half these keep — that the three scopes really do resolve to the
 * same notes when they should, which is still decided here.
 */
describe('the three copy scopes resolve to the same notes', () => {
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
		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) =>
			command === 'get_active_space' ? filled : baseInvoke(command, args),
		)
		await space.refresh()
	}

	it('names the same notes from the menu action and from select-all', async () => {
		await mountPanel()
		await installFilledDocument()

		await actions.copyDocumentAsMarkdown()
		await settle(2)
		const fromMenu = lastRender()

		selection.selectAll()
		await actions.copySelectionAsMarkdown()
		await settle(2)
		const fromSelection = lastRender()

		// One rendering for both, so the texts cannot differ once the notes agree.
		expect(fromSelection?.format).toBe('markdown')
		expect(fromMenu?.format).toBe('markdown')

		expect(fromMenu?.selection).toEqual({ kind: 'document' })
		// The select-all scope is the document scope restricted to its notes: every
		// one of them, in document order, whatever order they were selected in. With
		// no empty section left the two cover exactly the same ground.
		expect(fromSelection?.selection).toEqual({ kind: 'ids', ids: ['nte_1', 'nte_2', 'nte_3'] })
	})

	it('covers the document again as the two sections copied one at a time', async () => {
		await mountPanel()
		await installFilledDocument()

		await actions.copySectionAsMarkdown('sec_a')
		await settle(2)
		const research = lastRender()

		await actions.copySectionAsMarkdown('sec_b')
		await settle(2)
		const inbox = lastRender()

		expect(research?.selection).toEqual({ kind: 'section', id: 'sec_a' })
		expect(inbox?.selection).toEqual({ kind: 'section', id: 'sec_b' })
		// Between them the two requests name every section of the document, in the
		// order the document holds them — which is what makes the single-section
		// scope the document scope restricted rather than a second formatting.
		expect(space.sections.value.map((section) => section.id)).toEqual(['sec_a', 'sec_b'])
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

	/**
	 * The same reveal, reached through the signal the list actually subscribes to
	 * rather than by calling the flush by hand.
	 *
	 * Showing the panel does not unmount this tree, so the `visibilitychange`
	 * listener is only as good as the webview's tracking of a parent window it does
	 * not own — and the case the feature exists for is precisely a capture that
	 * arrived before the panel had ever been laid out. The region going from no
	 * height to a height is the same transition the pending request is waiting on,
	 * so that is what has to wake it.
	 *
	 * `ResizeObserver` is replaced rather than driven, since happy-dom lays nothing
	 * out and would never fire one: the test keeps what was observed and hands it
	 * the callback itself.
	 */
	it('flushes the reveal when the region gains a height, with no visibility event', async () => {
		const observed: { target: Element; fire: () => void }[] = []
		// On `window` rather than `globalThis`: VueUse constructs it as
		// `new window.ResizeObserver(...)`, and under vitest the two are not
		// guaranteed to be the same object.
		const realResizeObserver = window.ResizeObserver
		window.ResizeObserver = class {
			readonly callback: () => void
			constructor(callback: () => void) {
				this.callback = callback
			}
			observe(target: Element) {
				observed.push({ target, fire: () => this.callback() })
			}
			unobserve() {}
			disconnect() {}
		} as unknown as typeof ResizeObserver

		try {
			const wrapper = await mountWithTopInsertion()
			document.body.dispatchEvent(paste('pasted at the top'))
			await settle(4)

			const seen: (ScrollIntoViewOptions | undefined)[] = []
			wrapper.get(`[data-row-id="${noteRow('nte_3')}"]`).element.scrollIntoView = (
				options?: boolean | ScrollIntoViewOptions,
			) => {
				seen.push(options as ScrollIntoViewOptions)
			}

			const region = wrapper.get('[data-scroll-region]').element
			expect(observed.map((entry) => entry.target)).toContain(region)

			// The panel is shown: the region is laid out for the first time, which is
			// what the observer reports.
			Object.defineProperty(region, 'clientHeight', { configurable: true, get: () => 120 })
			for (const entry of observed) entry.fire()

			expect(seen).toEqual([{ block: 'nearest' }])
		} finally {
			window.ResizeObserver = realResizeObserver
		}
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
		list.hydrate(null)
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
	 * AC1 / AC2, under the ruling that reversed the short-lived todo default: the
	 * panel opens on the whole document, and each press narrows.
	 *
	 * `nte_2` is `SPACE`'s only done note, so each of the three states is a
	 * different list and the walk through them is observable in one case.
	 */
	it('cycles through everything, hiding done, and done only', async () => {
		const wrapper = await mountPanel()
		const button = wrapper.get('[data-done-filter]')

		expect(renderedRows(wrapper)).toEqual([noteRow('nte_1'), noteRow('nte_2')])

		await button.trigger('click')
		await settle(3)
		expect(renderedRows(wrapper)).toEqual([noteRow('nte_1')])

		await button.trigger('click')
		await settle(3)
		expect(renderedRows(wrapper)).toEqual([noteRow('nte_2')])

		// Round to where it started, so every view is one press from every other.
		await button.trigger('click')
		await settle(3)
		expect(renderedRows(wrapper)).toEqual([noteRow('nte_1'), noteRow('nte_2')])
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

		// Two notes in `SPACE`, one of them done — every offer carries its own
		// document-wide total, so all three labels here end in a count.
		expect(button.text()).toContain('Todo 1')
		expect(button.attributes('aria-label')).toBe('All notes · press for Todo 1')

		await button.trigger('click')
		await settle(2)
		expect(button.text()).toContain('Done 1')
		expect(button.attributes('aria-label')).toBe('Unfinished notes only · press for Done 1')

		await button.trigger('click')
		await settle(2)
		expect(button.text()).toContain('All 2')
		expect(button.attributes('aria-label')).toBe('Done notes only · press for All 2')
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

	/** AC5. Nothing to purge, nothing to press — through both of the states that
	 *  are not the done view, since the button now sits two presses in. */
	it('offers the delete only inside the done view', async () => {
		const wrapper = await mountPanel()
		expect(wrapper.find('[data-delete-done]').exists()).toBe(false)

		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)
		expect(wrapper.find('[data-delete-done]').exists()).toBe(false)

		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)
		expect(wrapper.find('[data-delete-done]').exists()).toBe(true)
	})

	/**
	 * At rest it is a trash icon and nothing else, so the accessible name is the
	 * only name it has. It no longer carries a count or a section: the button
	 * opens a scope choice, and both scopes show their own counts inside the
	 * popover at the moment of choosing.
	 */
	it('rests as an icon whose accessible name names the action', async () => {
		const { wrapper } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		const button = wrapper.get('[data-delete-done]')
		expect(button.text()).toBe('')
		expect(button.attributes('aria-label')).toBe('Delete done notes')
	})

	/** The confirmation popover, portalled to the panel's overlay host — so it is
	 *  found on the document rather than through the wrapper, like the menus. */
	function confirmPopover() {
		return document.querySelector<HTMLElement>('[data-slot="popover-content"]')
	}

	/** AC6. One press asks, the second acts — and the first press must not delete
	 *  anything, which is the whole point of the confirmation. */
	it('asks before deleting, and the first press deletes nothing', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(3)

		expect(calls).toEqual([])
		// Both offers, each with its own count — the disagreement between the two
		// numbers is the reason the popover asks at all.
		const popover = confirmPopover()
		expect(popover?.textContent).toContain('Delete done notes?')
		expect(popover?.querySelector('[data-delete-done-section]')?.textContent).toContain('Research')
		expect(popover?.querySelector('[data-delete-done-section]')?.textContent).toContain('2')
		expect(popover?.querySelector('[data-delete-done-all]')?.textContent).toContain('all sections')
		expect(popover?.querySelector('[data-delete-done-all]')?.textContent).toContain('3')

		popover!.querySelector<HTMLElement>('[data-delete-done-section]')!.click()
		await settle(3)
		expect(calls).toHaveLength(1)
	})

	/** The regression that shipped in 0.2.4: the panel's portal host is
	 *  `pointer-events-none`, reka's menu layer restores `auto` inline and its
	 *  popover layer does not — so without this class the popover's buttons had
	 *  no hover and every pointerdown fell through and dismissed the popover as
	 *  an outside click. Keyboard worked throughout, which is what let it ship. */
	it('carries its own pointer-events, because the portal host has none', async () => {
		const { wrapper } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(3)

		expect(confirmPopover()?.classList.contains('pointer-events-auto')).toBe(true)
	})

	/** AC9. `nte_4` is done and in `sec_b`, and must survive. */
	it('deletes the active section’s done notes and no others', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(3)
		confirmPopover()!.querySelector<HTMLElement>('[data-delete-done-section]')!.click()
		await settle(4)

		// AC7's other half: one call, not one per note. The store pushes one
		// snapshot per `mutate`, so one call is one Ctrl+Z — the depth itself is
		// asserted in `store_fs.rs`, which is the only side that can see it.
		expect(calls).toHaveLength(1)
		expect(calls[0]).toEqual(['nte_1', 'nte_2'])
	})

	/** The wide offer takes every section's done notes in one call — still one
	 *  `delete_notes`, so still one `Ctrl+Z`. */
	it('deletes every section’s done notes from the all-sections offer', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(3)
		confirmPopover()!.querySelector<HTMLElement>('[data-delete-done-all]')!.click()
		await settle(4)

		expect(calls).toHaveLength(1)
		expect(calls[0]).toEqual(['nte_1', 'nte_2', 'nte_4'])
	})

	/** The button now shows whenever the *document* has done notes, so the wide
	 *  offer is reachable while the active section is clean — and the section
	 *  offer is disabled there rather than hidden, holding both scopes in place. */
	it('offers the wide delete while the active section has nothing to purge', async () => {
		const calls: string[][] = []
		const elsewhereOnly: Space = {
			...DONE_IN_BOTH,
			notes: DONE_IN_BOTH.notes.map((entry) =>
				entry.section === 'sec_a' ? { ...entry, done: false } : entry,
			),
		}
		mocks.invoke.mockImplementation(async (command: string, args?: { ids?: string[] }) => {
			if (command === 'get_active_space') return elsewhereOnly
			if (command === 'delete_notes') {
				calls.push(args?.ids ?? [])
				return elsewhereOnly
			}
			return baseInvoke(command)
		})
		const wrapper = await mountPanel()
		await space.refresh()
		await settle(3)

		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(3)

		const section = confirmPopover()?.querySelector<HTMLButtonElement>('[data-delete-done-section]')
		expect(section?.disabled).toBe(true)

		confirmPopover()!.querySelector<HTMLElement>('[data-delete-done-all]')!.click()
		await settle(4)
		expect(calls).toEqual([['nte_4']])
	})

	/** The undo affordance is the message, exactly as the singular delete's is. */
	it('says how to undo', async () => {
		const { wrapper } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(3)
		confirmPopover()!.querySelector<HTMLElement>('[data-delete-done-section]')!.click()
		await settle(4)

		// The chord no longer has to be spelled in the sentence: the pill carries the
		// button that performs the same single step.
		expect(wrapper.text()).toContain('Deleted 2 done notes')
		expect(wrapper.get('[data-sonner-toast] [data-action]').text()).toBe('Undo')
	})

	/**
	 * Reordering stays alive in the narrowed views (user ruling 2026-08-12,
	 * reversing the refusal this test used to pin): the filter hides notes but
	 * never reorders them, and a move anchors to its visible neighbours — the
	 * mechanics live with the reordering suite's own 'under the done filter'
	 * block. Here, only the view's surface: the grip stays through the cycle.
	 */
	it('keeps the drag handle through every filter state', async () => {
		const wrapper = await mountPanel()
		expect(wrapper.find('[data-drag-handle]').exists()).toBe(true)

		list.setDoneFilter('todo')
		await settle(2)
		expect(wrapper.find('[data-drag-handle]').exists()).toBe(true)

		list.setDoneFilter('done')
		await settle(2)
		expect(wrapper.find('[data-drag-handle]').exists()).toBe(true)
	})

	/**
	 * The other emptiness, and the reason the copy could not stay one sentence:
	 * the done view is empty when nothing is finished, the todo view when
	 * everything is. Saying the wrong one states the exact opposite of the truth.
	 */
	it('explains a todo view with nothing left to do', async () => {
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'get_active_space') {
				return { ...SPACE, notes: SPACE.notes.map((entry) => ({ ...entry, done: true })) }
			}
			return baseInvoke(command)
		})
		const wrapper = await mountPanel()
		await space.refresh()
		await settle(3)

		// At rest the done notes are simply on screen — the emptiness only exists
		// once the todo view drops them.
		expect(wrapper.text()).not.toContain('Everything here is done.')

		list.setDoneFilter('todo')
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
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(3)

		expect(wrapper.text()).toContain('Nothing is done yet.')
	})

	/**
	 * The section offer names the **section**, because the count and the view can
	 * legitimately disagree: the filter shows done notes document-wide, and the
	 * two offers make both readings of "delete done" reachable — each wearing the
	 * count that tells them apart at the moment of choosing.
	 */
	it('names the section scope and shows both counts', async () => {
		const { wrapper } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		// Three done notes are on screen; the section offer covers two of them.
		expect(wrapper.findAll('[data-note-row]')).toHaveLength(3)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(3)

		const section = confirmPopover()?.querySelector('[data-delete-done-section]')
		expect(section?.textContent).toContain('In Research')
		expect(section?.textContent).toContain('2')
		expect(confirmPopover()?.querySelector('[data-delete-done-all]')?.textContent).toContain('3')
		// The button itself stays icon-only in every state, which is what keeps the
		// chip beside it from ever being pushed out of the header.
		expect(wrapper.get('[data-delete-done]').text()).toBe('')
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
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(3)
		expect(confirmPopover()).not.toBeNull()

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

		// The offer went away rather than silently re-aiming: the popover closed, so
		// the next press asks again about the new set instead of deleting the old
		// one.
		expect(confirmPopover()).toBeNull()
		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(2)
		expect(calls).toEqual([])
	})

	/**
	 * A held Enter must not open the popover and confirm inside one hold. The
	 * browser synthesises a click from every repeat of the keydown, and reka
	 * autofocuses the content on open — so wherever focus lands, the repeat is
	 * refused at the source on the popover content, before any click exists.
	 * The same guard the old inline form carried, moved to where the
	 * destructive controls now live.
	 */
	it('refuses the repeat of a held activation key inside the popover', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(3)

		const target = confirmPopover()!.querySelector<HTMLElement>('[data-delete-done-section]')!
		const repeat = new KeyboardEvent('keydown', {
			key: 'Enter',
			repeat: true,
			bubbles: true,
			cancelable: true,
		})
		target.dispatchEvent(repeat)
		await settle(2)

		expect(repeat.defaultPrevented).toBe(true)
		expect(calls).toEqual([])
		expect(confirmPopover()).not.toBeNull()
	})

	/** The second click of a double-click is the same gesture as the first: it
	 *  closes the popover the first click opened, and deletes nothing. */
	it('does not let a double-click arm and confirm in one gesture', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		const button = wrapper.get('[data-delete-done]')
		await button.trigger('click', { detail: 1 })
		await settle(2)
		await button.trigger('click', { detail: 2 })
		await settle(3)

		expect(calls).toEqual([])
		expect(confirmPopover()).toBeNull()

		// A deliberate separate flow still works.
		await button.trigger('click', { detail: 1 })
		await settle(3)
		confirmPopover()!.querySelector<HTMLElement>('[data-delete-done-section]')!.click()
		await settle(3)
		expect(calls).toHaveLength(1)
	})

	/** Escape dismisses the question through reka's own layer, resolving at the
	 *  shell's `inOverlay` guard — so it neither hides the panel nor takes any
	 *  other rung of the Escape ladder with it. */
	it('backs out on Escape without taking a rung of the ladder', async () => {
		const { wrapper, calls } = await mountWithDoneInBoth()
		await wrapper.get('[data-done-filter]').trigger('click')
		await wrapper.get('[data-done-filter]').trigger('click')
		await settle(2)

		await wrapper.get('[data-delete-done]').trigger('click')
		await settle(3)
		expect(confirmPopover()).not.toBeNull()

		confirmPopover()!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
		await settle(3)

		expect(confirmPopover()).toBeNull()
		expect(calls).toEqual([])
		expect(mocks.invoke).not.toHaveBeenCalledWith('hide_panel')
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

		expect(mocks.invoke).not.toHaveBeenCalledWith('reorder_notes', expect.anything())
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
	 * The sort and the search are the two conditions left refusing reordering —
	 * the done filter reorders by visible anchor since the 2026-08-12 ruling —
	 * and the refusal covers every path: the grip is gone for the pointer,
	 * `reorderBlocked` answers the keyboard, and the commit itself refuses in
	 * case a drag is ever started some other way.
	 */
	it('refuses the drag commit itself while a sort is active', async () => {
		const wrapper = await mountPanel()
		list.setSort('newest')
		await settle(3)

		await actions.finishDrag('nte_2', 'sec_a', 0)
		await settle(3)

		expect(mocks.invoke).not.toHaveBeenCalledWith('reorder_notes', expect.anything())
		expect(wrapper.text()).toContain('Set the sort to Manual to reorder notes.')
	})

	/**
	 * `blockUnmoved` answers in **document** coordinates, which is what
	 * `finishDrag`'s no-op check needs — `useNoteDrag` counts the destination
	 * index over the whole section, so a position taken from the rendered rows
	 * compares two different coordinate systems.
	 *
	 * Collapse is the condition this is observable under. Every *other* way the
	 * rendered rows can disagree with the document — a query, the done filter, a
	 * non-manual sort — is refused outright by `reorderBlocked` before the no-op
	 * check runs, so the defect was unreachable rather than absent. A collapsed
	 * section is not refused, and it publishes an empty note list: a position
	 * read off `visibleGroups` there returned -1, which never equals the index,
	 * so a drag that changed nothing went to the store and pushed an undo entry
	 * the user then had to press Ctrl+Z to get rid of.
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

		expect(mocks.invoke).not.toHaveBeenCalledWith('reorder_notes', expect.anything())
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
	})

	/**
	 * The line reads how long ago rather than which day, and the absolute date it
	 * gives up is on the same element as a `title`. Asserted as a pair, because
	 * either one alone is a footer that has lost half its meaning: relative text
	 * with no hover is a note whose day is unrecoverable, and a title with no
	 * relative text is the old behaviour wearing a tooltip.
	 */
	it('reads as elapsed time, with the exact date on hover', async () => {
		const wrapper = await mountWithDates()
		const stamp = wrapper.findAll('time')[0]!

		expect(stamp.text()).not.toContain('2026')
		expect(stamp.attributes('title')).toContain('2026')
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
	 * **The pill sits at the foot of the list, and what put it at the head of it
	 * was grid auto-placement rather than alignment.**
	 *
	 * The band is definite in both axes, so it is placed before auto-placement
	 * runs and its cell counts as occupied for everything after it — and nothing
	 * auto-placed is ever put on top of an occupied cell. Whichever axis a sibling
	 * leaves to the grid is the axis it gets moved in.
	 *
	 * With no placement at all, the three were pushed a *row* past where they
	 * belong: the region into a content-sized row 3, the composer into an implicit
	 * row 4, and the pill was correctly at the bottom of a row 2 that had become
	 * the empty strip above the list. Naming only the row moved the failure into
	 * the *column*: the region, locked to row 2 but auto in its column, was placed
	 * at the earliest column that did not overlap the band — an implicit second
	 * track, 111px of a 441px panel — while the header and the composer stayed in
	 * the first. Note bodies wrapped at a character or two per line.
	 *
	 * So the assertion is over both axes and over every flow child, not over the
	 * three by name: leaving *either* axis to the grid on *any* of them is the
	 * whole bug, including on a child added later. happy-dom lays nothing out, so
	 * the placement classes are as close to the real thing as this environment
	 * reaches — that the resulting single column then fills the panel is the one
	 * part only a real render can show.
	 */
	it('places every flow child of the shell in both axes, in one column', async () => {
		const wrapper = await mountPanel()
		status.setMessage('Copied 1 note')
		// One macrotask, not one tick: Sonner's Toaster takes the message in a
		// `nextTick` callback of its own, and the render is a tick behind that.
		await settle(1)

		const root = wrapper.get('[data-panel-root]')
		expect(root.classes()).toContain('grid-cols-1')

		// Out-of-flow children are not grid items and place themselves: the portal
		// host, the clamp probe and the two live regions. `transition-stub` is VTU's
		// stand-in for the image viewer's `<Transition>`, which at runtime renders
		// no element of its own.
		const flow = [...root.element.children].filter(
			(child) =>
				child.tagName !== 'TRANSITION-STUB' &&
				!/(^|\s)(absolute|sr-only)(\s|$)/.test(child.className),
		)
		expect(flow.length).toBeGreaterThanOrEqual(4)
		for (const child of flow) {
			expect
				.soft(child.className, `${child.tagName} names a column`)
				.toMatch(/(^|\s)col-start-1(\s|$)/)
			expect
				.soft(child.className, `${child.tagName} names a row`)
				.toMatch(/(^|\s)row-start-[123](\s|$)/)
		}

		// And the rows they name: the two fixed bands either side of the region, and
		// the toast host sharing the region's cell rather than taking a row from it.
		expect(wrapper.get('header').classes()).toContain('row-start-1')
		expect(wrapper.get('[data-scroll-region]').classes()).toContain('row-start-2')
		expect(wrapper.get('form').classes()).toContain('row-start-3')
		// Through `closest` rather than a parent chain: the toast sits inside
		// Sonner's own list and section, so the host is levels above it and
		// counting the hops would break the moment the library gains one.
		const band = wrapper.get('[data-sonner-toast]').element.closest('.row-start-2')
		expect(band).not.toBeNull()
		// And it is a child of the grid itself, so the cell it names is a cell of the
		// shell rather than of something nested inside the region.
		expect(band?.parentElement).toBe(root.element)
	})

	/**
	 * The toasts overlay the last rows of the list for five seconds after every
	 * action, so the *bare* parts of their host must not eat presses aimed at
	 * what is underneath. The host cell is click-through; each toast re-enables
	 * pointer events for itself — hovering one is what holds its clock, and its
	 * button is what does anything.
	 */
	it('is click-through outside the toasts themselves', async () => {
		const wrapper = await mountPanel()
		status.setMessage('Copied 1 note', { label: 'Undo', run: () => {} })
		await settle(1)

		const pill = wrapper.get('[data-sonner-toast]')
		expect(pill.classes()).toContain('pointer-events-auto')
		expect(pill.element.closest('.pointer-events-none')).not.toBeNull()
		expect(wrapper.get('[data-sonner-toast] [data-action]').text()).toBe('Undo')
	})

	/** The stack, which is what replaced the single pill (user direction,
	 *  2026-08-11): a second message joins the first rather than taking its
	 *  surface, so marking five notes done leaves five `Undo`s, each undoing its
	 *  own press. */
	it('stacks a second message rather than replacing the first', async () => {
		const wrapper = await mountPanel()
		status.setMessage('Copied 1 note')
		await settle(1)
		status.setMessage('Copied 3 notes')
		await settle(1)

		expect(wrapper.findAll('[data-sonner-toast]')).toHaveLength(2)
		expect(wrapper.text()).toContain('Copied 1 note')
		expect(wrapper.text()).toContain('Copied 3 notes')
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
			// Two ticks under the fake clock — `settle` would wait on a timer that
			// never fires. The first flushes the Toaster's own `nextTick` intake,
			// the second the render behind it.
			await wrapper.vm.$nextTick()
			await wrapper.vm.$nextTick()
			expect(wrapper.find('[data-sonner-toast]').exists()).toBe(true)

			await wrapper.trigger('keydown', { key: 'Escape' })
			expect(wrapper.find('[data-sonner-toast]').exists()).toBe(true)

			// The expiry, then Sonner's 200ms unmount delay — the element outlives
			// its dismissal by the length of the exit animation.
			vi.advanceTimersByTime(5000)
			await wrapper.vm.$nextTick()
			vi.advanceTimersByTime(300)
			await wrapper.vm.$nextTick()
			expect(wrapper.find('[data-sonner-toast]').exists()).toBe(false)
		} finally {
			vi.useRealTimers()
		}
	})

	/**
	 * Marking a note done in the todo view is an action whose only visible result
	 * is a row leaving the list, which is exactly why the toast carries the way
	 * back. One press of `Undo` is one store step — the same one `Ctrl+Z` takes —
	 * and a batch is already one step, so it restores all of it.
	 */
	it('reports a note moved to Done and offers one undo step', async () => {
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'set_notes_done') {
				return { ...SPACE, notes: SPACE.notes.map((entry) => ({ ...entry, done: true })) }
			}
			return baseInvoke(command)
		})
		// The narrowed view, where marking done is what removes a row. The resting
		// `all` view keeps the row on screen and hands nothing on.
		list.setDoneFilter('todo')
		const wrapper = await mountPanel()
		// The fixture's done note is already hidden here.
		expect(wrapper.findAll('[data-note-row]')).toHaveLength(1)

		selection.select('nte_1')
		await settle(2)
		await actions.toggleDone()
		await settle(4)

		expect(wrapper.findAll('[data-note-row]')).toHaveLength(0)
		expect(wrapper.get('[data-sonner-toast]').text()).toContain('Moved 1 note to Done')

		await wrapper.get('[data-sonner-toast] [data-action]').trigger('click')
		await settle(3)

		expect(mocks.invoke).toHaveBeenCalledWith('undo')
		// The offer is spent, so the toast does not stay up inviting a second press
		// at a step that has already been taken. Sonner keeps the element for its
		// 200ms exit animation, so the wait is real time, not ticks.
		await toastGone()
		expect(wrapper.find('[data-sonner-toast] [data-action]').exists()).toBe(false)
	})

	/**
	 * **The button undoes the top of the store's stack, so the pill has to be
	 * retired by anything that changes what that is.** Most mutations push a step
	 * and report nothing — a composer submit, a paste, a drag, an Alt+Arrow, a
	 * `Move to ▸` — so without the invalidation in `useSpace.mutate` this exact
	 * sequence left "Moved 1 note to Done" on screen over a button that removed the
	 * note just written.
	 */
	it('retires the pill when a later mutation lands without one of its own', async () => {
		const wrapper = await mountPanel()

		selection.select('nte_1')
		await settle(2)
		await actions.toggleDone()
		await settle(4)
		expect(wrapper.get('[data-sonner-toast]').text()).toContain('Moved 1 note to Done')

		const composer = wrapper.find('#composer')
		await composer.setValue('a new note')
		await composer.trigger('keydown', { key: 'Enter' })
		await settle(4)

		expect(mocks.invoke).toHaveBeenCalledWith(
			'submit_entry',
			expect.objectContaining({ body: 'a new note' }),
		)
		// Message and button together: a toast naming a step that is no longer the
		// one its button would take has no honest half left.
		await toastGone()
		expect(wrapper.find('[data-sonner-toast]').exists()).toBe(false)
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

		expect(wrapper.get('[data-sonner-toast]').text()).toContain('Moved 1 note out of Done')
	})

	/**
	 * The row that vanishes takes focus with it, and it is handed on the same way a
	 * delete hands it on — through the row reconciliation already chose, rather
	 * than through a second rule about where focus goes.
	 */
	it('hands focus on when the marked note leaves the todo view', async () => {
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
		// The narrowed view, where marking done is what removes a row. The resting
		// `all` view keeps the row on screen and hands nothing on.
		list.setDoneFilter('todo')
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

	/**
	 * **The completion circle is a control inside the row, and the press that
	 * toggles the note takes DOM focus itself.** So the note focus has to be handed
	 * on *from* is the one whose row holds the focused element — not the roving
	 * target, which in a panel nobody has arrowed through is nothing at all, and
	 * otherwise is some other note still happily on screen. Asked that way,
	 * `handFocusOnVanished` correctly declines both times, the row leaves with the
	 * focused button inside it, and focus falls to `<body>` where no arrow key does
	 * anything.
	 *
	 * Focused explicitly because happy-dom dispatches a click without moving focus;
	 * a real press focuses the button first, which is the state under test.
	 *
	 * **These assert the guarantee, not the mechanism, and they cannot do more
	 * here.** The failure in a real WebView needs the leave animation's exit
	 * window — the row is removed from the list and still `isConnected` at the
	 * tick `restoreDom` runs, so its own handoff declines; the same race
	 * `handFocusOnVanished` was written for. In this environment the row leaves
	 * synchronously, `restoreDom` covers the same ground, and no arrangement of
	 * the stubbed WAAPI reopens the window.
	 */
	describe('clicking the completion circle', () => {
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

		async function mountThree() {
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
			// The narrowed view, in which a note marked done leaves the list — the
			// resting `all` view keeps it on screen and hands nothing on.
			list.setDoneFilter('todo')
			const wrapper = await mountPanel()
			await space.refresh()
			await settle(3)
			return wrapper
		}

		async function clickCircle(wrapper: Awaited<ReturnType<typeof mountPanel>>, noteId: string) {
			const circle = wrapper.get(`[data-row-id="${noteRow(noteId)}"] [data-slot="checkbox"]`)
			;(circle.element as HTMLElement).focus()
			await circle.trigger('click')
			await settle(5)
		}

		it('hands focus on from the clicked row, which the click made the roving target', async () => {
			const wrapper = await mountThree()
			selection.select('nte_1')
			takeRow(noteRow('nte_1'))
			await settle(2)

			await clickCircle(wrapper, 'nte_3')

			expect(wrapper.findAll('[data-note-row]')).toHaveLength(2)
			// The grid's focusin sync moves the roving target to the clicked row —
			// that is what keeps the arrows resuming from where the user actually
			// acted — so when `nte_3` leaves the narrowed view, focus hands on to
			// *its* nearest survivor rather than back to wherever the arrows last
			// were.
			expect(selection.focusedId.value).toBe(noteRow('nte_2'))
			expect(document.activeElement).toBe(
				wrapper.get(`[data-row-id="${noteRow('nte_2')}"]`).element,
			)
		})

		it('still walks focus forward when the clicked note is the roving target', async () => {
			const wrapper = await mountThree()
			selection.select('nte_2')
			takeRow(noteRow('nte_2'))
			await settle(2)

			await clickCircle(wrapper, 'nte_2')

			// Reconciliation's nearest-survivor walk, unchanged by reading the handoff
			// source off the DOM: forward first, and the row it chose has focus.
			expect(selection.focusedId.value).toBe(noteRow('nte_3'))
			expect(document.activeElement).toBe(
				wrapper.get(`[data-row-id="${noteRow('nte_3')}"]`).element,
			)
		})
	})
})
