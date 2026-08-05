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

/** `Copied 1 note` / `Copied 3 notes` — whole strings, one per number. */
export function noteCountLabel(verb: string, count: number) {
	return count === 1 ? `${verb} 1 note` : `${verb} ${count} notes`
}

export function useStatusMessage() {
	return {
		message: readonly(message),
		setMessage,
		clear,
	}
}
