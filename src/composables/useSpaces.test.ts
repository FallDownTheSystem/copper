import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import type { Space } from './useSpace'
import type { Availability, RecentEntry } from './useSpaces'

/**
 * Mocked at the two Tauri entry points, exactly as `useSpace.test.ts` does.
 * Everything else — the listen-then-pull ordering, the `changed: false` no-op,
 * the row patching — is real code under test.
 */
const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
	handlers: new Map<string, (event: { payload: unknown }) => void>(),
	/** Recorded so "subscribe before you pull" can be asserted rather than
	 *  assumed: registration is not complete when `listen()` returns. */
	handlersAtFirstInvoke: -1,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
	listen: async (name: string, handler: (event: { payload: unknown }) => void) => {
		await Promise.resolve()
		mocks.handlers.set(name, handler)
		return () => mocks.handlers.delete(name)
	},
}))

type Responder = (args?: Record<string, unknown>) => unknown
const responders = new Map<string, Responder>()

function respond(command: string, responder: Responder) {
	responders.set(command, responder)
}

function callsTo(command: string) {
	return mocks.invoke.mock.calls.filter((call) => call[0] === command).length
}

function emit(name: string, payload: unknown) {
	mocks.handlers.get(name)?.({ payload })
}

async function flush(times = 4) {
	for (let i = 0; i < times; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

function makeSpace(id: string, name: string): Space {
	return {
		id,
		name,
		activeSection: 'sec_a',
		sections: [{ id: 'sec_a', name: 'Notes', order: 0 }],
		notes: [],
	}
}

function entry(path: string, overrides: Partial<RecentEntry> = {}): RecentEntry {
	return {
		path,
		displayPath: path,
		key: path.toUpperCase(),
		name: path.replace(/^.*\\|\.copper$/g, ''),
		active: false,
		availability: { state: 'pending' },
		...overrides,
	}
}

const STATUS = {
	path: 'C:\\work.copper',
	errored: false,
	watching: true,
	canUndo: false,
	canRedo: false,
	startupNotice: null,
}

const SETTINGS = {
	recents: ['C:\\work.copper'],
	activeSpace: 0,
	panelPosition: null,
	shortcuts: {},
	theme: 'system',
}

let recents: RecentEntry[] = []

/** Both modules come from one fresh graph, so the singleton `useSpaces` reaches
 *  is the same singleton the assertions read. */
async function freshModules() {
	vi.resetModules()
	const spaces = (await import('./useSpaces')).useSpaces()
	const space = (await import('./useSpace')).useSpace()
	return { spaces, space }
}

beforeEach(() => {
	mocks.invoke.mockReset()
	mocks.handlers.clear()
	mocks.handlersAtFirstInvoke = -1
	responders.clear()

	recents = [
		entry('C:\\work.copper', { active: true, availability: { state: 'available' } }),
		entry('D:\\archive.copper'),
	]

	mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
		if (mocks.handlersAtFirstInvoke === -1) mocks.handlersAtFirstInvoke = mocks.handlers.size
		const responder = responders.get(command)
		if (!responder) throw { kind: 'invalid', message: `no responder: ${command}` }
		return responder(args)
	})

	respond('list_recents', () => recents.map((row) => ({ ...row })))
	respond('refresh_recents', () => null)
	respond('get_status', () => ({ ...STATUS }))
	respond('get_settings', () => ({ ...SETTINGS }))
	respond('get_active_space', () => makeSpace('spc_1', 'work'))
})

describe('the mount-time pull', () => {
	it('registers both handlers before the first invoke', async () => {
		const { spaces } = await freshModules()
		await spaces.initialize()

		expect(mocks.handlersAtFirstInvoke).toBe(2)
		expect(spaces.recents.value).toHaveLength(2)
	})

	// A launch-with-file open happens during `setup()`, before the webview can
	// listen, and Tauri replays nothing — so the pull is what discovers it.
	it('does not start probes just by listing', async () => {
		const { spaces } = await freshModules()
		await spaces.initialize()

		expect(callsTo('list_recents')).toBe(1)
		expect(callsTo('refresh_recents')).toBe(0)
	})

	it('starts probes only when asked', async () => {
		const { spaces } = await freshModules()
		await spaces.initialize()
		await spaces.probeRecents()

		expect(callsTo('refresh_recents')).toBe(1)
	})
})

describe('availability results', () => {
	it('patches the row with the matching key and leaves the others alone', async () => {
		const { spaces } = await freshModules()
		await spaces.initialize()

		const availability: Availability = {
			state: 'unavailable',
			reason: 'drive-unavailable',
			message: "The drive this space is on isn't connected.",
		}
		emit('spaces-availability-changed', {
			generation: 1,
			key: 'D:\\ARCHIVE.COPPER',
			availability,
			name: null,
		})
		await flush()

		expect(spaces.recents.value[1]?.availability).toEqual(availability)
		expect(spaces.recents.value[0]?.availability).toEqual({ state: 'available' })
		// A re-list would be a feedback loop: results would ask for a list, and a
		// list would ask for results.
		expect(callsTo('list_recents')).toBe(1)
	})

	it('takes the document name a probe read back', async () => {
		const { spaces } = await freshModules()
		await spaces.initialize()

		emit('spaces-availability-changed', {
			generation: 1,
			key: 'D:\\ARCHIVE.COPPER',
			availability: { state: 'available' },
			name: 'archive of everything',
		})
		await flush()

		expect(spaces.recents.value[1]?.name).toBe('archive of everything')
	})

	it('ignores a result for an entry that is no longer listed', async () => {
		const { spaces } = await freshModules()
		await spaces.initialize()

		expect(() =>
			emit('spaces-availability-changed', {
				generation: 1,
				key: 'E:\\GONE.COPPER',
				availability: { state: 'available' },
				name: null,
			}),
		).not.toThrow()
		expect(spaces.recents.value).toHaveLength(2)
	})
})

describe('switching', () => {
	it('adopts the document the command returned rather than pulling it again', async () => {
		const { spaces, space } = await freshModules()
		await space.initialize()
		await spaces.initialize()
		await flush()
		const pullsBefore = callsTo('get_active_space')

		respond('activate_space', () => ({ changed: true, space: makeSpace('spc_2', 'archive') }))
		await spaces.openSpace('D:\\archive.copper')
		await flush()

		expect(space.space.value?.id).toBe('spc_2')
		expect(space.spaceName.value).toBe('archive')
		// The outcome carries the document; reading it again would be a second
		// chance for the two to disagree.
		expect(callsTo('get_active_space')).toBe(pullsBefore)
	})

	// A23. Re-opening the space that is already open must change nothing, which
	// is what preserves the list's scroll position and selection.
	it('does nothing at all when the space is already active', async () => {
		const { spaces, space } = await freshModules()
		await space.initialize()
		await spaces.initialize()
		await flush()
		const before = space.space.value

		respond('activate_space', () => ({ changed: false, space: null }))
		const outcome = await spaces.openSpace('C:\\work.copper')
		await flush()

		expect(outcome?.changed).toBe(false)
		expect(space.space.value).toBe(before)
	})

	it('re-reads settings after a switch, because recents just moved', async () => {
		const { spaces, space } = await freshModules()
		await space.initialize()
		await spaces.initialize()
		await flush()
		const settingsBefore = callsTo('get_settings')

		respond('activate_space', () => ({ changed: true, space: makeSpace('spc_2', 'archive') }))
		await spaces.openSpace('D:\\archive.copper')
		await flush()

		expect(callsTo('get_settings')).toBe(settingsBefore + 1)
		expect(callsTo('list_recents')).toBeGreaterThan(1)
	})

	// A8. Selecting an unavailable entry surfaces its cause and leaves the
	// active space alone. The refusal comes from Rust so that an entry which has
	// come back opens on the next attempt with no repair step.
	it('surfaces a refusal in the list-scope error band', async () => {
		const { spaces, space } = await freshModules()
		await space.initialize()
		await spaces.initialize()
		await flush()
		const before = space.space.value

		respond('activate_space', () => {
			throw { kind: 'not-found', message: 'This file has been moved, renamed, or deleted.' }
		})
		const outcome = await spaces.openSpace('D:\\archive.copper')
		await flush()

		expect(outcome).toBeNull()
		expect(space.errorFor('list').value).toBe('This file has been moved, renamed, or deleted.')
		expect(space.space.value).toBe(before)
	})
})

describe('removing an entry', () => {
	it('re-lists after a removal', async () => {
		const { spaces } = await freshModules()
		await spaces.initialize()
		respond('remove_recent', () => {
			recents = recents.filter((row) => row.path !== 'D:\\archive.copper')
			return null
		})

		expect(await spaces.removeRecent('D:\\archive.copper')).toBe(true)
		await flush()

		expect(spaces.recents.value.map((row) => row.path)).toEqual(['C:\\work.copper'])
	})

	// A26. The disabled control in the menu is a courtesy; this is the
	// enforcement, and the message has to reach the user either way.
	it('reports the refusal when the active entry is targeted', async () => {
		const { spaces, space } = await freshModules()
		await spaces.initialize()
		respond('remove_recent', () => {
			throw {
				kind: 'invalid',
				message: 'this is the space you have open. Switch to another space first.',
			}
		})

		expect(await spaces.removeRecent('C:\\work.copper')).toBe(false)
		await flush()

		expect(space.errorFor('list').value).toContain('Switch to another space first')
		expect(spaces.recents.value).toHaveLength(2)
	})
})

describe('the settings-changed handler', () => {
	it('re-lists recents without starting probes', async () => {
		const { spaces } = await freshModules()
		await spaces.initialize()

		recents = [entry('C:\\work.copper', { active: true })]
		emit('settings-changed', {})
		await flush()

		expect(spaces.recents.value).toHaveLength(1)
		expect(callsTo('refresh_recents')).toBe(0)
	})
})
