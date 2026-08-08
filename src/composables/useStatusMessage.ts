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
 * `Undo` then undoes. A failure takes the pill on the same terms, and is the one
 * kind that does not then leave on its own — see `StatusSeverity`.
 *
 * String policy, since these are read consecutively by the same user: a full
 * sentence takes a terminal period (`Nothing to undo.`), a bare confirmation
 * does not (`Copied 3 notes`). Counts are whole strings per grammatical number —
 * never `note(s)`, and never a number concatenated into a fragment.
 */

/** Something the toast offers to do, as one button. There is at most one: a pill
 *  with a choice in it is a dialog, and this is a thing the reader is meant to
 *  be able to ignore. */
export type StatusAction = { label: string; run: () => void }

/**
 * Whether the message is allowed to leave on its own.
 *
 * The distinction is not decoration: a confirmation the reader missed costs
 * nothing, because the thing it confirms already happened and is visible in the
 * list. A failure the reader missed costs them the knowledge that the action
 * they asked for did *not* happen — the list looks the same either way, so the
 * pill is the only place that difference is written down. So `error` has no
 * timer at all and stands until it is dismissed or replaced.
 */
export type StatusSeverity = 'info' | 'error'

export type StatusToast = {
	text: string
	action: StatusAction | null
	severity: StatusSeverity
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
 *
 * **Five seconds of the reader's attention, not five seconds of wall clock.**
 * The number is a guess at how long it takes to notice a pill and decide about
 * it, so it may only run while there is a reader in front of it. It is held
 * whenever that decision is demonstrably still being made — a pointer on the
 * button, or the keyboard standing on it — and whenever the panel is not on
 * screen at all, which is every Escape to the tray: a message that spent its
 * whole window hidden would be an `Undo` that expired in a tray icon.
 */
const LIFETIME_MS = 5000

/**
 * Why a set of named holds and not a counter.
 *
 * The pointer and the keyboard can be on the button at once, and either can
 * report an entry the browser never pairs with an exit — a button removed under
 * the cursor sends no `pointerleave`. A counter drifts permanently on the first
 * such miss and the pill either never expires or expires while being read;
 * naming each holder makes a repeat of the same one idempotent and lets the next
 * message clear the lot.
 */
type StatusHold = 'pointer' | 'focus' | 'hidden'

let generation = 0
let timer: ReturnType<typeof setTimeout> | null = null
/** What is left of the lifetime: banked while held, `null` when nothing is
 *  counting down — an error, an empty pill, or one already expired. */
let remaining: number | null = null
let deadline = 0
const holds = new Set<StatusHold>()

function stopTimer() {
	if (timer === null) return
	clearTimeout(timer)
	timer = null
}

function expire() {
	timer = null
	remaining = null
	toast.value = null
}

function startTimer(ms: number) {
	stopTimer()
	remaining = ms
	if (holds.size > 0) return
	deadline = Date.now() + ms
	timer = setTimeout(expire, ms)
}

/** Banks the remainder and stops the clock. Safe to call for a hold already
 *  held, and safe with no pill up — there is simply nothing to bank. */
function pause(hold: StatusHold = 'pointer') {
	if (holds.has(hold)) return
	holds.add(hold)
	if (timer === null) return
	stopTimer()
	remaining = Math.max(0, deadline - Date.now())
}

/** Releases one hold, and restarts on the banked remainder once the last one is
 *  gone. A hold released twice is not a second resume. */
function resume(hold: StatusHold = 'pointer') {
	if (!holds.delete(hold)) return
	if (holds.size > 0 || remaining === null || timer !== null) return
	deadline = Date.now() + remaining
	timer = setTimeout(expire, remaining)
}

/**
 * Registered once for the module, and never removed.
 *
 * The composable is a module singleton with one pill for the whole application,
 * so this listener has exactly the lifetime the state it guards does; hanging it
 * off a component's `onMounted` would instead tie the undo window to whichever
 * component happened to be mounted, and unhook it during the one thing it exists
 * to survive — the panel being hidden.
 */
if (typeof document !== 'undefined') {
	document.addEventListener('visibilitychange', () => {
		if (document.hidden) pause('hidden')
		else resume('hidden')
	})
}

/** The only way to be rid of a failure, since it has no clock. Named on the
 *  button rather than left to a close glyph: the pill has no chrome, and a bare
 *  `×` in a strip of text is a smaller target than the word. */
const DISMISS_ACTION: StatusAction = { label: 'Dismiss', run: () => clear() }

/**
 * `severity` decides whether the pill is on a clock, and an `error` never
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
	generation++
	const failed = severity === 'error'
	toast.value = { text, action: failed ? DISMISS_ACTION : action, severity, generation }

	// A new message is a new decision: whatever the last one was being held open
	// for, the reader is looking at something else now.
	holds.delete('pointer')
	holds.delete('focus')

	stopTimer()
	remaining = null
	if (!failed) startTimer(LIFETIME_MS)
}

/** A failure, reported where every other outcome of the same action is. */
function setError(text: string) {
	setMessage(text, null, 'error')
}

function clear() {
	stopTimer()
	remaining = null
	holds.delete('pointer')
	holds.delete('focus')
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
		setError,
		clear,
		pause,
		resume,
	}
}
