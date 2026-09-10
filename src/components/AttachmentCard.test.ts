import { mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'
import { defineComponent, h } from 'vue'

import AttachmentCard from './AttachmentCard.vue'
import { useAttachments, type Attachment } from '@/composables/useAttachments'
import { useOverlayHost } from '@/composables/useOverlayHost'
import { useAttachmentDrag } from '@/composables/useAttachmentDrag'
import { useAttachmentSelection } from '@/composables/useAttachmentSelection'
import { useNoteList } from '@/composables/useNoteList'
import { useNoteSearch } from '@/composables/useNoteSearch'
import { useSections } from '@/composables/useSections'
import { useSelection } from '@/composables/useSelection'
import { useSpace, type Space } from '@/composables/useSpace'

/**
 * The card's own context menu, and the two entries on it.
 *
 * Three things here are worth pinning and the rest is not. The reveal entry has
 * to reach `attachment_reveal` rather than `attachment_open`, because the two
 * differ exactly in that the second one may *launch* what it is given. Each has
 * to carry the content-addressed `file` — the argument Rust rebuilds the path
 * from — where the attachment's `id` would be the plausible mistake: both fields
 * are strings, so nothing but a test can tell them apart. And the OS route has to
 * be *here*: it used to be a bare `Space` on the card, which is the key a button
 * must use to do what `Enter` does, so the menu is now the only place it exists.
 *
 * Statically imported, like the component: the composables behind it hold
 * module-scoped state by design, and a dynamic import would hand this file a
 * second instance of it.
 */

const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))

const attachments = useAttachments()
const { setOverlayHost } = useOverlayHost()
const selection = useAttachmentSelection()
const space = useSpace()
const drag = useAttachmentDrag()

/** A `.pdf`, so the card is available without being viewable — the plain case.
 *  Its `id` and its `file` are deliberately nothing like each other. */
const PDF: Attachment = {
	id: 'att_1',
	file: 'b1946ac92492d2347c6235b4d2611184.pdf',
	name: 'brief.pdf',
	mime: 'application/pdf',
	bytes: 2048,
}

const FILES = [
	PDF,
	{ ...PDF, id: 'att_2', file: 'second.pdf', name: 'second.pdf' },
	{ ...PDF, id: 'att_3', file: 'third.pdf', name: 'third.pdf' },
]
const TARGETS = FILES.map((attachment) => ({ note: 'nte_1', attachment: attachment.id }))
const SPACE: Space = {
	id: 'test-space',
	name: 'Synthetic attachments',
	activeSection: 'section',
	sections: [{ id: 'section', name: 'Notes', order: 0 }],
	notes: [
		{
			id: 'nte_1',
			section: 'section',
			order: 0,
			body: 'Test note',
			done: false,
			created: '',
			updated: '',
			attachments: FILES,
		},
	],
}
const SOURCE = { path: 'C:\\synthetic-fixtures\\attachments.copper', id: SPACE.id }

async function baseInvoke(command: string) {
	if (command === 'get_active_space') return structuredClone(SPACE)
	if (command === 'get_status') {
		return {
			path: SOURCE.path,
			errored: false,
			watching: true,
			canUndo: false,
			canRedo: false,
			startupNotice: null,
		}
	}
	if (command === 'get_settings') return { doneFilter: 'all', sortMode: 'manual' }
	if (command === 'attachment_thumb') return new ArrayBuffer(0)
	if (command === 'attachment_prepare_drag') return 'drag-ticket'
	if (command === 'attachment_start_drag') return true
	return null
}

let wrapper: ReturnType<typeof mount> | null = null
let host: HTMLElement | null = null
let finishPreparation: (() => void) | null = null
let finishNative: (() => void) | null = null

async function settle(turns = 3) {
	for (let i = 0; i < turns; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

beforeEach(async () => {
	// The preview cache is module state and outlives a mount, so without this the
	// second case's request is deduped against the first case's answer.
	attachments.clearPreviews()
	mocks.invoke.mockReset()
	// An empty answer is Rust's "the file is there and has nothing to show",
	// which is what a `.pdf` gets.
	mocks.invoke.mockImplementation(baseInvoke)
	selection.syncDocument(null)
	useSelection().resetForNewSpace()
	useSections().reset()
	useNoteSearch().clearQuery()
	useNoteList().hydrate(null)
	await space.load()

	// The menu is portalled, and it renders only once a host exists — `PanelShell`
	// publishes one in the running app, and there is no shell here.
	host = document.createElement('div')
	document.body.append(host)
	setOverlayHost(host, host)
})

afterEach(async () => {
	wrapper?.unmount()
	wrapper = null
	finishPreparation?.()
	finishPreparation = null
	finishNative?.()
	finishNative = null
	await settle()
	selection.clear()
	setOverlayHost(null, null)
	host?.remove()
	host = null
})

async function openCardMenu() {
	wrapper = mount(AttachmentCard, {
		attachTo: document.body,
		props: { attachment: PDF, note: 'nte_1', tabIndex: 0 },
	})
	await settle()

	await wrapper.get('[data-attachment-open]').trigger('contextmenu')
	await settle()

	const content = document.querySelector<HTMLElement>('[data-slot="context-menu-content"]')
	expect(content, 'the attachment context menu did not open').not.toBeNull()
	return content!
}

function itemNamed(content: HTMLElement, label: string) {
	const item = [...content.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((entry) =>
		entry.textContent?.includes(label),
	)
	expect(item, `no menu entry named ${label}`).not.toBeUndefined()
	return item!
}

describe('the attachment context menu', () => {
	it('offers the two routes out of the panel, each named for where it goes', async () => {
		const content = await openCardMenu()

		const items = [...content.querySelectorAll<HTMLElement>('[role="menuitem"]')]
		expect(items.map((item) => item.textContent?.trim())).toEqual(
			expect.arrayContaining(['Open in default app', 'Open attachment location']),
		)
	})

	it('reveals through attachment_reveal, carrying the stored name and not the id', async () => {
		const content = await openCardMenu()

		itemNamed(content, 'Open attachment location').click()
		await settle()

		expect(mocks.invoke).toHaveBeenCalledWith('attachment_reveal', { file: PDF.file })
		// The other half of the pair stays out of it. It is the arm that launches.
		expect(mocks.invoke).not.toHaveBeenCalledWith('attachment_open', expect.anything())
	})

	/** The route `Space` used to be. It is the only one left, so if this entry ever
	 *  stops reaching `attachment_open` the card cannot open its file at all. */
	it('opens in the OS handler through attachment_open, on the entry that says so', async () => {
		const content = await openCardMenu()

		itemNamed(content, 'Open in default app').click()
		await settle()

		expect(mocks.invoke).toHaveBeenCalledWith('attachment_open', { file: PDF.file })
	})
})

/**
 * A button activates on `Enter` and on `Space`, identically, everywhere. This
 * card broke that: `Space` launched the OS handler while `Enter` opened the
 * in-panel viewer, so the same control did two different things depending on
 * which of the two activation keys the user reached for.
 *
 * A `.pdf` is the shape that makes the assertion legible — nothing to view, so
 * both keys end at `attachment_open` and the test is about *sameness* rather than
 * about which destination won.
 */
describe('keyboard activation', () => {
	async function press(key: string) {
		wrapper = mount(AttachmentCard, {
			attachTo: document.body,
			props: { attachment: PDF, note: 'nte_1', tabIndex: 0 },
		})
		await settle()
		mocks.invoke.mockClear()

		wrapper
			.get('[data-attachment-open]')
			.element.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }))
		await settle()
	}

	it('treats Space exactly as Enter', async () => {
		await press('Enter')
		const onEnter = mocks.invoke.mock.calls.filter(([command]) => command !== 'attachment_thumb')

		wrapper?.unmount()
		wrapper = null

		await press(' ')
		const onSpace = mocks.invoke.mock.calls.filter(([command]) => command !== 'attachment_thumb')

		expect(onEnter).toEqual([['attachment_open', { file: PDF.file }]])
		expect(onSpace).toEqual(onEnter)
	})
})

function pointer(target: EventTarget, type: string, options: PointerEventInit = {}) {
	target.dispatchEvent(
		new PointerEvent(type, {
			bubbles: true,
			pointerType: 'mouse',
			pointerId: 7,
			button: 0,
			buttons: 1,
			clientX: 10,
			clientY: 10,
			...options,
		}),
	)
}

async function mountFiles() {
	const parentClick = vi.fn()
	const parentDoubleClick = vi.fn()
	wrapper = mount(
		defineComponent({
			setup: () => () =>
				h(
					'div',
					{ onClick: parentClick, onDblclick: parentDoubleClick },
					FILES.map((attachment) => h(AttachmentCard, { attachment, note: 'nte_1', tabIndex: 0 })),
				),
		}),
		{ attachTo: document.body },
	)
	await settle()
	mocks.invoke.mockClear()
	const buttons = wrapper.findAll('[data-attachment-open]')
	return { buttons, parentClick, parentDoubleClick }
}

function delayPreparation() {
	const pending = new Promise<string>((resolve) => {
		finishPreparation = () => resolve('drag-ticket')
	})
	mocks.invoke.mockImplementation((command) =>
		command === 'attachment_prepare_drag' ? pending : baseInvoke(command),
	)
}

function expectNoMutation() {
	expect(mocks.invoke.mock.calls.map(([command]) => command)).not.toEqual(
		expect.arrayContaining([expect.stringMatching(/clipboard|done|copy/)]),
	)
	expect(space.space.value).toEqual(SPACE)
}

describe('file-style attachment gestures', () => {
	it('has no checkbox and selects one file without opening or selecting its note', async () => {
		const { buttons, parentClick } = await mountFiles()
		expect(wrapper!.find('[role="checkbox"], input[type="checkbox"]').exists()).toBe(false)
		useSelection().select('nte_1')
		await buttons[0]!.trigger('click')
		expect(selection.selected.value).toEqual([TARGETS[0]])
		expect(useSelection().selectedIds.value).toEqual([])
		expect(buttons[0]!.attributes('aria-pressed')).toBe('true')
		expect(parentClick).not.toHaveBeenCalled()
		expect(mocks.invoke).not.toHaveBeenCalled()
	})

	it('toggles with Ctrl, extends with Shift, and replaces a range with a plain click', async () => {
		const { buttons } = await mountFiles()
		await buttons[0]!.trigger('click')
		await buttons[2]!.trigger('click', { ctrlKey: true })
		expect(selection.selected.value).toEqual([TARGETS[0], TARGETS[2]])
		await buttons[2]!.trigger('click', { ctrlKey: true })
		expect(selection.selected.value).toEqual([TARGETS[0]])
		await buttons[0]!.trigger('click')
		await buttons[2]!.trigger('click', { shiftKey: true })
		expect(selection.selected.value).toEqual(TARGETS)
		await buttons[1]!.trigger('click')
		expect(selection.selected.value).toEqual([TARGETS[1]])
		expect(mocks.invoke).not.toHaveBeenCalled()
	})

	it('keeps sub-threshold movement as a click, not a native drag', async () => {
		const { buttons } = await mountFiles()
		pointer(buttons[1]!.element, 'pointerdown')
		pointer(window, 'pointermove', { clientX: 13, clientY: 14 })
		expect(mocks.invoke).not.toHaveBeenCalled()
		pointer(window, 'pointerup', { buttons: 0 })
		await buttons[1]!.trigger('click')
		expect(selection.selected.value).toEqual([TARGETS[1]])
		expect(mocks.invoke).not.toHaveBeenCalled()
	})

	it('starts once at the window movement threshold and keeps a selected multi-file drag intact', async () => {
		const { buttons, parentClick, parentDoubleClick } = await mountFiles()
		await buttons[0]!.trigger('click')
		await buttons[1]!.trigger('click', { ctrlKey: true })
		pointer(buttons[0]!.element, 'pointerdown')
		expect(selection.selected.value).toEqual(TARGETS.slice(0, 2))
		expect(mocks.invoke).not.toHaveBeenCalled()
		pointer(window, 'pointermove', { clientX: 16 })
		pointer(window, 'pointermove', { clientX: 40 })
		await settle()
		expect(mocks.invoke.mock.calls).toEqual([
			['attachment_prepare_drag', { targets: TARGETS.slice(0, 2), source: SOURCE }],
			['attachment_start_drag', { id: 'drag-ticket' }],
		])
		pointer(window, 'pointerup', { buttons: 0 })
		await buttons[0]!.trigger('click')
		await buttons[0]!.trigger('dblclick')
		expect(selection.selected.value).toEqual(TARGETS.slice(0, 2))
		expect(mocks.invoke).not.toHaveBeenCalledWith('attachment_open', expect.anything())
		expect(parentClick).not.toHaveBeenCalled()
		expect(parentDoubleClick).not.toHaveBeenCalled()
		expectNoMutation()
		pointer(buttons[0]!.element, 'pointerdown')
		pointer(window, 'pointerup', { buttons: 0 })
		await buttons[0]!.trigger('click')
		await buttons[0]!.trigger('dblclick')
		expect(selection.selected.value).toEqual([TARGETS[0]])
		expect(mocks.invoke).toHaveBeenCalledWith('attachment_open', { file: PDF.file })
	})

	it('selects an unselected dragged file instead of exporting the old selection', async () => {
		const { buttons } = await mountFiles()
		selection.select(TARGETS[0]!)
		selection.toggle(TARGETS[1]!)
		pointer(buttons[2]!.element, 'pointerdown')
		pointer(window, 'pointermove', { clientY: 16 })
		await settle()
		expect(selection.selected.value).toEqual([TARGETS[2]])
		expect(mocks.invoke).toHaveBeenCalledWith('attachment_prepare_drag', {
			targets: [TARGETS[2]],
			source: SOURCE,
		})
		expectNoMutation()
	})

	it.each([
		{ pointerType: 'touch', button: 0 },
		{ pointerType: 'pen', button: 0 },
		{ pointerType: 'mouse', button: 2 },
	])('does not start native dragging for $pointerType button $button', async (options) => {
		const { buttons } = await mountFiles()
		pointer(buttons[0]!.element, 'pointerdown', options)
		pointer(window, 'pointermove', { ...options, clientX: 30 })
		await settle()
		expect(mocks.invoke).not.toHaveBeenCalled()
	})

	it('ignores another pointer and abandons a move without the primary button held', async () => {
		const { buttons } = await mountFiles()
		pointer(buttons[0]!.element, 'pointerdown')
		pointer(window, 'pointermove', { pointerId: 8, clientX: 30 })
		pointer(window, 'pointermove', { buttons: 0, clientX: 30 })
		pointer(window, 'pointermove', { clientX: 40 })
		await settle()
		expect(mocks.invoke).not.toHaveBeenCalled()
	})

	it.each(['pointerup', 'Escape'])(
		'discards a dispatched native drag on %s before the native result resolves',
		async (cancel) => {
			const { buttons, parentClick, parentDoubleClick } = await mountFiles()
			const pending = new Promise<boolean>((resolve) => {
				finishNative = () => resolve(false)
			})
			mocks.invoke.mockImplementation((command) =>
				command === 'attachment_start_drag' ? pending : baseInvoke(command),
			)
			selection.select(TARGETS[0]!)
			selection.toggle(TARGETS[1]!)
			pointer(buttons[0]!.element, 'pointerdown')
			pointer(window, 'pointermove', { clientX: 16 })
			await settle()
			expect(mocks.invoke).toHaveBeenCalledWith('attachment_start_drag', { id: 'drag-ticket' })
			if (cancel === 'Escape') {
				window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
			} else {
				pointer(window, 'pointerup', { buttons: 0 })
			}
			expect(mocks.invoke.mock.calls).toEqual([
				['attachment_prepare_drag', { targets: TARGETS.slice(0, 2), source: SOURCE }],
				['attachment_start_drag', { id: 'drag-ticket' }],
				['attachment_discard_drag', { id: 'drag-ticket' }],
			])
			expect(drag.dragging.value).toBe(true)
			await buttons[0]!.trigger('click')
			await buttons[0]!.trigger('dblclick')
			expect(selection.selected.value).toEqual(TARGETS.slice(0, 2))
			expect(parentClick).not.toHaveBeenCalled()
			expect(parentDoubleClick).not.toHaveBeenCalled()
			expect(mocks.invoke).not.toHaveBeenCalledWith('attachment_open', expect.anything())
			finishNative!()
			await settle()
			expect(drag.dragging.value).toBe(false)
			expectNoMutation()
		},
	)

	it.each(['pointerup', 'pointercancel', 'Escape', 'blur', 'unmount'])(
		'discards pending preparation after %s without opening or collapsing on trailing clicks',
		async (cancel) => {
			const { buttons, parentClick, parentDoubleClick } = await mountFiles()
			selection.select(TARGETS[0]!)
			selection.toggle(TARGETS[1]!)
			delayPreparation()
			pointer(buttons[0]!.element, 'pointerdown')
			pointer(window, 'pointermove', { clientX: 16 })
			expect(drag.dragging.value).toBe(true)
			if (cancel === 'unmount') {
				wrapper!.unmount()
				wrapper = null
			} else if (cancel === 'Escape') {
				window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
			} else if (cancel === 'blur') {
				window.dispatchEvent(new Event('blur'))
			} else {
				pointer(window, cancel, { buttons: 0 })
			}
			finishPreparation!()
			await settle()
			expect(mocks.invoke.mock.calls).toEqual([
				['attachment_prepare_drag', { targets: TARGETS.slice(0, 2), source: SOURCE }],
				['attachment_discard_drag', { id: 'drag-ticket' }],
			])
			expect(drag.dragging.value).toBe(false)
			if (wrapper) {
				await buttons[0]!.trigger('click')
				await buttons[0]!.trigger('dblclick')
			}
			expect(selection.selected.value).toEqual(TARGETS.slice(0, 2))
			expect(mocks.invoke).not.toHaveBeenCalledWith('attachment_open', expect.anything())
			expect(parentClick).not.toHaveBeenCalled()
			expect(parentDoubleClick).not.toHaveBeenCalled()
			expectNoMutation()
		},
	)
})
