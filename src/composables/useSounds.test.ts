import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import type { Settings, Space, StoreStatus } from './useSpace'

/**
 * The engine is mocked here — `engine.test.ts` covers the real one. What this
 * file is for is the two things that live outside it: that the `sounds` setting
 * reaches `setEnabled` at the right moments, and that each of the seven sound
 * points is actually *wired* to the interaction it claims. The wiring is the
 * half that rots silently: a refactor that moves a funnel takes the sound with
 * it and nothing else notices.
 */
const engine = vi.hoisted(() => ({ play: vi.fn(), setEnabled: vi.fn() }))
vi.mock('@/lib/sounds', () => engine)

const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
	handlers: new Map<string, (event: { payload: unknown }) => void>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
	emit: vi.fn(),
	listen: async (name: string, handler: (event: { payload: unknown }) => void) => {
		await Promise.resolve()
		mocks.handlers.set(name, handler)
		return () => mocks.handlers.delete(name)
	},
}))

const STATUS: StoreStatus = {
	path: 'C:\\notes.copper',
	errored: false,
	watching: true,
	canUndo: false,
	canRedo: false,
	startupNotice: null,
}

function makeSettings(sounds: boolean): Settings {
	return {
		recents: ['C:\\notes.copper'],
		activeSpace: 0,
		panelPosition: null,
		shortcuts: {},
		theme: 'system',
		sounds,
		motion: 'auto',
		insertionPoint: 'bottom',
		doubleClick: 'copy',
		alwaysOnTop: true,
		showCreated: false,
		captureNotifications: true,
		linkPreviews: false,
	}
}

function makeSpace(): Space {
	return {
		id: 'spc_1',
		name: 'development',
		activeSection: 'sec_a',
		sections: [{ id: 'sec_a', name: 'Research', order: 0 }],
		notes: [
			{
				id: 'n1',
				section: 'sec_a',
				order: 0,
				done: false,
				body: 'n1',
				created: '2026-08-05T00:00:00Z',
				updated: '2026-08-05T00:00:00Z',
			},
		],
	}
}

type Responder = (args?: Record<string, unknown>) => unknown
const responders = new Map<string, Responder>()

function respond(command: string, responder: Responder) {
	responders.set(command, responder)
}

function emit(name: string, payload: unknown) {
	mocks.handlers.get(name)?.({ payload })
}

async function flush(times = 4) {
	for (let i = 0; i < times; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

/** One module graph per case: `useSounds`, `useSettings` and `useSpace` all hold
 *  module-scoped state, and they have to be the same instances the composables
 *  under test reach for. */
async function freshModules() {
	vi.resetModules()
	const [sounds, settings, space, attachments, captureNotice] = await Promise.all([
		import('./useSounds'),
		import('./useSettings'),
		import('./useSpace'),
		import('./useAttachments'),
		import('./useCaptureNotice'),
	])
	return {
		sounds: sounds.useSounds(),
		settings: settings.useSettings(),
		space: space.useSpace(),
		attachments: attachments.useAttachments(),
		captureNotice: captureNotice.useCaptureNotice(),
	}
}

beforeEach(() => {
	engine.play.mockReset()
	engine.setEnabled.mockReset()
	mocks.invoke.mockReset()
	mocks.handlers.clear()
	responders.clear()

	mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
		const responder = responders.get(command)
		if (!responder) throw { kind: 'invalid', message: `no responder: ${command}` }
		return responder(args)
	})

	respond('get_status', () => ({ ...STATUS }))
	respond('get_settings', () => makeSettings(false))
	respond('get_active_space', () => makeSpace())
	respond('submit_entry', () => ({
		space: makeSpace(),
		outcome: 'note',
		noteId: 'n1',
		sectionId: 'sec_a',
	}))
	respond('get_shortcut_state', () => ({
		capture: 'Shift Shift',
		summon: 'Ctrl+Shift+Space',
		defaults: { capture: 'Shift Shift', summon: 'Ctrl+Shift+Space' },
		summonRegistered: true,
		summonError: null,
		captureRegistered: true,
		captureError: null,
		captureFallback: null,
	}))
	respond('get_autostart_enabled', () => false)
})

describe('the sound setting', () => {
	/**
	 * Task-012 AC9 at the composable level. `settings` is null until the startup
	 * pull returns, and the engine ships enabled, so silence in that window is not
	 * the default — it is this watcher running immediately and ungated.
	 */
	it('disables the engine before any settings have arrived', async () => {
		await freshModules()

		expect(engine.setEnabled).toHaveBeenCalledWith(false)
	})

	it('enables the engine once the file says so', async () => {
		respond('get_settings', () => makeSettings(true))
		const { settings } = await freshModules()

		await settings.refresh()
		await flush()

		expect(engine.setEnabled).toHaveBeenLastCalledWith(true)
	})

	/** Task-012 AC12: no reload, and nothing re-mounted — the setter applies its
	 *  own return value and the watcher does the rest. */
	it('stops sound the moment the setting is turned off mid-session', async () => {
		respond('get_settings', () => makeSettings(true))
		const { settings } = await freshModules()
		await settings.refresh()
		await flush()
		expect(engine.setEnabled).toHaveBeenLastCalledWith(true)

		respond('update_settings', () => makeSettings(false))
		await settings.setSounds(false)
		await flush()

		expect(engine.setEnabled).toHaveBeenLastCalledWith(false)
	})

	it('sends the sounds key on its own, so writing it cannot clear motion', async () => {
		respond('update_settings', () => makeSettings(true))
		const { settings } = await freshModules()

		await settings.setSounds(true)

		const call = mocks.invoke.mock.calls.find((entry) => entry[0] === 'update_settings')
		expect(call?.[1]).toEqual({ patch: { sounds: true } })
	})
})

describe('the seven sound points', () => {
	it('sounds a note toggle once, whatever the size of the selection', async () => {
		const { space } = await freshModules()
		await space.initialize()
		await flush()
		engine.play.mockClear()

		respond('set_notes_done', () => makeSpace())
		await space.setNotesDone(['n1', 'n2', 'n3'], true)

		expect(engine.play).toHaveBeenCalledTimes(1)
		expect(engine.play).toHaveBeenCalledWith('toggle')
	})

	it('stays silent when a toggle fails', async () => {
		const { space } = await freshModules()
		await space.initialize()
		await flush()
		engine.play.mockClear()

		respond('set_notes_done', () => {
			throw { kind: 'invalid', message: 'nope' }
		})
		await space.setNotesDone(['n1'], true)

		// The failure sound, and specifically not the toggle one.
		expect(engine.play).toHaveBeenCalledTimes(1)
		expect(engine.play).toHaveBeenCalledWith('error')
	})

	it('sounds a section switch only when the user asked for one', async () => {
		const { space } = await freshModules()
		await space.initialize()
		await flush()
		engine.play.mockClear()

		respond('set_active_section', () => makeSpace())
		await space.setActiveSection('sec_a')
		expect(engine.play).toHaveBeenCalledWith('pop')

		// A document arriving from the watcher re-derives `activeSection` too, and
		// that is exactly what a watcher-based implementation would sound by
		// mistake.
		engine.play.mockClear()
		emit('space-changed', { id: 'spc_1', path: 'C:\\notes.copper', reason: 'external' })
		await flush()
		expect(engine.play).not.toHaveBeenCalled()
	})

	/**
	 * Task-012 AC11's frontend half. There is no `capture://succeeded` event —
	 * a capture is silent on success by design — so `space-changed` with this one
	 * reason is the only signal, and `append_capture` emits it only after the
	 * write.
	 */
	it('sounds a successful capture, and only for the capture reason', async () => {
		const { space } = await freshModules()
		await space.initialize()
		await flush()
		engine.play.mockClear()

		emit('space-changed', { id: 'spc_1', path: 'C:\\notes.copper', reason: 'capture' })
		await flush()
		expect(engine.play).toHaveBeenCalledWith('tick')

		engine.play.mockClear()
		for (const reason of ['external', 'reload', 'editor']) {
			emit('space-changed', { id: 'spc_1', path: 'C:\\notes.copper', reason })
		}
		await flush()
		expect(engine.play).not.toHaveBeenCalled()
	})

	it('sounds a capture failure once per failure', async () => {
		const { captureNotice } = await freshModules()
		await captureNotice.initialize()
		await flush()
		engine.play.mockClear()

		emit('capture://failed', { cause: 'no-selection', message: 'Nothing selected', generation: 1 })
		expect(engine.play).toHaveBeenCalledTimes(1)
		expect(engine.play).toHaveBeenCalledWith('error')

		// Dismissal is not an event in its own right.
		engine.play.mockClear()
		emit('capture://cleared', { generation: 1 })
		expect(engine.play).not.toHaveBeenCalled()
	})

	it('sounds an attachment commit once, not once per file', async () => {
		const { attachments } = await freshModules()
		engine.play.mockClear()

		respond('attach_paths', () => [
			{ id: 'att_1', name: 'a.png', path: 'C:\\a.png', bytes: 1, kind: 'image' },
			{ id: 'att_2', name: 'b.png', path: 'C:\\b.png', bytes: 1, kind: 'image' },
			{ id: 'att_3', name: 'c.png', path: 'C:\\c.png', bytes: 1, kind: 'image' },
		])
		await attachments.attachPaths(['C:\\a.png', 'C:\\b.png', 'C:\\c.png'])

		expect(engine.play).toHaveBeenCalledTimes(1)
		expect(engine.play).toHaveBeenCalledWith('plip')
	})

	it('stays silent when an ingest adds nothing', async () => {
		const { attachments } = await freshModules()
		engine.play.mockClear()

		respond('attach_paths', () => [])
		await attachments.attachPaths(['C:\\a.png'])

		expect(engine.play).not.toHaveBeenCalled()
	})
})

/**
 * These two go through `Composer` rather than through the composable, and that
 * is the point of them.
 *
 * `Composer` is the real first caller of `useSounds()` in the running app — it
 * calls it in `setup()` — so it is the component that decides which effect scope
 * the settings watcher lands in, and it is also the only place the composer chime
 * is wired. Nothing here may call `useSounds()` before the mount, or the install
 * happens outside a component scope and the defect under test disappears.
 */
describe('the composer, which is the component that installs the watcher', () => {
	async function freshComposer() {
		vi.resetModules()
		const [settingsModule, spaceModule, composerModule] = await Promise.all([
			import('./useSettings'),
			import('./useSpace'),
			import('@/components/Composer.vue'),
		])
		const space = spaceModule.useSpace()
		await space.initialize()
		await flush()
		return {
			settings: settingsModule.useSettings(),
			wrapper: mount(composerModule.default, { attachTo: document.body }),
		}
	}

	/** Task-012 AC10's other half — the Specification names `chime` for a composer
	 *  submit by hand, and the `if (result)` placement was previously untested. */
	it('sounds a chime when a submit is accepted, and nothing when it is refused', async () => {
		const { wrapper } = await freshComposer()
		await flush()
		engine.play.mockClear()

		await wrapper.find('textarea').setValue('a note')
		await wrapper.find('form').trigger('submit')
		await flush()

		expect(engine.play).toHaveBeenCalledWith('chime')

		engine.play.mockClear()
		respond('submit_entry', () => {
			throw { kind: 'invalid', message: 'nope' }
		})
		await wrapper.find('textarea').setValue('another note')
		await wrapper.find('form').trigger('submit')
		await flush()

		// The failure sound, and specifically not the confirmation one.
		expect(engine.play).not.toHaveBeenCalledWith('chime')
		expect(engine.play).toHaveBeenCalledWith('error')
	})

	/**
	 * The regression test for the watcher's lifetime, and the reason `install()`
	 * owns a detached `effectScope`.
	 *
	 * A `watch` registered during `Composer`'s `setup()` belongs to that
	 * component's scope. The panel replaces the list with the settings view, so
	 * Composer unmounts — and because `installed` never resets, the watcher would
	 * be gone for the rest of the session. The user would then be standing on the
	 * one screen that can change this setting, changing it, and hearing nothing
	 * happen, in either direction.
	 *
	 * The unmount is the whole test. Without it this passes against the bug.
	 */
	it('keeps applying the setting after the installing component is gone', async () => {
		const { settings, wrapper } = await freshComposer()
		await flush()

		wrapper.unmount()
		engine.setEnabled.mockClear()

		respond('update_settings', () => makeSettings(true))
		await settings.setSounds(true)
		await flush()

		expect(engine.setEnabled).toHaveBeenLastCalledWith(true)

		respond('update_settings', () => makeSettings(false))
		await settings.setSounds(false)
		await flush()

		expect(engine.setEnabled).toHaveBeenLastCalledWith(false)
	})
})
