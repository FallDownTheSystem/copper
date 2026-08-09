import { mount } from '@vue/test-utils'
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import App from './App.vue'
import { useSettings } from '@/composables/useSettings'
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

// happy-dom implements no Web Animations API; auto-animate calls `el.animate`
// from a MutationObserver callback and throws out of band without this.
//
// Torn down again below: `restoreMocks` does not reach a plain assignment to a
// host prototype, so a stub left in place would hand every later suite in the
// worker a fake WAAPI they never asked for.
const elementPrototype = Element.prototype as unknown as Record<string, unknown>
const stubbedAnimate = elementPrototype.animate === undefined
if (stubbedAnimate) {
	elementPrototype.animate = () => ({
		playState: 'finished',
		finished: Promise.resolve(),
		cancel: () => {},
		addEventListener: (name: string, handler: () => void) => {
			if (name === 'finish') queueMicrotask(handler)
		},
		removeEventListener: () => {},
	})
}

afterAll(() => {
	if (stubbedAnimate) Reflect.deleteProperty(elementPrototype, 'animate')
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
	summonFallback: null,
}

/** The handlers `listen` was given, so a Rust-originated event can be delivered. */
const listeners = new Map<string, (event: { payload: unknown }) => void>()

/**
 * The whole startup responder, with the two tables a case here varies.
 *
 * Overridden rather than replaced, and that distinction is the point: a case
 * that rewrote `mockImplementation` wholesale silently dropped
 * `get_active_space` along with everything else it did not name, and then
 * asserted against a panel whose document load had failed.
 */
function respond(
	overrides: {
		settings?: Record<string, unknown>
		shortcuts?: Record<string, unknown>
	} = {},
) {
	const settings = overrides.settings ?? SETTINGS
	const shortcuts = overrides.shortcuts ?? SHORTCUTS
	mocks.invoke.mockImplementation(async (command: string) => {
		if (command === 'get_settings') return settings
		if (command === 'get_shortcut_state') return shortcuts
		if (command === 'get_autostart_enabled') return false
		// Task-026. `useDeviceShare` is initialised from App.vue, so the whole app
		// pulls this on mount whether or not Settings is ever opened.
		if (command === 'get_share_config') return SHARE_CONFIG
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
}

beforeEach(() => {
	listeners.clear()
	mocks.listen.mockReset()
	mocks.listen.mockImplementation(async (name: string, handler: (event: never) => void) => {
		listeners.set(name, handler as (event: { payload: unknown }) => void)
		return () => {}
	})
	mocks.emit.mockReset()
	mocks.invoke.mockReset()
	respond()
	useView().showList()
})

let app: ReturnType<typeof mount> | null = null

afterEach(() => {
	app?.unmount()
	app = null
	useView().showList()
	// `initialize()` memoises its promise at module scope and `App.vue` is its only
	// caller, so without this only the *first* mount in the file ever pulls — every
	// later case would mount against whatever settings the previous one left behind.
	useSettings().dispose()
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

/** Mounted with the settings view open and settled, which is where most of the
 *  cases below start. */
async function mountSettings() {
	const wrapper = await mountApp()
	useView().showSettings()
	await settle()
	return wrapper
}

/**
 * The transitioning view's inline transform, sampled across the whole swap.
 *
 * Sampled rather than read once because `motion-v` drives this from JavaScript
 * here — happy-dom has no Web Animations API — so the number moves between turns
 * and only the transform's *shape* is stable: a transition with no shift emits
 * `transform: none` at every frame, and one with a shift emits a `translateX`
 * from the first frame to the last.
 */
async function shiftDuringSwap(wrapper: ReturnType<typeof mount>) {
	useView().showSettings()
	const samples: string[] = []
	for (let i = 0; i < 8; i++) {
		await settle(1)
		// The outermost inline-styled element is the `motion.div` wrapping the view;
		// neither view's own root carries a style attribute.
		samples.push(wrapper.element.querySelector('[style*="transform"]')?.getAttribute('style') ?? '')
	}
	return samples
}

describe('the view transition', () => {
	/**
	 * This is the one animation a user is watching at the moment they toggle
	 * "Animate controls", so it has to obey that setting and not only the OS —
	 * which is why `App.vue` goes through Copper's own `useReducedMotion` rather
	 * than reading `usePreferredReducedMotion` directly, as it first did.
	 *
	 * Reduce, not remove: the translate goes and the cross-fade stays, so what is
	 * asserted is the absence of a horizontal shift rather than the absence of a
	 * transition.
	 */
	it('drops the slide when the motion setting is off', async () => {
		respond({ settings: { ...SETTINGS, motion: 'off' } })
		const wrapper = await mountApp()

		expect(
			(await shiftDuringSwap(wrapper)).filter((style) => style.includes('translateX')),
		).toEqual([])
	})

	/** The other half, and what makes the case above discriminating rather than an
	 *  assertion that passes on a view that never moved in the first place. */
	it('slides when nothing has asked it not to', async () => {
		const wrapper = await mountApp()

		expect((await shiftDuringSwap(wrapper)).some((style) => style.includes('translateX'))).toBe(
			true,
		)
	})
})

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
		const wrapper = await mountSettings()

		for (const heading of ['Theme', 'Shortcuts', 'Behavior']) {
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
		const wrapper = await mountSettings()

		const group = wrapper.find('[role="radiogroup"]')
		expect(group.exists()).toBe(true)
		expect(group.findAll('[role="radio"]')).toHaveLength(3)
		expect(wrapper.findAll('[aria-pressed]')).toHaveLength(0)
	})

	it('gives the autostart switch the row label as its accessible name', async () => {
		const wrapper = await mountSettings()

		// Named rather than taken as the first switch on the surface: task-012 added
		// two more above it, and "the first one" silently became a different row.
		const control = wrapper.find('#autostart')
		expect(control.exists()).toBe(true)
		expect(control.attributes('role')).toBe('switch')
		expect(wrapper.find('label[for="autostart"]').text()).toBe('Launch Copper at login')
	})

	it('shows a startup registration failure against the summon row', async () => {
		// Pulled, never pushed: `setup()` runs before the webview exists, so there is
		// no event this could have been told by.
		respond({
			shortcuts: {
				...SHORTCUTS,
				summonRegistered: false,
				summonError: "Windows wouldn't accept it",
			},
		})

		const wrapper = await mountSettings()

		expect(wrapper.text()).toContain("Windows wouldn't accept it")
	})

	it('returns to the list on Escape', async () => {
		const wrapper = await mountSettings()

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
		const wrapper = await mountSettings()

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
		const wrapper = await mountSettings()
		useView().showList()
		await settle()

		expect(document.activeElement).not.toBe(document.body)
		expect(wrapper.element.contains(document.activeElement)).toBe(true)
	})
})
