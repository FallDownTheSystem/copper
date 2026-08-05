/**
 * The `$EDITOR` handoff, as far as the frontend is concerned — which is
 * deliberately not far.
 *
 * This is a **display seam only**. Temp paths, the bytes last written, the
 * baseline body a save is checked against, and every reconciliation rule live in
 * Rust's `HandoffRegistry`. They have to: the webview is never told the temp
 * path, and the store's change event does not fire for local commands, so a
 * frontend reconciler would be driven by a signal that misses the undo and merge
 * cases it exists to cover.
 *
 * What crosses the boundary is a list of note ids and, for each, whether its last
 * save was refused as conflicted.
 */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

export type HandoffState = { noteId: string; conflicted: boolean }
type HandoffChangedPayload = { handoffs: HandoffState[] }

/** Named by Rust; the messages are the frontend's to render. */
export type OpenOutcome =
	| { kind: 'opened' }
	| { kind: 'no-editor' }
	| { kind: 'at-capacity'; limit: number }
	| { kind: 'error'; message: string }

const handoffs = ref<HandoffState[]>([])

let initPromise: Promise<void> | null = null
let unlisteners: UnlistenFn[] = []

const activeHandoffIds = computed(() => new Set(handoffs.value.map((entry) => entry.noteId)))
const conflictedHandoffIds = computed(
	() => new Set(handoffs.value.filter((entry) => entry.conflicted).map((entry) => entry.noteId)),
)

function isHandingOff(noteId: string) {
	return activeHandoffIds.value.has(noteId)
}

function isConflicted(noteId: string) {
	return conflictedHandoffIds.value.has(noteId)
}

/**
 * Registers the listener, then pulls once.
 *
 * The pull is not redundant: Rust scavenges `%TEMP%\Copper` before any handoff
 * can be registered, so at mount the list is empty — but a reload of the webview
 * with the process still running is not, and Tauri replays no events.
 */
function initialize(): Promise<void> {
	initPromise ??= (async () => {
		unlisteners = [
			await listen<HandoffChangedPayload>(
				'editor-handoff-changed',
				(event) => (handoffs.value = event.payload.handoffs),
			),
		]
		try {
			handoffs.value = await invoke<HandoffState[]>('editor_handoffs')
		} catch (error) {
			console.error('[copper] could not read editor handoffs', error)
		}
	})()

	return initPromise
}

function dispose() {
	for (const unlisten of unlisteners) unlisten()
	unlisteners = []
	initPromise = null
	handoffs.value = []
}

/** Rust decides the outcome — it owns the editor resolution order and the
 *  concurrency cap — and the frontend only renders it. */
async function openInEditor(noteId: string): Promise<OpenOutcome> {
	try {
		return await invoke<OpenOutcome>('editor_open_note', { id: noteId })
	} catch (error) {
		return { kind: 'error', message: String(error) }
	}
}

/**
 * Asks Rust to re-check its live handoffs against the document that has just
 * been applied: a note that no longer exists ends its handoff, and one whose
 * body moved has its temp file rewritten. The work stays in Rust because the
 * temp paths and baselines are only there; only the *trigger* is here, because
 * this side is the only one that sees every writer.
 */
async function reconcile(): Promise<void> {
	try {
		await invoke('editor_reconcile')
	} catch (error) {
		console.error('[copper] editor handoff reconciliation failed', error)
	}
}

async function stopHandoff(noteId: string): Promise<boolean> {
	try {
		await invoke('editor_stop_handoff', { id: noteId })
		return true
	} catch (error) {
		console.error('[copper] could not end the editor handoff', error)
		return false
	}
}

export function useEditorHandoff() {
	return {
		handoffs: readonly(handoffs),
		activeHandoffIds,
		isHandingOff,
		isConflicted,
		initialize,
		dispose,
		openInEditor,
		stopHandoff,
		reconcile,
	}
}
