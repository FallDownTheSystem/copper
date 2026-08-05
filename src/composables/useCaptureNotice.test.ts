import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import type { CaptureNotice } from './useCaptureNotice'

/**
 * Mocked at the two Tauri entry points the composable touches. The generation
 * rule and the readiness ordering are real code under test.
 */
const mocks = vi.hoisted(() => ({
	handlers: new Map<string, (event: { payload: unknown }) => void>(),
	listenCount: 0,
	unlistenCount: 0,
	emitted: [] as string[],
	/** Recorded so the ordering rule can be asserted rather than assumed: the
	 *  readiness signal must not go out before both handlers are registered. */
	handlersAtReady: -1,
	/** Drives the setup-failure path, which is the one that would otherwise leave
	 *  capture disarmed for the session with nothing written anywhere. */
	failNextEmit: false,
}))

vi.mock('@tauri-apps/api/event', () => ({
	listen: async (name: string, handler: (event: { payload: unknown }) => void) => {
		// Registration is deliberately not complete when listen() returns, which is
		// the window a second caller must not slip through.
		await Promise.resolve()
		mocks.listenCount++
		mocks.handlers.set(name, handler)
		return () => {
			mocks.unlistenCount++
			mocks.handlers.delete(name)
		}
	},
	emit: async (name: string) => {
		if (mocks.failNextEmit) {
			mocks.failNextEmit = false
			throw new Error('emit failed')
		}
		mocks.handlersAtReady = mocks.handlers.size
		mocks.emitted.push(name)
	},
}))

const { useCaptureNotice } = await import('./useCaptureNotice')

function fail(payload: CaptureNotice) {
	mocks.handlers.get('capture://failed')?.({ payload })
}

function clear(generation: number) {
	mocks.handlers.get('capture://cleared')?.({ payload: { generation } })
}

const FIRST: CaptureNotice = {
	cause: 'no-selection',
	message: 'Nothing was selected.',
	generation: 1,
}

describe('useCaptureNotice', () => {
	beforeEach(() => {
		useCaptureNotice().dispose()
		mocks.handlers.clear()
		mocks.listenCount = 0
		mocks.unlistenCount = 0
		mocks.emitted = []
		mocks.handlersAtReady = -1
		mocks.failNextEmit = false
	})

	it('renders the message a failure carries', async () => {
		const { notice, initialize } = useCaptureNotice()
		await initialize()

		fail(FIRST)
		expect(notice.value).toEqual(FIRST)
	})

	it('clears on a matching generation', async () => {
		const { notice, initialize } = useCaptureNotice()
		await initialize()

		fail(FIRST)
		clear(1)
		expect(notice.value).toBeNull()
	})

	it('ignores a clear for a superseded generation', async () => {
		// A burst of failures resets the timer rather than stacking, so the first
		// timer's clear arrives while a newer message is on screen. Without the
		// generation check it would wipe it.
		const { notice, initialize } = useCaptureNotice()
		await initialize()

		fail(FIRST)
		fail({ cause: 'modifier-held', message: 'Let go of the modifier keys.', generation: 2 })
		clear(1)

		expect(notice.value?.generation).toBe(2)
	})

	it('signals readiness only once both listeners are registered', async () => {
		// Rust keeps the keyboard hook disarmed until this arrives. Emitting early
		// would let a failure reveal a panel with nothing in it — the exact flash
		// the emit-before-reveal ordering exists to prevent.
		const { initialize } = useCaptureNotice()
		await initialize()

		expect(mocks.emitted).toEqual(['capture://ready'])
		expect(mocks.handlersAtReady).toBe(2)
	})

	it('registers once no matter how many callers there are', async () => {
		const first = useCaptureNotice().initialize()
		const second = useCaptureNotice().initialize()
		await Promise.all([first, second])

		expect(mocks.listenCount).toBe(2)
		expect(mocks.emitted).toEqual(['capture://ready'])
	})

	it('registers once even for a caller arriving mid-registration', async () => {
		// The async gap between initialize() being called and listen() resolving.
		const first = useCaptureNotice().initialize()
		await Promise.resolve()
		const second = useCaptureNotice().initialize()
		await Promise.all([first, second])

		expect(mocks.listenCount).toBe(2)
	})

	it('shares one notice across callers', async () => {
		// A ref created inside the exported function would hand every caller a
		// private copy, and the notice would render in whichever component
		// happened to call first.
		const a = useCaptureNotice()
		const b = useCaptureNotice()
		await a.initialize()

		fail(FIRST)
		expect(b.notice.value).toEqual(FIRST)
	})

	it('leaves capture retryable when setup fails', async () => {
		// The failure that disables the whole feature: capture stays disarmed and
		// the symptom is a double-tap doing nothing. A rejected promise must not be
		// cached, or every later caller is handed the same dead one.
		mocks.failNextEmit = true
		const { initialize } = useCaptureNotice()

		await expect(initialize()).rejects.toThrow('emit failed')
		expect(mocks.handlers.size).toBe(0)

		await initialize()
		expect(mocks.emitted).toEqual(['capture://ready'])
	})

	it('unlistens and forgets its state on dispose', async () => {
		const { notice, initialize, dispose } = useCaptureNotice()
		await initialize()
		fail(FIRST)

		dispose()

		expect(mocks.unlistenCount).toBe(2)
		expect(notice.value).toBeNull()
		// And it can start again cleanly.
		await initialize()
		expect(mocks.listenCount).toBe(4)
	})
})
