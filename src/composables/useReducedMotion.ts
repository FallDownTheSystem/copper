/**
 * `true` when animation must not run — because the OS asks for reduced motion,
 * or because Copper's own `motion` setting is `off`.
 *
 * The `prefers-reduced-motion` block in `main.css` can only zero CSS transitions
 * and animations, so nothing driven by JavaScript is covered by it. `motion-v`
 * renders through the Web Animations API, which means every animation this task
 * adds is invisible to that block and has to consult the preference here
 * instead. Ported from the reference app, which had the composable but — the bug
 * this port fixes — only its radio actually called it.
 *
 * **The two sources are OR-ed rather than chosen between, and that is the whole
 * design.** It makes the setting reduce-only structurally: no value of `motion`
 * can animate against an OS `prefers-reduced-motion: reduce`, because no value
 * can subtract from an OR. A three-way "force on" would have to be a different
 * shape, which is the point — that preference is an accessibility signal and an
 * app setting is not entitled to override it.
 */

import { useSettings } from './useSettings'

export function useReducedMotion() {
	const preference = usePreferredReducedMotion()
	const { motionPreference } = useSettings()
	return computed(() => preference.value === 'reduce' || motionPreference.value === 'off')
}
