import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import type { Space, StoreStatus } from './useSpace'

/**
 * The IPC seam is mocked at the two Tauri entry points, which is the whole
 * surface `useSpace` touches. Everything else — the coalescing, the discard
 * rule, the status re-pull rule — is real code under test.
 */
const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
	handlers: new Map<string, (event: { payload: unknown }) => void>(),
	listenCount: 0,
	/** Recorded so the awaited-registration rule can be asserted rather than
	 *  assumed: the first invoke must happen with both handlers already in. */
	handlersAtFirstInvoke: -1,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
	listen: async (name: string, handler: (event: { payload: unknown }) => void) => {
		// Registration is deliberately not complete when listen() returns, which is
		// the exact window `await Promise.all([...listen()])` exists to close.
		await Promise.resolve()
		mocks.listenCount++
		mocks.handlers.set(name, handler)
		return () => mocks.handlers.delete(name)
	},
}))

function deferred<T>() {
	let resolve!: (value: T) => void
	let reject!: (reason?: unknown) => void
	const promise = new Promise<T>((res, rej) => {
		resolve = res
		reject = rej
	})
	return { promise, resolve, reject }
}

function makeSpace(id: string, noteIds: string[]): Space {
	return {
		id,
		name: 'development',
		activeSection: 'sec_a',
		sections: [{ id: 'sec_a', name: 'Research', order: 0 }],
		notes: noteIds.map((noteId, order) => ({
			id: noteId,
			section: 'sec_a',
			order,
			done: false,
			body: noteId,
			created: '2026-08-05T00:00:00Z',
			updated: '2026-08-05T00:00:00Z',
		})),
	}
}

const STATUS: StoreStatus = {
	path: 'C:\\notes.copper',
	errored: false,
	watching: true,
	canUndo: false,
	canRedo: false,
	startupNotice: null,
}

const SETTINGS = {
	recents: ['C:\\notes.copper'],
	activeSpace: 0,
	panelPosition: null,
	shortcuts: {},
	theme: 'system',
}

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

/** Lets pending microtasks and Vue's scheduler settle. */
async function flush(times = 4) {
	for (let i = 0; i < times; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

async function freshModule() {
	vi.resetModules()
	const module = await import('./useSpace')
	return module.useSpace()
}

beforeEach(() => {
	mocks.invoke.mockReset()
	mocks.handlers.clear()
	mocks.listenCount = 0
	mocks.handlersAtFirstInvoke = -1
	responders.clear()

	// Tauri's `invoke` is declared `async`, so it always returns a promise and
	// never throws synchronously. The mock matches that: a responder that throws
	// produces a rejection, not a synchronous exception at the call site.
	mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
		if (mocks.handlersAtFirstInvoke === -1) mocks.handlersAtFirstInvoke = mocks.handlers.size
		const responder = responders.get(command)
		if (!responder) throw { kind: 'invalid', message: `no responder: ${command}` }
		return responder(args)
	})

	respond('get_status', () => ({ ...STATUS }))
	respond('get_settings', () => ({ ...SETTINGS }))
	respond('get_active_space', () => makeSpace('spc_1', ['n1']))
})

describe('the mount-time pull', () => {
	it('populates the panel from three calls with no event involved', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()

		expect(space.loadState.value).toBe('ready')
		expect(space.space.value?.notes).toHaveLength(1)
		expect(space.storeStatus.value.watching).toBe(true)
		expect(space.settings.value).not.toBeNull()
		expect(callsTo('get_active_space')).toBe(1)
	})

	it('registers both handlers before the first invoke', async () => {
		const space = await freshModule()
		await space.initialize()

		// `listen()` returns a promise and registration is not complete when it
		// returns, so calling them in source order above the pull leaves the
		// lost-event window open.
		expect(mocks.handlersAtFirstInvoke).toBe(2)
	})

	it('is idempotent: two initialize calls produce one pull and one subscription', async () => {
		const space = await freshModule()
		await Promise.all([space.initialize(), space.initialize()])
		await flush()

		expect(callsTo('get_active_space')).toBe(1)
		expect(mocks.listenCount).toBe(2) // one per event name, not two per name
	})

	it('reports a failed initial pull as an error rather than an empty space', async () => {
		respond('get_active_space', () => {
			throw { kind: 'io', message: 'Access is denied.' }
		})

		const space = await freshModule()
		await space.initialize()

		expect(space.loadState.value).toBe('error')
		expect(space.loadError.value).toContain('Access is denied.')
		expect(space.space.value).toBeNull()
	})
})

describe('local mutations', () => {
	it('updates the list from the return value, with no event fired', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()

		respond('add_note', () => ({ space: makeSpace('spc_1', ['n1', 'n2']), noteId: 'n2' }))
		await space.addNote('second')
		await flush()

		// The regression test for the return-value contract: an adapter that waited
		// for `space-changed` here would hang forever, because a frontend-initiated
		// mutation emits nothing.
		expect(space.space.value?.notes.map((note) => note.id)).toEqual(['n1', 'n2'])
		expect(callsTo('get_active_space')).toBe(1)
	})

	it('sets canUndo locally after a structural mutation instead of re-pulling', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()
		const statusCalls = callsTo('get_status')

		respond('set_notes_done', () => makeSpace('spc_1', ['n1']))
		await space.setNotesDone(['n1'], true)
		await flush()

		// Deterministic and knowable without a round trip.
		expect(callsTo('get_status')).toBe(statusCalls)
		expect(space.storeStatus.value.canUndo).toBe(true)
		expect(space.storeStatus.value.canRedo).toBe(false)
	})

	it('re-pulls status after edit_note, which takes no undo snapshot of its own', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()
		const statusCalls = callsTo('get_status')

		respond('edit_note', () => makeSpace('spc_1', ['n1']))
		await space.updateNoteBody('n1', 'edited')
		await flush()

		// A write re-applied over an external change clears both stacks and emits
		// nothing, so a re-pull is the only way to learn about it.
		expect(callsTo('get_status')).toBe(statusCalls + 1)
	})

	it('leaves the document alone and surfaces a failure without a global error', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()

		respond('add_note', () => {
			throw { kind: 'unavailable', message: 'the space is unreadable' }
		})
		const result = await space.addNote('nope')

		expect(result).toBeNull()
		expect(space.actionError.value).toBe('the space is unreadable')
		// A failed mutation must not become the global error state, and must not
		// leave the panel showing the empty state.
		expect(space.loadState.value).toBe('ready')
		expect(space.space.value?.notes).toHaveLength(1)
	})
})

describe('the single-in-flight coalesced refresh', () => {
	it('discards a late response for a superseded refresh instead of applying it', async () => {
		const pull = deferred<Space>()
		respond('get_active_space', () => pull.promise)

		const space = await freshModule()
		const init = space.initialize()
		await flush()

		// The watcher installs B and the event handler applies it while the mount
		// pull is still outstanding.
		respond('get_active_space', () => makeSpace('spc_1', ['n1', 'n2']))
		emit('space-changed', { id: 'spc_1', path: 'p', reason: 'external' })
		await flush()
		expect(space.space.value?.notes).toHaveLength(2)

		// The mount pull now resolves with A. Applying it "in request order" would
		// still display A indefinitely, because nothing has changed since and no
		// further event is coming.
		pull.resolve(makeSpace('spc_1', ['n1']))
		await init
		await flush()

		expect(space.space.value?.notes.map((note) => note.id)).toEqual(['n1', 'n2'])
	})

	it('discards a mutation response superseded by a newer applied refresh', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()

		const add = deferred<{ space: Space; noteId: string }>()
		respond('add_note', () => add.promise)
		const pending = space.addNote('mine')
		await flush()

		respond('get_active_space', () => makeSpace('spc_1', ['n1', 'external']))
		emit('space-changed', { id: 'spc_1', path: 'p', reason: 'external' })
		await flush()

		add.resolve({ space: makeSpace('spc_1', ['n1', 'mine']), noteId: 'mine' })
		await pending
		await flush()

		// The stale mutation document is dropped and a refresh scheduled, rather
		// than being written over the fresher one.
		expect(space.space.value?.notes.map((note) => note.id)).toEqual(['n1', 'external'])
	})

	it('coalesces three events during one in-flight refresh into exactly one more', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()
		const before = callsTo('get_active_space')

		const first = deferred<Space>()
		respond('get_active_space', () => first.promise)
		emit('space-changed', { id: 'spc_1', path: 'p', reason: 'external' })
		await flush()

		// Three more arrive while that one is still outstanding.
		respond('get_active_space', () => makeSpace('spc_1', ['n1', 'n2']))
		emit('space-changed', { id: 'spc_1', path: 'p', reason: 'external' })
		emit('space-changed', { id: 'spc_1', path: 'p', reason: 'capture' })
		emit('space-changed', { id: 'spc_1', path: 'p', reason: 'external' })
		await flush()

		first.resolve(makeSpace('spc_1', ['n1']))
		await flush()

		// A trailing-edge flag, not a queue of N: one in flight plus exactly one
		// more, because every payload is identity-only and a coalesced refresh is
		// always at least as fresh as the events it replaces.
		expect(callsTo('get_active_space') - before).toBe(2)
	})

	it('sets refreshing rather than loadState during a background reload', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()

		const pull = deferred<Space>()
		respond('get_active_space', () => pull.promise)
		emit('space-changed', { id: 'spc_1', path: 'p', reason: 'external' })
		await flush()

		expect(space.refreshing.value).toBe(true)
		// Must not unmount the list or the open editor.
		expect(space.loadState.value).toBe('ready')

		pull.resolve(makeSpace('spc_1', ['n1']))
		await flush()
		expect(space.refreshing.value).toBe(false)
	})
})

describe('status and the error banner', () => {
	it('re-pulls status on every space-changed and store-error event', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()
		const before = callsTo('get_status')

		emit('space-changed', { id: 'spc_1', path: 'p', reason: 'external' })
		await flush()
		emit('store-error', { kind: 'parse', message: 'duplicate note ids' })
		await flush()

		expect(callsTo('get_status') - before).toBe(2)
	})

	it('keeps the document rendered when the file becomes unparseable', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()

		respond('get_status', () => ({ ...STATUS, errored: true }))
		emit('store-error', { kind: 'parse', message: 'duplicate note ids' })
		await flush()

		expect(space.storeStatus.value.errored).toBe(true)
		expect(space.storeErrorEvent.value?.message).toBe('duplicate note ids')
		// The in-memory document stays alive; only the banner reports it is stale.
		expect(space.loadState.value).toBe('ready')
		expect(space.space.value?.notes).toHaveLength(1)
	})

	it('clears the errored banner on reason "reload" even when the document is unchanged', async () => {
		const space = await freshModule()
		await space.initialize()
		await flush()

		respond('get_status', () => ({ ...STATUS, errored: true }))
		emit('store-error', { kind: 'parse', message: 'bad json' })
		await flush()
		expect(space.storeStatus.value.errored).toBe(true)

		// The file came back byte-identical. The document domain legitimately
		// no-ops; the status domain must still reset, because the flag clearing is
		// itself the observable change and no further event is coming.
		respond('get_status', () => ({ ...STATUS, errored: false }))
		emit('space-changed', { id: 'spc_1', path: 'p', reason: 'reload' })
		await flush()

		expect(space.storeStatus.value.errored).toBe(false)
		expect(space.storeErrorEvent.value).toBeNull()
	})

	it('does not treat watching:false as an error or block mutations', async () => {
		respond('get_status', () => ({ ...STATUS, watching: false }))

		const space = await freshModule()
		await space.initialize()
		await flush()

		expect(space.storeStatus.value.watching).toBe(false)
		expect(space.storeStatus.value.errored).toBe(false)
		expect(space.loadState.value).toBe('ready')

		respond('add_note', () => ({ space: makeSpace('spc_1', ['n1', 'n2']), noteId: 'n2' }))
		const result = await space.addNote('still writable')

		expect(result).not.toBeNull()
		expect(space.actionError.value).toBeNull()
	})
})

describe('retry', () => {
	it('re-opens the space by path rather than re-reading the in-memory document', async () => {
		respond('get_active_space', () => {
			throw { kind: 'io', message: 'gone' }
		})

		const space = await freshModule()
		await space.initialize()
		expect(space.loadState.value).toBe('error')

		respond('open_space', (args) => {
			expect(args).toEqual({ path: 'C:\\notes.copper' })
			return makeSpace('spc_1', ['n1'])
		})
		await space.retry()
		await flush()

		expect(callsTo('open_space')).toBe(1)
		expect(space.loadState.value).toBe('ready')
	})
})

describe('space identity', () => {
	it('bumps the epoch and drops document-scoped state when the id changes', async () => {
		vi.resetModules()
		const spaceModule = await import('./useSpace')
		const selectionModule = await import('./useSelection')
		const space = spaceModule.useSpace()
		const selection = selectionModule.useSelection()

		await space.initialize()
		await flush()

		selection.select('n1')
		expect(selection.selectedIds.value).toEqual(['n1'])
		const epochBefore = space.epoch.value

		// A checkout replaced the file with a different document in which an id
		// coincidentally matches.
		respond('get_active_space', () => makeSpace('spc_2', ['other', 'n1']))
		emit('space-changed', { id: 'spc_2', path: 'p', reason: 'external' })
		await flush()

		expect(space.epoch.value).toBe(epochBefore + 1)
		// Nothing carries over onto the coincidentally matching id — not the
		// selection, and not the focus position either. The grid still gets a
		// roving target, but by the first-load rule rather than by the previous
		// document's flattened index.
		expect(selection.selectedIds.value).toEqual([])
		expect(selection.focusedId.value).toBe('n:other')
	})
})
