/**
 * The one-line status region above the composer.
 *
 * Separate from task-005's `useCaptureNotice`, deliberately. That one is a
 * capture-specific band driven by `capture://failed` / `capture://cleared` and
 * torn down on a timer; reusing it would give a copy confirmation the capture
 * notice's event source and its 1500 ms lifetime. This one is written by
 * in-panel actions and cleared by the *next user action* rather than by a timer,
 * which is why nothing here schedules anything.
 *
 * String policy, since these are read consecutively by the same user: a full
 * sentence takes a terminal period (`Nothing to undo.`), a bare confirmation
 * does not (`Copied 3 notes`). Counts are whole strings per grammatical number —
 * never `note(s)`, and never a number concatenated into a fragment.
 */

const message = ref<string | null>(null)

function setMessage(text: string) {
	message.value = text
}

function clear() {
	if (message.value !== null) message.value = null
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
		message: readonly(message),
		setMessage,
		clear,
	}
}
