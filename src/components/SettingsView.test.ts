import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import SettingsView from './SettingsView.vue'
import type { Settings } from '@/composables/useSpace'

/**
 * The two rows task-012 adds. The `sounds` row is an ordinary boolean; the
 * `motion` row is the one worth a test, because the control and the stored value
 * do not have the same shape — the switch is "animate", the setting is
 * `auto | off`, and an inverted mapping would look completely plausible in
 * review.
 */

const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
	emit: vi.fn(),
	listen: async () => () => {},
}))

function makeSettings(over: Partial<Settings> = {}): Settings {
	return {
		recents: ['C:\\notes.copper'],
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
		...over,
	}
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

async function flush(times = 4) {
	for (let i = 0; i < times; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

async function openSettings(stored: Partial<Settings> = {}) {
	// No module reset here, unlike the suites that use one: `SettingsView` and the
	// composables behind it are static imports, and `vi.resetModules()` cannot
	// re-evaluate a module that has already been imported. What separates the cases
	// is that every mount pulls `get_settings` again and overwrites the
	// module-scoped value.
	//
	// Cleared per mount, not per test: a case that opens the view twice is
	// comparing what the *second* one wrote.
	mocks.invoke.mockClear()
	mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
		switch (command) {
			case 'get_settings':
				return makeSettings(stored)
			case 'get_shortcut_state':
				return SHORTCUTS
			case 'get_autostart_enabled':
				return false
			case 'get_app_version':
				return '0.1.0'
			case 'update_settings':
				return makeSettings({ ...stored, ...(args?.patch as Partial<Settings>) })
			// Its own command rather than a patch, because it has a native side to
			// apply — so the responder has to answer with the whole settings object
			// the same way `set_theme_preference` does.
			case 'set_always_on_top':
				return makeSettings({ ...stored, alwaysOnTop: args?.enabled as boolean })
			default:
				throw { kind: 'invalid', message: `no responder: ${command}` }
		}
	})

	const wrapper = mount(SettingsView, { attachTo: document.body })
	await flush()
	return wrapper
}

function patchesSent() {
	return mocks.invoke.mock.calls
		.filter((call) => call[0] === 'update_settings')
		.map((call) => (call[1] as { patch: unknown }).patch)
}

beforeEach(() => {
	mocks.invoke.mockReset()
})

describe('the sound and motion rows', () => {
	it('renders both as switches the keyboard can reach and name', async () => {
		const wrapper = await openSettings()

		for (const id of ['sounds', 'motion']) {
			const control = wrapper.get(`#${id}`)
			// reka renders a real `<button role="switch">`, which Space and Enter both
			// operate — the same control task-008 shipped for autostart.
			expect(control.element.tagName).toBe('BUTTON')
			expect(control.attributes('role')).toBe('switch')
			expect(control.attributes('aria-checked')).toBeDefined()
			// A switch has no text of its own, so the row's visible label is what
			// names it.
			expect(wrapper.find(`label[for="${id}"]`).exists()).toBe(true)
		}
	})

	it('shows sound off by default and turns it on with a patch of one key', async () => {
		const wrapper = await openSettings()
		expect(wrapper.get('#sounds').attributes('aria-checked')).toBe('false')

		await wrapper.get('#sounds').trigger('click')
		await flush()

		expect(patchesSent()).toEqual([{ sounds: true }])
	})

	/** `auto` is the animating value, so the switch reads *on* — the row that
	 *  would be easiest to wire backwards. */
	it('reads the animate switch as on for auto and off for off', async () => {
		const auto = await openSettings({ motion: 'auto' })
		expect(auto.get('#motion').attributes('aria-checked')).toBe('true')

		const off = await openSettings({ motion: 'off' })
		expect(off.get('#motion').attributes('aria-checked')).toBe('false')
	})

	it('stores off when animation is switched off, and auto when it is switched back', async () => {
		const on = await openSettings({ motion: 'auto' })
		await on.get('#motion').trigger('click')
		await flush()
		expect(patchesSent()).toEqual([{ motion: 'off' }])

		const off = await openSettings({ motion: 'off' })
		await off.get('#motion').trigger('click')
		await flush()
		expect(patchesSent()).toEqual([{ motion: 'auto' }])
	})

	/** Each key is written on its own, so turning sound on cannot silently rewrite
	 *  a motion preference read at mount — the failure mode a whole-object save
	 *  would have. */
	it('never writes the two keys together', async () => {
		const wrapper = await openSettings({ motion: 'off' })

		await wrapper.get('#sounds').trigger('click')
		await flush()

		expect(patchesSent()).toEqual([{ sounds: true }])
	})
})

/**
 * Task-013's two rows. Both are two-value *choices* rather than booleans, which
 * is why they are radio groups and not switches — and the ARIA is the whole
 * point of that decision, so it is what this asserts.
 */
describe('the always-on-top row', () => {
	it('renders on by default and turns off through its own Rust command', async () => {
		const wrapper = await openSettings()

		const control = wrapper.get('#always-on-top')
		expect(control.attributes('role')).toBe('switch')
		expect(control.attributes('aria-checked')).toBe('true')
		expect(wrapper.find('label[for="always-on-top"]').exists()).toBe(true)

		await control.trigger('click')
		await flush()

		// Not `update_settings`. Unlike `sounds` and `motion` this preference has a
		// native side — the window's z-order band — so it goes through the command
		// that applies it before persisting it, exactly as the theme does.
		expect(patchesSent()).toEqual([])
		expect(mocks.invoke).toHaveBeenCalledWith('set_always_on_top', { enabled: false })
		expect(wrapper.get('#always-on-top').attributes('aria-checked')).toBe('false')
	})

	it('reads a stored false as off', async () => {
		const wrapper = await openSettings({ alwaysOnTop: false })

		expect(wrapper.get('#always-on-top').attributes('aria-checked')).toBe('false')
	})

	/** The row's own error slot, not the panel's band: a failure here has to
	 *  render next to the control that produced it. */
	it('reports a refused write on its own row and leaves the switch where it was', async () => {
		const wrapper = await openSettings()
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'set_always_on_top') {
				throw { kind: 'persist', message: "Copper couldn't save the always-on-top setting" }
			}
			throw { kind: 'invalid', message: `no responder: ${command}` }
		})

		await wrapper.get('#always-on-top').trigger('click')
		await flush()

		expect(wrapper.text()).toContain("Copper couldn't save the always-on-top setting")
		expect(wrapper.get('#always-on-top').attributes('aria-checked')).toBe('true')
	})
})

describe('the notes rows', () => {
	function group(wrapper: ReturnType<typeof mount>, label: string) {
		return wrapper.get(`[role="radiogroup"][aria-label="${label}"]`)
	}

	function segment(wrapper: ReturnType<typeof mount>, label: string, text: string) {
		const found = group(wrapper, label)
			.findAll('[role="radio"]')
			.find((item) => item.text() === text)
		if (!found) throw new Error(`no ${text} segment in ${label}`)
		return found
	}

	it('renders each as one radiogroup rather than a row of independent toggles', async () => {
		const wrapper = await openSettings()

		for (const label of ['Where new notes go', 'What double-clicking a note does']) {
			const radios = group(wrapper, label).findAll('[role="radio"]')
			expect(radios).toHaveLength(2)
			// `aria-pressed` here would mean two independent toggle buttons, which is
			// exactly what `ToggleGroup` would have produced and why it was declined.
			for (const radio of radios) expect(radio.attributes('aria-checked')).toBeDefined()
		}
	})

	it('shows the shipped defaults, which are what every earlier build did', async () => {
		const wrapper = await openSettings()

		expect(segment(wrapper, 'Where new notes go', 'Bottom').attributes('aria-checked')).toBe('true')
		expect(
			segment(wrapper, 'What double-clicking a note does', 'Copy').attributes('aria-checked'),
		).toBe('true')
	})

	it('reflects a stored top and edit', async () => {
		const wrapper = await openSettings({ insertionPoint: 'top', doubleClick: 'edit' })

		expect(segment(wrapper, 'Where new notes go', 'Top').attributes('aria-checked')).toBe('true')
		expect(
			segment(wrapper, 'What double-clicking a note does', 'Edit').attributes('aria-checked'),
		).toBe('true')
	})

	it('writes one key per choice, so neither row can clear the other', async () => {
		const wrapper = await openSettings()

		await segment(wrapper, 'Where new notes go', 'Top').trigger('click')
		await flush()
		expect(patchesSent()).toEqual([{ insertionPoint: 'top' }])

		await segment(wrapper, 'What double-clicking a note does', 'Edit').trigger('click')
		await flush()
		expect(patchesSent()).toEqual([{ insertionPoint: 'top' }, { doubleClick: 'edit' }])
	})

	/** A hand-edited value nothing recognises collapses to the default on read
	 *  rather than leaving the control showing nothing at all. */
	it('falls back to the default for a name it does not recognise', async () => {
		const wrapper = await openSettings({ insertionPoint: 'sideways', doubleClick: 'launch' })

		expect(segment(wrapper, 'Where new notes go', 'Bottom').attributes('aria-checked')).toBe('true')
		expect(
			segment(wrapper, 'What double-clicking a note does', 'Copy').attributes('aria-checked'),
		).toBe('true')
	})
})

/**
 * Task-016's one settings key. It is a *display* switch and nothing else: the
 * `created` it reveals has been recorded on every note since task-003, so
 * turning it on shows history that already exists rather than starting to
 * collect any.
 */
describe('the date-added switch', () => {
	it('ships off, so an upgrade shows the cards it showed before', async () => {
		const wrapper = await openSettings()
		expect(wrapper.get('#show-created').attributes('aria-checked')).toBe('false')
	})

	it('reflects a stored true', async () => {
		const wrapper = await openSettings({ showCreated: true })
		expect(wrapper.get('#show-created').attributes('aria-checked')).toBe('true')
	})

	/** One key wide, like every other row here, so writing this one cannot clear
	 *  the preference beside it. */
	it('writes only its own key', async () => {
		const wrapper = await openSettings()

		await wrapper.get('#show-created').trigger('click')
		await flush()

		expect(patchesSent()).toEqual([{ showCreated: true }])
	})
})
