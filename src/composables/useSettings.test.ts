import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import { deferred } from '@/testing/deferred'

/**
 * The reordering rules, which are the only thing in this module that cannot be
 * read off the source.
 *
 * Every setter here is fire-and-forget from a switch or a button, so two of them
 * are in flight together the moment a user clicks twice — and Tauri makes no
 * promise about the order replies cross the boundary. There are two separate
 * guards and they partition the module differently: one per **ref a setter
 * writes**, one per **row a message lands under**. Conflating them is what these
 * cases exist to catch.
 */
const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }))

const SETTINGS = {
	recents: [],
	activeSpace: 0,
	panelPosition: null,
	shortcuts: { capture: 'Shift Shift', summon: 'Ctrl+Shift+Space' },
	theme: 'system',
	sounds: false,
	motion: 'auto',
	insertionPoint: 'bottom',
	doubleClick: 'copy',
	enterKey: 'submit',
	alwaysOnTop: true,
	showCreated: false,
	captureNotifications: true,
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

/** One module graph per case: the generations, the settings copy and the error
 *  map are all module-scoped by design. */
async function freshModule() {
	vi.resetModules()
	const module = await import('./useSettings')
	return module.useSettings()
}

/** The two preference writes, held open so their order can be chosen. Both go
 *  through `update_settings`, so they are told apart by their patch — which is
 *  also what makes them share one value generation and not one row. */
function heldPreferences() {
	const sounds = deferred<unknown>()
	const motion = deferred<unknown>()

	mocks.invoke.mockImplementation(async (command, args) => {
		const patch = (args as { patch?: Record<string, unknown> } | undefined)?.patch
		if (command === 'update_settings' && patch && 'sounds' in patch) return sounds.promise
		if (command === 'update_settings' && patch && 'motion' in patch) return motion.promise
		throw new Error(`unexpected command in this case: ${command}`)
	})

	return { sounds, motion }
}

beforeEach(() => {
	mocks.invoke.mockReset()
})

describe('two preference writes in flight together', () => {
	/**
	 * The failure half is guarded per **row**, not per ref.
	 *
	 * `sounds` and `motion` both write `settings.value`, so they share a value
	 * generation — but they are two rows with two error slots, and a motion write
	 * that succeeded says nothing whatsoever about whether the sounds write did.
	 * Guarding the failure with the shared counter left the sounds row silent
	 * while its setting had not in fact changed, which is worse than the reorder
	 * the counter was added to prevent.
	 */
	it('keeps a failure on its own row when a different row settled first', async () => {
		const settings = await freshModule()
		const { sounds, motion } = heldPreferences()

		const soundsCall = settings.setSounds(true)
		const motionCall = settings.setMotion('off')

		motion.resolve({ ...SETTINGS, motion: 'off' })
		await motionCall
		sounds.reject({ kind: 'io', message: 'the settings file is read-only' })
		await soundsCall

		expect(settings.errorFor('sounds').value).toBe('the settings file is read-only')
		expect(settings.errorFor('motion').value).toBeNull()
		expect(settings.motionPreference.value).toBe('off')
	})

	/**
	 * The value half compares against what has been **applied**, not against what
	 * has been issued.
	 *
	 * A newer write that rejects applies nothing, so it has no claim on the ref —
	 * and the older success is then the only answer there is. Discarding it on the
	 * newer write's behalf loses a value that reached `settings.json`, and nothing
	 * corrects it: the store emits no `settings-changed` for a write the frontend
	 * itself made, so the panel and the file disagree until something else happens
	 * to re-read.
	 */
	it('applies an older success after a newer write has rejected', async () => {
		const settings = await freshModule()
		const { sounds, motion } = heldPreferences()

		const soundsCall = settings.setSounds(true)
		const motionCall = settings.setMotion('off')

		motion.reject({ kind: 'io', message: 'the settings file is read-only' })
		await motionCall
		sounds.resolve({ ...SETTINGS, sounds: true })
		await soundsCall

		expect(settings.soundsEnabled.value).toBe(true)
		expect(settings.errorFor('motion').value).toBe('the settings file is read-only')
	})

	/** The guarantee the counter was added for, still holding: once a newer answer
	 *  has actually been applied, an older one is dropped rather than written over
	 *  it. Without this the two cases above could both pass with no guard at all. */
	it('still drops an older answer once a newer one has been applied', async () => {
		const settings = await freshModule()
		const { sounds, motion } = heldPreferences()

		const soundsCall = settings.setSounds(true)
		const motionCall = settings.setMotion('off')

		motion.resolve({ ...SETTINGS, motion: 'off', sounds: false })
		await motionCall
		// Taken before the motion write landed, so its `motion: 'auto'` is stale.
		sounds.resolve({ ...SETTINGS, sounds: true, motion: 'auto' })
		await soundsCall

		expect(settings.motionPreference.value).toBe('off')
	})
})

describe('the two shortcut rows', () => {
	/** The same partition, one layer over: both Reset buttons write
	 *  `shortcuts.value` and share its generation, and each has its own row. */
	it('keeps a summon failure visible when a capture rebind succeeds first', async () => {
		const settings = await freshModule()

		const summon = deferred<unknown>()
		const capture = deferred<unknown>()
		mocks.invoke.mockImplementation(async (command) => {
			if (command === 'get_shortcut_state') return SHORTCUTS
			if (command === 'get_settings') return SETTINGS
			if (command === 'get_autostart_enabled') return false
			if (command === 'set_summon_shortcut') return summon.promise
			if (command === 'set_capture_trigger') return capture.promise
			throw new Error(`unexpected command in this case: ${command}`)
		})

		// `resetShortcut` reads the shipped defaults off the pulled state.
		await settings.refresh()

		const summonCall = settings.resetShortcut('summon')
		const captureCall = settings.resetShortcut('capture')

		capture.resolve({ ...SHORTCUTS, capture: 'Shift Shift' })
		await captureCall
		summon.reject({ kind: 'invalid', message: "Windows wouldn't accept it" })
		await summonCall

		expect(settings.errorFor('summon').value).toBe("Windows wouldn't accept it")
		expect(settings.errorFor('capture').value).toBeNull()
	})
})
