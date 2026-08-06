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
 * that already exists. A `dispose` racing the *registration* has the opposite
 * problem — there is nothing to take down yet — so the attempt carries a
 * generation and a continuation that finds itself stale unregisters what it just
 * collected rather than storing it. Without that, a view opened and closed
 * inside the `listen()` round trip leaks the listener, and the next visit
 * registers a second one and drives the first with doubled events.
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
	/** Which attempt is current. Only `dispose` moves it on. */
	let generation = 0

	return {
		initialize() {
			promise ??= (async () => {
				const attempt = generation
				const registered = await register()
				// A `dispose` landed while the listens were still in flight, so it
				// found an empty list and this registration is already orphaned —
				// nothing else holds it and nothing else will take it down. The pull
				// is skipped for the same reason: its result would be written on
				// behalf of a startup the caller has already abandoned.
				if (attempt !== generation) {
					for (const unlisten of registered) unlisten()
					return
				}
				unlisteners = registered
				await pull()
			})()

			return promise
		},

		dispose() {
			generation++
			for (const unlisten of unlisteners) unlisten()
			unlisteners = []
			promise = null
		},
	}
}
