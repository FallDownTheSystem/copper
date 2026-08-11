/**
 * The toasts that report what an in-panel action just did.
 *
 * Separate from task-005's `useCaptureNotice`, deliberately. That one is driven
 * by `capture://failed` / `capture://cleared` and reports on a capture that
 * happened while the user was in another application; this one is written by
 * actions the user performed in the panel and is the only one of the two that
 * can carry something to press.
 *
 * **A seam over vue-sonner, not a state module.** Every caller in the panel
 * reports through `setMessage`/`setError`, and this module translates that
 * contract into Sonner toasts — which is what lets the backend change without
 * touching thirty call sites, and did: the previous single-pill implementation
 * (one message, replaced not stacked) lived here in full. Sonner stacks
 * messages instead, per the user's 2026-08-11 direction: marking five notes
 * done one press at a time now leaves five pills, each `Undo` undoing its own
 * press. The rendering half is `StatusToaster.vue`.
 *
 * String policy, since these are read consecutively by the same user: a full
 * sentence takes a terminal period (`Nothing to undo.`), a bare confirmation
 * does not (`Copied 3 notes`). Counts are whole strings per grammatical number —
 * never `note(s)`, and never a number concatenated into a fragment.
 */

import { toast as sonner } from 'vue-sonner'

/** Something a toast offers to do, as one button. There is at most one: a pill
 *  with a choice in it is a dialog, and this is a thing the reader is meant to
 *  be able to ignore. Clicking it also dismisses the toast — Sonner's default,
 *  and the right one: an `Undo` pressed is a decision made. */
export type StatusAction = { label: string; run: () => void }

/**
 * Whether the message is allowed to leave on its own.
 *
 * The distinction is not decoration: a confirmation the reader missed costs
 * nothing, because the thing it confirms already happened and is visible in the
 * list. A failure the reader missed costs them the knowledge that the action
 * they asked for did *not* happen — the list looks the same either way, so the
 * pill is the only place that difference is written down. So `error` has no
 * timer at all and stands until it is dismissed or the stack is cleared.
 */
export type StatusSeverity = 'info' | 'error'

/**
 * Five seconds, matching the single-pill era. The number is a guess at how long
 * it takes to notice a pill and decide about it, so it may only run while there
 * is a reader in front of it. Sonner holds the clock while the pointer is on
 * the toast; the visibility listener below holds it while the panel is hidden,
 * which is every Escape to the tray — a message that spent its whole window
 * hidden would be an `Undo` that expired in a tray icon.
 */
const LIFETIME_MS = 5000

/** The infos currently on a clock, by Sonner id, with everything needed to
 *  re-issue them: pausing a Sonner toast from outside is an in-place update
 *  (same id, new duration), and an update must carry the whole payload. */
const live = new Map<string | number, { text: string; action: StatusAction | null }>()

function sonnerAction(action: StatusAction | null) {
	return action ? { label: action.label, onClick: () => action.run() } : undefined
}

/** Re-issues every live info with the given duration — `Infinity` parks them,
 *  `LIFETIME_MS` restarts them. Restarting grants the full window rather than a
 *  banked remainder, which is the point of the window: it measures the reader's
 *  attention, and a reader coming back to the panel is starting over. */
function retime(duration: number) {
	for (const [id, { text, action }] of live) {
		sonner(text, { id, duration, action: sonnerAction(action) })
	}
}

/**
 * Registered once for the module, and never removed. The toast state is a
 * module singleton for the whole application, so this listener has exactly the
 * lifetime the state it guards does; hanging it off a component's `onMounted`
 * would unhook it during the one thing it exists to survive — the panel being
 * hidden.
 */
if (typeof document !== 'undefined') {
	document.addEventListener('visibilitychange', () => {
		retime(document.hidden ? Number.POSITIVE_INFINITY : LIFETIME_MS)
	})
}

/**
 * `severity` decides whether the toast is on a clock, and an `error` never
 * carries a caller's action.
 *
 * A failure's only button is the one that acknowledges it, because an error is
 * the one message the reader is not finished with when it appears: something
 * they asked for did not happen, and offering them `Undo` beside that is
 * offering to reverse a thing that was never done.
 */
function setMessage(
	text: string,
	action: StatusAction | null = null,
	severity: StatusSeverity = 'info',
) {
	if (severity === 'error') return setError(text)
	const id = sonner(text, {
		// Born parked when the panel is hidden — a capture from the global hotkey
		// can raise a toast nobody is looking at, and its window must not start
		// until they are.
		duration:
			typeof document !== 'undefined' && document.hidden
				? Number.POSITIVE_INFINITY
				: LIFETIME_MS,
		action: sonnerAction(action),
		onDismiss: (toast) => live.delete(toast.id),
		onAutoClose: (toast) => live.delete(toast.id),
	})
	live.set(id, { text, action })
}

/** A failure, reported where every other outcome of the same action is. The
 *  button is named rather than left to a close glyph: a bare `×` in a strip of
 *  text is a smaller target than the word. Its handler is empty because the
 *  press's meaning *is* the dismissal Sonner performs after it. */
function setError(text: string) {
	sonner.error(text, {
		duration: Number.POSITIVE_INFINITY,
		action: { label: 'Dismiss', onClick: () => {} },
	})
}

/**
 * The store's `list`-scope error, mirrored into the stack under one stable id.
 *
 * It is not a `setError` because its lifecycle is not the reader's: the store
 * owns it — it appears when a shell operation is refused and leaves when a
 * retry succeeds — so it carries no Dismiss and is withdrawn by the same watch
 * that raised it. `PanelShell` announces the same text through its assertive
 * live region; this is only the visible half.
 */
const LIST_ERROR_ID = 'list-error'

function showListError(text: string | null) {
	if (text === null) sonner.dismiss(LIST_ERROR_ID)
	else sonner.error(text, { id: LIST_ERROR_ID, duration: Number.POSITIVE_INFINITY })
}

/** Empties the whole stack. An applied external document is the main caller:
 *  every standing `Undo` was minted against a document that no longer exists. */
function clear() {
	live.clear()
	sonner.dismiss()
}

/**
 * Picks between whole messages, one per grammatical number.
 *
 * The point is that each caller writes out every form it needs — `Copied 1 note`
 * and `Copied 3 notes` are two complete strings, not one fragment with a count
 * glued to it. A shared `${verb} ${count} note(s)` helper is the thing this
 * exists to prevent: it cannot express a language where the noun, the verb or
 * the word order changes with the number, and it produces `note(s)` in the one
 * it can.
 */
export function countMessage(count: number, forms: { one: string; many: (n: number) => string }) {
	return count === 1 ? forms.one : forms.many(count)
}

export function useStatusMessage() {
	return {
		setMessage,
		setError,
		showListError,
		clear,
	}
}
