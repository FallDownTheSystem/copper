/**
 * How a list animates when rows arrive, leave and move — as `<TransitionGroup>`
 * hooks rather than as an auto-animate plugin.
 *
 * **Why the framework primitive replaced the library.** auto-animate keeps a
 * cache of every row's last known position and animates the *next* change from
 * that cache. The cache is refreshed by heuristics — an IntersectionObserver
 * whose watch box is computed in document coordinates while the coords include
 * the panel's inner scroll offset, a 2-second poll behind a 500ms debounce, and
 * an `updatePos` that awaits the element's last animation's `finished` promise,
 * which a single cancelled animation leaves rejected forever. In Copper's layout
 * every one of those fails: collapsing one section moves every section below it
 * without a mutation in their own rowgroups, so their caches went stale and the
 * next gesture FLIPped them in from positions they left seconds ago (the
 * "section teleports from far below" bug, 2026-08-11). No trigger list can
 * patch that — any layout shift from any cause stales the cache. TransitionGroup
 * has no cache: it measures children in the render pass immediately before each
 * DOM patch and compares after, so a stale position cannot exist.
 *
 * **The division of labour.** Moves ride TransitionGroup's own FLIP, which is
 * CSS-class-driven — `.list-move` in `main.css` carries the duration and curve,
 * and the `moveClass` returned here swaps in a class no stylesheet defines when
 * animation is off, which is the documented way to stand the FLIP down (Vue
 * probes the class for a transform transition and skips the work when it finds
 * none). Enter and leave are Web Animations started by the hooks below under
 * `:css="false"`, because both need JavaScript anyway: the gate is reactive
 * state CSS cannot read, and a leave has to measure the row's height to fold it.
 *
 * **Rows fold and unfold; they do not float.** auto-animate lifted a removed row
 * out of flow with `position: absolute` at its cached coordinates and faded it
 * where it stood. Absolute positioning needs a correct number for "where it
 * stood" — the exact thing the stale cache could not provide — and when a whole
 * section's rows leave in one patch, each out-of-flow row would collapse the
 * static position the next one resolves against, stacking the ghosts. Folding
 * the row's own height to zero needs no coordinates at all, survives any number
 * of simultaneous leaves, and lets the rows below ride the real layout down
 * instead of being FLIPped in parallel. The enter is the same motion reversed,
 * and it carries the expand: the group's growth is nothing but its children's
 * layout, so rows unfolding from zero *is* the section unfolding — a fade alone
 * left the space arriving in one frame with only the paint inside it easing.
 *
 * Written once and shared, because the note list and the composer's attachment
 * tray are the same motion: two copies of the arithmetic is two chances for one
 * list to drift away from the other.
 */

import { EASE_OUT_QUINT_CSS } from '@/lib/motion'

/** Matches `--transition-duration-base` and `.list-move`. A row settling is the
 *  panel's hottest motion and anything slower sits badly under a capture. */
const TIMING: KeyframeAnimationOptions = { duration: 150, easing: EASE_OUT_QUINT_CSS }

/** The in-flight enter or leave of each element, so a cancellation can stop the
 *  exact animation it interrupts rather than everything on the element. */
const running = new WeakMap<Element, Animation>()

/**
 * @param animated - Read per animation, never captured: the gate is reactive
 * state (the reduced-motion pair, and for the note list also `listAnimated` and
 * the drag) and an animation must see its value at the moment it would start.
 */
export function useListTransition(animated: () => boolean) {
	/** happy-dom ships no `Element.animate`, and a leave that never reports
	 *  `finish` would hold its row in the DOM forever — so no engine means no
	 *  animation, reported done immediately, which is also what the test suite
	 *  wants a list change to be. */
	function start(
		el: Element,
		keyframes: Keyframe[],
		options: KeyframeAnimationOptions,
		done: () => void,
		cleanup?: () => void,
	) {
		if (typeof el.animate !== 'function') {
			cleanup?.()
			return done()
		}
		const animation = el.animate(keyframes, options)
		running.set(el, animation)
		// `cancel` as well as `finish`: a drag start finishes in-flight motion via
		// `runningMotions`, but anything that cancels instead must still hand the
		// element back to Vue, or a leaving row is orphaned mid-removal. Vue's own
		// callback is guarded against running twice.
		const settle = () => {
			cleanup?.()
			done()
		}
		animation.addEventListener('finish', settle, { once: true })
		animation.addEventListener('cancel', settle, { once: true })
	}

	/**
	 * The mirror of the leave: the row unfolds from nothing rather than fading
	 * in at full size. The fold pair is what animates a section's expand and
	 * collapse at all — the rows are this group's children, the group's own
	 * growth is just their layout, and everything below rides it. A fade alone
	 * left the expand looking instant (user report, 2026-08-11): the space
	 * arrived in one frame and only the paint inside it eased.
	 */
	function onEnter(el: Element, done: () => void) {
		if (!animated() || !(el instanceof HTMLElement)) return done()
		const { marginTop } = getComputedStyle(el)
		el.style.overflow = 'hidden'
		start(
			el,
			[
				{ height: '0px', marginTop: '0px', opacity: 0 },
				{ height: `${el.offsetHeight}px`, marginTop, opacity: 1 },
			],
			TIMING,
			done,
			// The end keyframe equals the natural layout, so releasing the clip and
			// the fill together is invisible.
			() => {
				el.style.overflow = ''
			},
		)
	}

	function onLeave(el: Element, done: () => void) {
		if (!animated() || !(el instanceof HTMLElement)) return done()
		// The margin folds with the height — it is the row's own share of the gap,
		// granted by the sibling rules in `NoteSection`, and left standing it would
		// hold a 4px seam open until the element vanished.
		const { marginTop } = getComputedStyle(el)
		el.style.overflow = 'hidden'
		start(
			el,
			[
				{ height: `${el.offsetHeight}px`, marginTop, opacity: 1 },
				{ height: '0px', marginTop: '0px', opacity: 0 },
			],
			// Held at the end value: Vue removes the element from the `finish`
			// listener, and without the fill the frame between the animation ending
			// and the removal would flash the row back at full height.
			{ ...TIMING, fill: 'forwards' },
			done,
		)
	}

	/** A cancelled enter is an element on its way out — the leave that follows
	 *  measures and animates it, and the entrance still writing height underneath
	 *  would compose with the fold. */
	function onEnterCancelled(el: Element) {
		running.get(el)?.cancel()
		if (el instanceof HTMLElement) el.style.overflow = ''
	}

	/** A cancelled leave is an element staying after all. Cancelling drops the
	 *  forwards fill, which is what returns the height. */
	function onLeaveCancelled(el: Element) {
		running.get(el)?.cancel()
		if (el instanceof HTMLElement) el.style.overflow = ''
	}

	/** Bound to `move-class`. The off name is defined by no stylesheet on
	 *  purpose: Vue probes the class for a transform transition before doing any
	 *  FLIP work, and an undefined class fails the probe — the supported way to
	 *  disable moves without also disabling enter and leave. */
	const moveClass = computed(() => (animated() ? 'list-move' : 'list-move-off'))

	return { moveClass, onEnter, onLeave, onEnterCancelled, onLeaveCancelled }
}
