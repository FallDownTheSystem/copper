/**
 * The failure notice, and the readiness signal that lets capture start firing.
 *
 * A capture is silent on success and visible only on failure, so this is the
 * only surface the capture pipeline ever renders. Rust emits `capture://failed`
 * *before* it reveals the panel, so the notice is painted by the time the window
 * appears rather than flashing an empty panel.
 *
 * Two things here are load-bearing:
 *
 * - **Generations.** Both events carry the one Rust allocated, and a
 *   `capture://cleared` whose generation is not the current one is ignored. A
 *   burst of failures resets the timer rather than stacking, so the first
 *   timer's clear will arrive while a newer message is on screen — without the
 *   check it would wipe it.
 * - **Readiness.** Tauri does not buffer or replay events, so a failure emitted
 *   before these listeners resolve would reveal a panel with nothing in it.
 *   Rust keeps the keyboard hook disarmed until `capture://ready` arrives, which
 *   is emitted below only after both `listen()` calls have resolved.
 *
 * It does not go through `useSpace.ts`: that file's single-seam rule is about
 * `invoke` strings for the store, and these are unrelated listens.
 */

import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'

export type CaptureNotice = { cause: string; message: string; generation: number }
type ClearedPayload = { generation: number }

/** The event Rust waits on before arming the keyboard hook. */
const READY_EVENT = 'capture://ready'

// --- module-scope state ------------------------------------------------------
// Declared here, not inside the exported function: a ref created inside would
// hand every caller a private copy, and the notice would render in whichever
// component happened to call first.

const notice = ref<CaptureNotice | null>(null)

/** Memoised, so a second caller arriving during the async gap between the first
 *  `initialize()` and its `listen()` resolving joins that registration rather
 *  than starting a duplicate one. */
let initPromise: Promise<void> | null = null
let unlisteners: UnlistenFn[] = []

function onFailed(payload: CaptureNotice) {
	notice.value = payload
}

function onCleared(payload: ClearedPayload) {
	// A stale clear must not clear a newer message.
	if (notice.value?.generation === payload.generation) notice.value = null
}

/**
 * Registers the listeners and tells Rust it is safe to arm capture. Idempotent.
 *
 * A failure here is the one that disables the whole feature: capture stays
 * disarmed forever and the symptom is a double-tap that does nothing, with
 * nothing written anywhere. So the promise is cleared on rejection — a later
 * caller gets a real second attempt instead of being handed the same dead
 * promise — and the reason is logged rather than swallowed.
 */
function initialize(): Promise<void> {
	initPromise ??= (async () => {
		try {
			unlisteners = await Promise.all([
				listen<CaptureNotice>('capture://failed', (event) => onFailed(event.payload)),
				listen<ClearedPayload>('capture://cleared', (event) => onCleared(event.payload)),
			])
			// Only now: a hook armed before this point could reveal an empty panel.
			await emit(READY_EVENT)
		} catch (error) {
			console.error(
				'[copper] capture notice setup failed; capture stays disarmed until this succeeds',
				error,
			)
			// Undo a half-finished registration so a retry cannot double-register.
			unlistenAll()
			throw error
		}
	})()

	return initPromise
}

/** Drops whatever registration exists and forgets the memoised promise, so the
 *  next `initialize()` is a real second attempt rather than the same dead one. */
function unlistenAll() {
	for (const unlisten of unlisteners) unlisten()
	unlisteners = []
	initPromise = null
}

function dispose() {
	unlistenAll()
	notice.value = null
}

export function useCaptureNotice() {
	// The listeners belong to whatever scope registered them, so they come down
	// with it. `tryOnScopeDispose` rather than `onScopeDispose` because this is
	// also called outside a component — from tests, and from anywhere a later
	// phase wants to read the current notice.
	tryOnScopeDispose(dispose)

	return {
		notice: readonly(notice),
		initialize,
		dispose,
	}
}
