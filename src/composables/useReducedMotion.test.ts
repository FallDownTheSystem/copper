import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import type { Settings } from './useSpace'
import { clearReducedMotion, setReducedMotion as setOsPreference } from '@/testing/matchMedia'

/**
 * Task-012 AC8, and the reason the composable ORs its two sources rather than
 * choosing between them: the setting must be able to *reduce* motion and never
 * to restore it. An OS `prefers-reduced-motion: reduce` is an accessibility
 * signal, and no value of an app setting is entitled to override it — so the
 * guarantee is expressed as an OR, where no value can subtract, rather than as a
 * rule someone has to remember not to break.
 */

const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
	emit: vi.fn(),
	listen: async () => () => {},
}))

function makeSettings(motion: string): Settings {
	return {
		recents: [],
		activeSpace: 0,
		panelPosition: null,
		shortcuts: {},
		theme: 'system',
		sounds: false,
		motion,
		insertionPoint: 'bottom',
		doubleClick: 'copy',
		enterKey: 'submit',
		doneOnCopy: false,
		alwaysOnTop: true,
		showCreated: false,
		captureNotifications: true,
		linkPreviews: false,
		translucent: false,
		neutral: 'warm',
		accent: 'copper',
		vibrancy: 1,
		resizable: false,
		panelWidth: 440,
		panelHeight: 760,
		doneFilter: 'all',
		sortMode: 'manual',
	}
}

/**
 * `os` is the Windows setting; `motion` is Copper's own. Returns what the
 * composable concludes once the settings pull has landed.
 */
async function resolve(os: boolean, motion: string) {
	setOsPreference(os)
	vi.resetModules()
	mocks.invoke.mockImplementation(async (command: string) => {
		if (command === 'get_settings') return makeSettings(motion)
		throw { kind: 'invalid', message: `no responder: ${command}` }
	})

	const settingsModule = await import('./useSettings')
	const reducedMotionModule = await import('./useReducedMotion')

	const settings = settingsModule.useSettings()
	const reduced = reducedMotionModule.useReducedMotion()
	// Only the settings pull, not `refresh()` — the shortcut and autostart pulls
	// are irrelevant here and would need responders of their own.
	await settings.refresh().catch(() => {})
	await new Promise((resolve) => setTimeout(resolve, 0))

	return reduced.value
}

beforeEach(() => {
	mocks.invoke.mockReset()
})

afterEach(() => {
	clearReducedMotion()
})

describe('useReducedMotion', () => {
	it('animates when neither the OS nor the setting objects', async () => {
		expect(await resolve(false, 'auto')).toBe(false)
	})

	it('is reduced when the setting says off', async () => {
		expect(await resolve(false, 'off')).toBe(true)
	})

	/** The load-bearing case. `auto` is the default and the only non-`off` value
	 *  there is, so this is the whole proof that no setting animates against the
	 *  OS preference. */
	it('stays reduced against the OS preference whatever the setting says', async () => {
		expect(await resolve(true, 'auto')).toBe(true)
		expect(await resolve(true, 'off')).toBe(true)
	})

	/** A `settings.json` from before this task has no `motion` key at all, and an
	 *  unreadable one leaves `settings` null. Neither may read as "reduce". */
	it('treats an absent or unparsed preference as auto', async () => {
		expect(await resolve(false, 'nonsense')).toBe(false)
		expect(await resolve(true, 'nonsense')).toBe(true)
	})
})
