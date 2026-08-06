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
