/**
 * The toast that reports what an in-panel action just did.
 *
 * Separate from task-005's `useCaptureNotice`, deliberately. That one is driven
 * by `capture://failed` / `capture://cleared` and reports on a capture that
 * happened while the user was in another application; this one is written by
 * actions the user performed in the panel and is the only one of the two that
 * can carry something to press.
 *
 * **One pill, replaced rather than stacked.** A second message does not queue
 * behind the first and does not open a second surface: it takes the pill and
 * restarts the clock. Marking five notes done one press at a time leaves one
 * toast on screen, saying what the fifth press did — which is also what its
 * `Undo` then undoes.
 *
 * String policy, since these are read consecutively by the same user: a full
 * sentence takes a terminal period (`Nothing to undo.`), a bare confirmation
 * does not (`Copied 3 notes`). Counts are whole strings per grammatical number —
 * never `note(s)`, and never a number concatenated into a fragment.
 */

/** Something the toast offers to do, as one button. There is at most one: a pill
 *  with a choice in it is a dialog, and this is a thing that disappears on its
 *  own after five seconds. */
export type StatusAction = { label: string; run: () => void }

export type StatusToast = {
	text: string
	action: StatusAction | null
	/**
	 * Bumped per message, and the pill is keyed on it.
	 *
	 * Two toasts with the same text are two events — a second `Moved 1 note to
	 * Done` means a second note went — and without a key Vue patches the text of
	 * an element that is already on screen, so the entry animation does not run
	 * and the replacement is invisible. Which is exactly the case that has to be
	 * visible, because it is also the one where the `Undo` button changed what it
	 * would undo.
	 */
	generation: number
}

const toast = ref<StatusToast | null>(null)

/**
 * Five seconds, and the timer replaced clearing on the next user action.
 *
 * The old rule — a capture-phase listener wiping the pill on the next keypress
 * or click — cannot coexist with a button in the toast: the affordance would be
 * gone before the pointer reached it, since moving toward it is itself a user
 * action somewhere. A timer is also the only form that survives the panel being
 * looked at rather than typed into, which is what the reader does for the second
 * or two it takes to decide whether to undo.
 */
const LIFETIME_MS = 5000

let generation = 0
let timer: ReturnType<typeof setTimeout> | null = null

function stopTimer() {
	if (timer === null) return
	clearTimeout(timer)
	timer = null
}

function setMessage(text: string, action: StatusAction | null = null) {
	generation++
	toast.value = { text, action, generation }

	stopTimer()
	timer = setTimeout(() => {
		timer = null
		toast.value = null
	}, LIFETIME_MS)
}

function clear() {
	stopTimer()
	if (toast.value !== null) toast.value = null
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
		toast: readonly(toast),
		setMessage,
		clear,
	}
}
