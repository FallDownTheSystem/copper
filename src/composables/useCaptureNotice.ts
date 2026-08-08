/**
 * The failure notice, the readiness signal that lets capture start firing, and
 * the scroll a capture notification asks for.
 *
 * A capture is silent on success and visible only on failure, so the notice is
 * the only surface the capture pipeline ever renders. Rust emits
 * `capture://failed` *before* it reveals the panel, so it is painted by the time
 * the window appears rather than flashing an empty panel.
 *
 * The third listener is here because this is the module that owns the `capture://`
 * channel, not because it is a notice: clicking a capture notification's body
 * names the note that toast was about, and the panel it reveals has to be looking
 * at *that* note rather than at whatever the most recent capture armed.
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
 *   is emitted below only after every `listen()` call has resolved.
 *
 * It does not go through `useSpace.ts`: that file's single-seam rule is about
 * `invoke` strings for the store, and these are unrelated listens.
 */

import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'

import { noteRow, revealRow } from './useSelection'
import { useSounds } from './useSounds'

export type CaptureNotice = { cause: string; message: string; generation: number }
type ClearedPayload = { generation: number }
type RevealPayload = { note: string }

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
	// Here and not in `onCleared`, which is the auto-dismiss timer: a burst of
	// failures resets the notice rather than stacking it, so this fires once per
	// failure and that is the thing being reported.
	useSounds().captureFailed()
}

function onCleared(payload: ClearedPayload) {
	// A stale clear must not clear a newer message.
	if (notice.value?.generation === payload.generation) notice.value = null
}

/**
 * A capture notification's body was clicked: the reader is asking for *that*
 * note.
 *
 * `revealRow` holds the request until the list has somewhere to scroll, which is
 * exactly what this needs — the panel is being revealed by the same activation
 * and is not laid out yet. It replaces whatever request was standing, which is
 * the point: two captures leave one slot armed for the second note, and clicking
 * the first toast has to overrule it.
 */
function onReveal(payload: RevealPayload) {
	if (payload.note) revealRow(noteRow(payload.note))
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
				listen<RevealPayload>('capture://reveal', (event) => onReveal(event.payload)),
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
