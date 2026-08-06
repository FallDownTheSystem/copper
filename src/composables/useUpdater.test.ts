import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

/**
 * The parts of the update flow that cannot be read off the source: which state
 * survives a failure, which does not, and what the row is therefore able to offer
 * next.
 *
 * The retry path is the one worth guarding. Rust puts the approved `Update` back
 * on a failed download so a retry costs no second manifest request and cannot be
 * handed a different version — and that is only reachable if this side keeps
 * offering Install rather than sending the user back through a check.
 */
const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
	listeners: new Map<string, (event: { payload: unknown }) => void>(),
	unlisten: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
	listen: async (event: string, handler: (event: { payload: unknown }) => void) => {
		mocks.listeners.set(event, handler)
		return mocks.unlisten
	},
}))

const UPDATE = {
	version: '0.1.1',
	currentVersion: '0.1.0',
	notes: 'Fixes the thing.',
	date: '2026-08-05',
}

function deferred<T>() {
	let resolve!: (value: T) => void
	let reject!: (reason?: unknown) => void
	const promise = new Promise<T>((res, rej) => {
		resolve = res
		reject = rej
	})
	return { promise, resolve, reject }
}

/** One module graph per case: every ref in this adapter is module-scoped. */
async function freshModule() {
	vi.resetModules()
	mocks.listeners.clear()
	const module = await import('./useUpdater')
	return module.useUpdater()
}

/** Emits on the channel the composable subscribed to during `initialize`. */
function emitProgress(payload: { downloaded: number; total: number | null }) {
	mocks.listeners.get('update://progress')?.({ payload })
}

beforeEach(() => {
	mocks.invoke.mockReset()
	mocks.unlisten.mockReset()
})

describe('checking', () => {
	it('reports up to date without offering an install', async () => {
		mocks.invoke.mockResolvedValue(null)
		const updater = await freshModule()

		await updater.checkForUpdate()

		expect(updater.status.value).toBe('upToDate')
		expect(updater.available.value).toBeNull()
		expect(updater.canInstall.value).toBe(false)
	})

	it('takes the running version from the check rather than keeping its own', async () => {
		mocks.invoke.mockImplementation(async (command) => {
			if (command === 'get_app_version') return '0.1.0'
			return UPDATE
		})
		const updater = await freshModule()

		await updater.checkForUpdate()

		expect(updater.status.value).toBe('available')
		expect(updater.currentVersion.value).toBe('0.1.0')
		expect(updater.available.value?.version).toBe('0.1.1')
		expect(updater.canInstall.value).toBe(true)
	})

	/** Stale state is worse than none: an install offered from a version the last
	 *  check could not confirm would ask Rust for an `Update` it has cleared. */
	it('drops a previously available update when a later check fails', async () => {
		mocks.invoke.mockResolvedValueOnce(UPDATE)
		const updater = await freshModule()
		await updater.checkForUpdate()
		expect(updater.canInstall.value).toBe(true)

		mocks.invoke.mockRejectedValueOnce('Copper couldn’t check for updates: offline')
		await updater.checkForUpdate()

		expect(updater.status.value).toBe('error')
		expect(updater.available.value).toBeNull()
		expect(updater.canInstall.value).toBe(false)
		expect(updater.error.value).toContain('offline')
	})

	it('refuses a second check while one is in flight', async () => {
		const held = deferred<unknown>()
		mocks.invoke.mockReturnValue(held.promise)
		const updater = await freshModule()

		const first = updater.checkForUpdate()
		await updater.checkForUpdate()
		expect(mocks.invoke).toHaveBeenCalledTimes(1)

		held.resolve(null)
		await first
		expect(updater.status.value).toBe('upToDate')
	})
})

describe('installing', () => {
	async function withAvailableUpdate() {
		mocks.invoke.mockResolvedValueOnce(UPDATE)
		const updater = await freshModule()
		await updater.checkForUpdate()
		return updater
	}

	/** The whole point of the retained `Update`: a failed download must leave the
	 *  row offering the same install, not a fresh check. */
	it('keeps the approved version installable after a failed download', async () => {
		const updater = await withAvailableUpdate()
		mocks.invoke.mockRejectedValueOnce("Copper couldn't install the update: signature")

		await updater.installUpdate()

		expect(updater.status.value).toBe('error')
		expect(updater.error.value).toContain('signature')
		expect(updater.available.value?.version).toBe('0.1.1')
		expect(updater.canInstall.value).toBe(true)
		expect(updater.progress.value).toBeNull()
	})

	/**
	 * The escape hatch from the rule above. Reusing the retained update is right
	 * for a corrupted download or a dropped connection, but not for a release
	 * that was re-cut under us — that download fails every time, and without a
	 * second action the row would offer the same doomed install for the life of
	 * the process.
	 */
	it('offers a re-check alongside the retry, and the re-check discards the stale update', async () => {
		const updater = await withAvailableUpdate()
		mocks.invoke.mockRejectedValueOnce("Copper couldn't install the update: signature")
		await updater.installUpdate()

		expect(updater.canInstall.value).toBe(true)
		expect(updater.canRecheck.value).toBe(true)

		mocks.invoke.mockResolvedValueOnce(null)
		await updater.checkForUpdate()

		expect(updater.status.value).toBe('upToDate')
		expect(updater.available.value).toBeNull()
		expect(updater.canInstall.value).toBe(false)
		expect(updater.canRecheck.value).toBe(false)
	})

	/** Two buttons for one decision is not a recovery path. A pending install
	 *  that has not failed offers exactly one action. */
	it('does not offer a re-check while the pending install is still good', async () => {
		const updater = await withAvailableUpdate()

		expect(updater.status.value).toBe('available')
		expect(updater.canInstall.value).toBe(true)
		expect(updater.canRecheck.value).toBe(false)
	})

	it('does nothing when no update has been approved', async () => {
		mocks.invoke.mockResolvedValue(null)
		const updater = await freshModule()

		await updater.installUpdate()

		expect(mocks.invoke).not.toHaveBeenCalled()
		expect(updater.status.value).toBe('idle')
	})
})

describe('progress', () => {
	async function downloading() {
		mocks.invoke.mockResolvedValueOnce(UPDATE)
		const updater = await freshModule()
		await updater.checkForUpdate()
		await updater.initialize()

		const held = deferred<unknown>()
		mocks.invoke.mockReturnValueOnce(held.promise)
		const install = updater.installUpdate()
		return { updater, install, held }
	}

	it('renders a percentage only when the server gave a total', async () => {
		const { updater, install, held } = await downloading()

		emitProgress({ downloaded: 512, total: 2048 })
		expect(updater.percentage.value).toBe(25)

		emitProgress({ downloaded: 512, total: null })
		expect(updater.percentage.value).toBeNull()

		held.reject('stopped')
		await install
	})

	/** A total of zero is a denominator too, and dividing by it would render
	 *  `Infinity%` rather than falling back to the indeterminate indicator. */
	it('treats a zero total as no total', async () => {
		const { updater, install, held } = await downloading()

		emitProgress({ downloaded: 0, total: 0 })
		expect(updater.percentage.value).toBeNull()

		held.reject('stopped')
		await install
	})

	it('ignores events that arrive after the download has failed', async () => {
		const { updater, install, held } = await downloading()

		held.reject('stopped')
		await install
		expect(updater.status.value).toBe('error')

		emitProgress({ downloaded: 4096, total: 8192 })
		expect(updater.progress.value).toBeNull()
	})

	it('takes the listener down on dispose', async () => {
		mocks.invoke.mockResolvedValue('0.1.0')
		const updater = await freshModule()

		await updater.initialize()
		expect(mocks.listeners.has('update://progress')).toBe(true)

		updater.dispose()
		expect(mocks.unlisten).toHaveBeenCalledTimes(1)
	})
})
