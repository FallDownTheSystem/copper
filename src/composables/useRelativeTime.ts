/**
 * The one clock every note footer reads.
 *
 * A relative timestamp is a function of two instants, and only one of them is in
 * the document — so "2m ago" is wrong the moment a minute passes and nothing
 * re-renders. Something has to move, and the shape of that something is the whole
 * of this file: **one interval for the whole list, not one per card.** A panel
 * holds a few hundred notes; a timer per footer is a few hundred timers, each
 * waking the same webview to change the same two characters.
 *
 * The ref is what the cards depend on, so a tick invalidates every footer's
 * computed at once and Vue patches the ones whose text actually changed.
 */

import { useSettings } from './useSettings'

/**
 * 30 seconds, and the number is bounded on both sides rather than picked.
 *
 * The smallest unit a footer shows for more than a moment is the minute, so
 * anything under a minute is enough to keep the *displayed* value honest —
 * a tick can leave "5m ago" standing for at most this long into the sixth
 * minute. Going slower widens exactly that error; going faster buys nothing a
 * reader can see, and this wakes the whole list.
 */
const TICK_MS = 30_000

/** Epoch milliseconds, replaced on every tick. `shallowRef` because a number has
 *  nothing to make reactive below its own identity. */
const now = shallowRef(Date.now())

let installed = false

function install() {
	if (installed) return
	installed = true

	/**
	 * A detached scope, for the reason `useSounds` records at greater length: this
	 * runs inside whichever component's `setup()` reached it first — a `NoteCard`,
	 * which unmounts every time the list re-renders around it — and a watcher or an
	 * interval registered there would be disposed with that card while `installed`
	 * stayed true. The clock would stop on the first card to leave the DOM.
	 */
	effectScope(true).run(() => {
		const { showCreated } = useSettings()
		const { pause, resume } = useIntervalFn(() => (now.value = Date.now()), TICK_MS, {
			immediate: false,
		})

		/**
		 * **Nothing ticks while the footers are hidden.** `showCreated` ships off, so
		 * for a user who never turns it on this composable costs one watcher and no
		 * timer at all — which is the point of gating it here rather than letting the
		 * cards decide, since the cards do not know about each other.
		 *
		 * The read on the way in is not redundant with the interval. Turning the
		 * setting on can happen an arbitrarily long time after the last tick, and the
		 * first interval fires `TICK_MS` later — so without it the footers would
		 * appear carrying whatever "now" was when the clock was last running.
		 */
		watch(
			showCreated,
			(shown) => {
				if (!shown) {
					pause()
					return
				}
				now.value = Date.now()
				resume()
			},
			{ immediate: true },
		)
	})
}

export function useRelativeTime() {
	install()
	return { now }
}
