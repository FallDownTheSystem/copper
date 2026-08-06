import { mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import App from './App.vue'
import { useView } from '@/composables/useView'

/**
 * The view switch, which nothing else covers.
 *
 * `<AnimatePresence>` with `mode="wait"` only mounts the incoming view once the
 * outgoing one has finished leaving, so a mistake in that wiring does not throw —
 * it leaves a blank panel. That is exactly the kind of failure a build passes and
 * a person discovers by launching the app.
 */
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), emit: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen, emit: mocks.emit }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))

// happy-dom implements no Web Animations API; auto-animate calls `el.animate`
// from a MutationObserver callback and throws out of band without this.
const elementPrototype = Element.prototype as unknown as Record<string, unknown>
elementPrototype.animate ??= () => ({
	playState: 'finished',
	finished: Promise.resolve(),
	cancel: () => {},
	addEventListener: (name: string, handler: () => void) => {
		if (name === 'finish') queueMicrotask(handler)
	},
	removeEventListener: () => {},
})

const SETTINGS = {
	recents: [],
	activeSpace: 0,
	panelPosition: null,
	shortcuts: { capture: 'Shift Shift', summon: 'Ctrl+Shift+Space' },
	theme: 'system',
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

/** The handlers `listen` was given, so a Rust-originated event can be delivered. */
const listeners = new Map<string, (event: { payload: unknown }) => void>()

beforeEach(() => {
	listeners.clear()
	mocks.listen.mockReset()
	mocks.listen.mockImplementation(async (name: string, handler: (event: never) => void) => {
		listeners.set(name, handler as (event: { payload: unknown }) => void)
		return () => {}
	})
	mocks.emit.mockReset()
	mocks.invoke.mockReset()
	mocks.invoke.mockImplementation(async (command: string) => {
		if (command === 'get_settings') return SETTINGS
		if (command === 'get_shortcut_state') return SHORTCUTS
		if (command === 'get_autostart_enabled') return false
		if (command === 'get_status') {
			return {
				path: 'C:\\notes.copper',
				errored: false,
				watching: true,
				canUndo: false,
				canRedo: false,
				startupNotice: null,
			}
		}
		if (command === 'get_active_space') {
			return {
				id: 'spc_1',
				name: 'development',
				activeSection: 'sec_a',
				sections: [{ id: 'sec_a', name: 'Research', order: 0 }],
				notes: [],
			}
		}
		if (command === 'editor_handoffs') return []
		if (command === 'list_recents') return []
		return null
	})
	useView().showList()
})

let app: ReturnType<typeof mount> | null = null

afterEach(() => {
	app?.unmount()
	app = null
	useView().showList()
	document.body.innerHTML = ''
})

async function settle(turns = 8) {
	for (let i = 0; i < turns; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

async function mountApp() {
	app = mount(App, { attachTo: document.body })
	await settle()
	return app
}

describe('the view switch', () => {
	it('shows the list first', async () => {
		const wrapper = await mountApp()

		expect(wrapper.find('#panel-search').exists()).toBe(true)
		expect(wrapper.text()).not.toContain('Launch Copper at login')
	})

	it('swaps to settings and back without losing either view', async () => {
		const wrapper = await mountApp()

		useView().showSettings()
		await settle()

		expect(wrapper.find('[aria-label="Back to notes"]').exists()).toBe(true)
		expect(wrapper.text()).toContain('Launch Copper at login')
		// The list is gone, not merely hidden — `AnimatePresence` unmounts it, which
		// is the whole reason its composables hold their state at module scope.
		expect(wrapper.find('#panel-search').exists()).toBe(false)

		useView().showList()
		await settle()

		expect(wrapper.find('#panel-search').exists()).toBe(true)
	})

	it('opens settings when the tray asks', async () => {
		// The tray's Settings item reveals the panel and switches the view in one
		// action, and the event half is the only half the frontend owns.
		const wrapper = await mountApp()

		listeners.get('open-settings')?.({ payload: null })
		await settle()

		expect(wrapper.text()).toContain('Launch Copper at login')
	})
})

describe('the settings view', () => {
	it('renders every group and pulls their state on open', async () => {
		const wrapper = await mountApp()
		useView().showSettings()
		await settle()

		for (const heading of ['Theme', 'Shortcuts', 'Startup']) {
			expect(wrapper.text()).toContain(heading)
		}
		// Re-read on open, not only at startup: a summon chord that failed during
		// `setup()` is state with no event behind it, and autostart can be switched
		// off in Task Manager while Copper is running.
		expect(mocks.invoke).toHaveBeenCalledWith('get_shortcut_state')
		expect(mocks.invoke).toHaveBeenCalledWith('get_autostart_enabled')
	})

	it('renders the theme choice as one radio group rather than three toggles', async () => {
		// reka's ToggleGroup announces three independent pressed buttons even in
		// single-select mode. This is the assertion that keeps it a RadioGroup.
		const wrapper = await mountApp()
		useView().showSettings()
		await settle()

		const group = wrapper.find('[role="radiogroup"]')
		expect(group.exists()).toBe(true)
		expect(group.findAll('[role="radio"]')).toHaveLength(3)
		expect(wrapper.findAll('[aria-pressed]')).toHaveLength(0)
	})

	it('gives the autostart switch the row label as its accessible name', async () => {
		const wrapper = await mountApp()
		useView().showSettings()
		await settle()

		const control = wrapper.find('[role="switch"]')
		expect(control.exists()).toBe(true)
		const id = control.attributes('id')
		expect(id).toBeTruthy()
		expect(wrapper.find(`label[for="${id}"]`).text()).toBe('Launch Copper at login')
	})

	it('shows a startup registration failure against the summon row', async () => {
		// Pulled, never pushed: `setup()` runs before the webview exists, so there is
		// no event this could have been told by.
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'get_shortcut_state') {
				return { ...SHORTCUTS, summonRegistered: false, summonError: "Windows wouldn't accept it" }
			}
			if (command === 'get_settings') return SETTINGS
			if (command === 'get_autostart_enabled') return false
			return null
		})

		const wrapper = await mountApp()
		useView().showSettings()
		await settle()

		expect(wrapper.text()).toContain("Windows wouldn't accept it")
	})

	it('returns to the list on Escape', async () => {
		const wrapper = await mountApp()
		useView().showSettings()
		await settle()

		await wrapper.find('[aria-label="Back to notes"]').trigger('keydown', { key: 'Escape' })
		await settle()

		expect(wrapper.find('#panel-search').exists()).toBe(true)
	})

	/**
	 * Escape is bound to the view's root, and `document.body` is an *ancestor* of
	 * that root rather than a descendant — so a press with focus left on the body
	 * never reaches the handler at all. Both entry paths leave it there: the `...`
	 * menu's trigger is unmounted as the list leaves, and the tray's event moves
	 * nothing. Without this the view opened unusable by keyboard, and the earlier
	 * Escape test only passed because it dispatched from the Back button.
	 */
	it('moves focus into the view on open, so Escape works on arrival', async () => {
		const wrapper = await mountApp()
		useView().showSettings()
		await settle()

		const back = wrapper.find('[aria-label="Back to notes"]')
		expect(document.activeElement).toBe(back.element)

		// Dispatched where focus actually is, rather than at a chosen element.
		document.activeElement?.dispatchEvent(
			new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }),
		)
		await settle()

		expect(wrapper.find('#panel-search').exists()).toBe(true)
	})

	it('leaves focus somewhere real on the way back to the list', async () => {
		// The return path has the same problem in reverse: `AnimatePresence`
		// remounts the whole list tree, and a panel whose focus sits on the body has
		// no working Escape ladder and no working in-panel chords.
		const wrapper = await mountApp()
		useView().showSettings()
		await settle()
		useView().showList()
		await settle()

		expect(document.activeElement).not.toBe(document.body)
		expect(wrapper.element.contains(document.activeElement)).toBe(true)
	})
})
