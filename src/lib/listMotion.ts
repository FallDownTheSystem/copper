/**
 * How a list animates when rows arrive, leave and move — as an auto-animate
 * *plugin* rather than as its options object.
 *
 * **Why the plugin form, when the options form is one line.** Passing
 * `{ duration, easing }` only reaches the FLIP that moves the rows already on
 * screen. The library hard-codes the other two: an arrival runs at
 * `duration * 1.5` on `ease-in` through a keyframe list that holds the row
 * invisible for the first half of it, and a removal runs on `ease-out`. So the
 * 150ms asked for became a 225ms entrance whose first 112ms showed nothing —
 * the row appeared to hesitate, then snap in. A plugin is the only way the
 * library offers to author all three, so all three are authored here.
 *
 * Written once and shared, because the note list and the composer's attachment
 * tray are the same motion: two copies of the FLIP arithmetic below is two
 * chances for one list to drift away from the other.
 */

import { getTransitionSizes, type AutoAnimationPlugin } from '@formkit/auto-animate'

import { EASE_OUT_QUINT_CSS } from './motion'

/** Matches `--duration-base`. A row settling is the panel's hottest motion and
 *  the library's own 250ms default is too slow to sit under a capture. */
const TIMING: KeyframeAnimationOptions = { duration: 150, easing: EASE_OUT_QUINT_CSS }

/** The row grows the last 2% into place rather than sliding: nothing in this
 *  list arrives from a direction, so nothing may leave in one. */
const HIDDEN: Keyframe = { opacity: 0, transform: 'scale(0.98)' }
const SHOWN: Keyframe = { opacity: 1, transform: 'scale(1)' }

/**
 * The third and fourth arguments are `(oldCoords, newCoords)` in that order,
 * which is what the library passes and what its documentation shows — its own
 * type declaration names them the other way round, and the names are the only
 * part that is wrong.
 */
export const listMotion: AutoAnimationPlugin = (el, action, oldCoords, newCoords) => {
	if (action === 'add') return new KeyframeEffect(el, [HIDDEN, SHOWN], TIMING)
	if (action === 'remove') return new KeyframeEffect(el, [SHOWN, HIDDEN], TIMING)

	// `remain` is the FLIP, and it has to be reproduced rather than delegated:
	// choosing the plugin form opts out of the library's own arithmetic for every
	// action at once. This is that arithmetic, with only the timing changed.
	// Unreachable — the library always measures both before it asks — but the
	// signature admits it, and an empty effect is the honest answer to "animate a
	// move you cannot describe": it finishes immediately and the row stays put.
	if (!oldCoords || !newCoords) return new KeyframeEffect(el, [], { duration: 0 })

	let deltaLeft = oldCoords.left - newCoords.left
	let deltaTop = oldCoords.top - newCoords.top
	const deltaRight = oldCoords.left + oldCoords.width - (newCoords.left + newCoords.width)
	const deltaBottom = oldCoords.top + oldCoords.height - (newCoords.top + newCoords.height)
	// An edge that did not move belongs to a row anchored on that side, which is
	// therefore not travelling along that axis however much its box changed.
	if (deltaBottom === 0) deltaTop = 0
	if (deltaRight === 0) deltaLeft = 0

	// Box-sizing aware, which is why it is the library's helper and not a
	// subtraction: a `content-box` row would otherwise be animated between two
	// numbers that include its padding.
	const [widthFrom, widthTo, heightFrom, heightTo] = getTransitionSizes(el, oldCoords, newCoords)

	const from: Keyframe = { transform: `translate(${deltaLeft}px, ${deltaTop}px)` }
	const to: Keyframe = { transform: 'translate(0, 0)' }
	if (widthFrom !== widthTo) {
		from.width = `${widthFrom}px`
		to.width = `${widthTo}px`
	}
	if (heightFrom !== heightTo) {
		from.height = `${heightFrom}px`
		to.height = `${heightTo}px`
	}

	return new KeyframeEffect(el, [from, to], TIMING)
}
