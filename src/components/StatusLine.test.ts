import { mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import StatusLine from './StatusLine.vue'
import { useStatusMessage } from '@/composables/useStatusMessage'

/**
 * The pill's own contract, away from the shell that hosts it.
 *
 * Three things here cannot be seen from `PanelShell.test.ts` without a panel in
 * the way: what the clock does while the reader is at the button or the window
 * is hidden, that a failure has no clock at all, and where focus lands when the
 * button that had it is removed. The band, the layering and the click-through
 * belong to the shell and are asserted there.
 */

const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
	listen: async () => () => {},
	emit: async () => {},
}))

const status = useStatusMessage()

let wrapper: ReturnType<typeof mount> | null = null
let panel: HTMLElement | null = null
let hidden = false

/** Enough turns for a leave transition to finish: Vue schedules the class swap a
 *  frame out, and happy-dom's `requestAnimationFrame` is a timer like any
 *  other. */
async function settle(turns = 6) {
	for (let i = 0; i < turns; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

function pill() {
	return wrapper?.find('[data-status-toast]')
}

beforeEach(() => {
	hidden = false
	// A getter, because `document.hidden` is derived from the visibility state and
	// cannot be assigned. Restored by `configurable` on the next definition.
	Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden })

	// The pill's last rung is the panel root, which is the shell's element in the
	// app and has to exist here for focus to have anywhere to land.
	panel = document.createElement('div')
	panel.setAttribute('data-panel-root', '')
	panel.tabIndex = -1
	document.body.append(panel)

	// The real `Transition`, not the test utils stub: the pill's focus handoff
	// happens on `after-leave`, which is the one hook a stub that renders its slot
	// straight through never emits. What the stub would hide is exactly what these
	// cases are about.
	wrapper = mount(StatusLine, { attachTo: panel, global: { stubs: { transition: false } } })
})

afterEach(() => {
	status.clear()
	wrapper?.unmount()
	wrapper = null
	panel?.remove()
	panel = null
	document.body.innerHTML = ''
})

describe('the live region', () => {
	/** Injecting a region and its text together does not announce; only a text
	 *  change inside a region already in the tree does. */
	it('is in the DOM and empty before there is anything to say', () => {
		const region = wrapper!.get('[role="status"]')
		expect(region.text()).toBe('')
		expect(region.classes()).toContain('sr-only')
	})

	/**
	 * The button must not be inside the region. An atomic region wrapping a control
	 * re-reads the whole pill — `Undo` included — on every unrelated change, and
	 * puts a tab stop in the middle of what is meant to be an announcement.
	 */
	it('holds no controls, and announces beside the pill rather than around it', async () => {
		status.setMessage('Copied 3 notes', { label: 'Undo', run: () => {} })
		await wrapper!.vm.$nextTick()

		const region = wrapper!.get('[role="status"]')
		expect(region.text()).toBe('Copied 3 notes')
		expect(region.element.querySelector('button')).toBeNull()
		expect(wrapper!.get('[data-toast-action]').element.closest('[role="status"]')).toBeNull()
	})
})

describe('the clock', () => {
	beforeEach(() => {
		vi.useFakeTimers()
	})

	afterEach(() => {
		vi.useRealTimers()
	})

	it('retires the pill after five seconds', async () => {
		status.setMessage('Copied 1 note')
		await wrapper!.vm.$nextTick()
		expect(pill()!.exists()).toBe(true)

		vi.advanceTimersByTime(5000)
		await wrapper!.vm.$nextTick()
		expect(status.toast.value).toBeNull()
	})

	/**
	 * The five seconds are the reader's, not the wall's: reaching the button is the
	 * clearest evidence there is that the decision is still being made, so what is
	 * left of the window is banked rather than spent.
	 */
	it('banks the remainder while the pointer is on the button', async () => {
		status.setMessage('Moved 1 note to Done', { label: 'Undo', run: () => {} })
		await wrapper!.vm.$nextTick()

		vi.advanceTimersByTime(4000)
		await wrapper!.get('[data-toast-action]').trigger('pointerenter')

		vi.advanceTimersByTime(60_000)
		expect(status.toast.value).not.toBeNull()

		await wrapper!.get('[data-toast-action]').trigger('pointerleave')
		vi.advanceTimersByTime(900)
		expect(status.toast.value).not.toBeNull()

		vi.advanceTimersByTime(200)
		expect(status.toast.value).toBeNull()
	})

	/** Keyboard and pointer hold it separately, so arriving with one and leaving
	 *  with the other does not start the clock early. */
	it('keeps holding while focus is on the button after the pointer has left', async () => {
		status.setMessage('Moved 1 note to Done', { label: 'Undo', run: () => {} })
		await wrapper!.vm.$nextTick()

		const button = wrapper!.get('[data-toast-action]')
		await button.trigger('pointerenter')
		await button.trigger('focusin')
		await button.trigger('pointerleave')

		vi.advanceTimersByTime(10_000)
		expect(status.toast.value).not.toBeNull()

		await button.trigger('focusout')
		vi.advanceTimersByTime(5000)
		expect(status.toast.value).toBeNull()
	})

	/** Escape hides the panel to the tray, and an undo window that burned down in
	 *  a tray icon would be one the reader never got. */
	it('freezes while the window is hidden', async () => {
		status.setMessage('Deleted 1 note', { label: 'Undo', run: () => {} })
		await wrapper!.vm.$nextTick()

		vi.advanceTimersByTime(1000)
		hidden = true
		document.dispatchEvent(new Event('visibilitychange'))

		vi.advanceTimersByTime(60_000)
		expect(status.toast.value).not.toBeNull()

		hidden = false
		document.dispatchEvent(new Event('visibilitychange'))
		vi.advanceTimersByTime(3900)
		expect(status.toast.value).not.toBeNull()

		vi.advanceTimersByTime(200)
		expect(status.toast.value).toBeNull()
	})

	/** A hold belongs to the message it was taken on. The next message is a new
	 *  decision, and inherits neither the remainder nor the hold. */
	it('does not carry a hold across a replacement', async () => {
		status.setMessage('Copied 1 note', { label: 'Undo', run: () => {} })
		await wrapper!.vm.$nextTick()
		await wrapper!.get('[data-toast-action]').trigger('pointerenter')

		status.setMessage('Copied 3 notes')
		vi.advanceTimersByTime(5000)
		expect(status.toast.value).toBeNull()
	})
})

describe('a failure', () => {
	/** The list looks the same whether or not the action landed, so the pill is the
	 *  only place the difference is written down — and it waits to be read. */
	it('stands until it is dismissed rather than expiring', async () => {
		vi.useFakeTimers()
		try {
			status.setError("Couldn't write to the clipboard.")
			await wrapper!.vm.$nextTick()

			vi.advanceTimersByTime(60_000)
			await wrapper!.vm.$nextTick()
			expect(status.toast.value?.text).toBe("Couldn't write to the clipboard.")
			expect(wrapper!.get('[data-toast-action]').text()).toBe('Dismiss')
		} finally {
			vi.useRealTimers()
		}
	})

	it('is cleared by its own button', async () => {
		status.setError('Something went wrong.')
		await wrapper!.vm.$nextTick()

		await wrapper!.get('[data-toast-action]').trigger('click')
		expect(status.toast.value).toBeNull()
	})

	/** Errors never carry a caller's action: offering `Undo` beside a failure is
	 *  offering to reverse something that was never done. */
	it('refuses an action it was handed', async () => {
		status.setMessage('Nope.', { label: 'Undo', run: () => {} }, 'error')
		await wrapper!.vm.$nextTick()
		expect(wrapper!.get('[data-toast-action]').text()).toBe('Dismiss')
	})
})

describe('focus, when the pill goes', () => {
	/**
	 * `document.body` is an *ancestor* of the panel root, so focus falling there
	 * puts every chord and the whole Escape ladder out of reach until a mouse
	 * fixes it — and pressing the button removes the element that had focus.
	 */
	it('lands on the panel root after the button that had it is removed', async () => {
		status.setMessage('Moved 1 note to Done', { label: 'Undo', run: () => {} })
		await wrapper!.vm.$nextTick()

		const button = wrapper!.get('[data-toast-action]').element as HTMLElement
		button.focus()
		expect(document.activeElement).toBe(button)

		await wrapper!.get('[data-toast-action]').trigger('click')
		await settle()

		expect(pill()!.exists()).toBe(false)
		expect(document.activeElement).toBe(panel)
	})

	/** The same fall happens when the clock runs out with the reader standing on
	 *  the button, which is the case a press-only fix would miss. */
	it('lands there when the message expires instead', async () => {
		status.setMessage('Copied 1 note', { label: 'Undo', run: () => {} })
		await wrapper!.vm.$nextTick()
		;(wrapper!.get('[data-toast-action]').element as HTMLElement).focus()

		status.clear()
		await settle()

		expect(document.activeElement).toBe(panel)
	})

	/** Only from body, so a pill leaving while the reader is typing does not take
	 *  the caret out of the composer. */
	it('leaves focus alone when something else already has it', async () => {
		const elsewhere = document.createElement('input')
		panel!.append(elsewhere)
		status.setMessage('Copied 1 note', { label: 'Undo', run: () => {} })
		await wrapper!.vm.$nextTick()

		elsewhere.focus()
		status.clear()
		await settle()

		expect(document.activeElement).toBe(elsewhere)
	})
})
