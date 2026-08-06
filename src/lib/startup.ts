/**
 * The startup shape the module-scoped Rust adapters share.
 *
 * Four of them — `useSpace`, `useSpaces`, `useSettings` and `useEditorHandoff` —
 * open the same way: register the listeners, **await** that registration, then
 * pull once. The ordering is the whole point and is easy to get subtly wrong.
 * `listen()` returns a promise and registration is not complete when it returns,
 * so calling it earlier in source order than the pull still leaves exactly the
 * window where an event fires between the two and is lost. Awaiting it is what
 * closes that window, and it is stated here once rather than four times.
 *
 * Memoised, so a second caller arriving during the async gap joins the first
 * attempt rather than starting a duplicate registration. `dispose` forgets the
 * memo along with the listeners, so a later `initialize` is a real second
 * attempt — which is what the tests rely on to re-run startup between cases.
 *
 * The unlisteners are stored *before* the pull is awaited, deliberately: a
 * `dispose` racing a slow first pull has to be able to take down a registration
 * that already exists.
 *
 * `useCaptureNotice` deliberately does not use this. Its `initialize` is the one
 * that must be retryable — a failure there leaves the keyboard hook disarmed for
 * the life of the process — so it unwinds a half-finished registration and
 * clears its memo on rejection, which the four above deliberately do not.
 */

import type { UnlistenFn } from '@tauri-apps/api/event'

export type Startup = {
	initialize: () => Promise<void>
	dispose: () => void
}

/**
 * @param register the listens, resolved together
 * @param pull     the first read, run only once every listener is live
 */
export function createStartup(
	register: () => Promise<UnlistenFn[]>,
	pull: () => Promise<void>,
): Startup {
	let promise: Promise<void> | null = null
	let unlisteners: UnlistenFn[] = []

	return {
		initialize() {
			promise ??= (async () => {
				unlisteners = await register()
				await pull()
			})()

			return promise
		},

		dispose() {
			for (const unlisten of unlisteners) unlisten()
			unlisteners = []
			promise = null
		},
	}
}
