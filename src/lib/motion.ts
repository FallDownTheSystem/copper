/**
 * The panel's one easing curve, in the two forms its consumers need.
 *
 * Three call sites had grown three near-identical curves — a local `EASE` in
 * `App.vue`, an inline tuple in `Checkbox.vue`, and a bare `ease-out` string in
 * `NoteSection.vue` — which is three chances for the family to drift apart and
 * no place to change it once. A CSS custom property cannot serve the two
 * JavaScript consumers: motion-v wants the control points as numbers, so a token
 * would have to be hand-synced with a constant anyway.
 */

/** Control points for motion-v / WAAPI, which take the curve as numbers. */
export const EASE_OUT_QUINT = [0.22, 1, 0.36, 1] as const

/** The same curve for anything that takes a CSS easing string — the list
 *  transition's Web Animations keyframes, and the drag's settle-home. */
export const EASE_OUT_QUINT_CSS = 'cubic-bezier(0.22, 1, 0.36, 1)'

/**
 * The animations running under `root` that are **motion** — driven by the
 * document's clock, and therefore on their way to an end.
 *
 * Two callers ask the same question of the same list: `useNoteDrag` finishes
 * what is in flight before it measures a row, and `useSelection` waits for it to
 * quiet before it stops pinning the region to the bottom. Both used to walk
 * `getAnimations({ subtree: true })` themselves, and both were wrong in the same
 * way, so the walk lives here once.
 *
 * **What it excludes is the point: a scroll-driven animation is not motion.**
 * The section band erases every row that slides under the pinned heading with a
 * `clip-path` keyframe on the row's own `view()` timeline (see `.section-band` in
 * `main.css`). That animation is geometry expressed as a keyframe — it is
 * permanently `running`, it never ends, and its progress is a function of the
 * scroll offset and nothing else. Treating it as motion breaks both callers, and
 * the drag's way was the worse one: `finish()` parks a progress-based animation
 * at its end value and detaches it from the timeline for good, which for this
 * keyframe is `inset(100% 0 0 0)` — the row is clipped away entirely and no
 * amount of scrolling brings it back. One drag erased every row in the list.
 *
 * A null timeline counts as time-driven, which costs nothing: such an animation
 * is never `running`, so the first test has already excluded it.
 */
export function runningMotions(root: Element): Animation[] {
	const animations = root.getAnimations?.({ subtree: true }) ?? []
	return animations.filter(
		(animation) =>
			animation.playState === 'running' &&
			(animation.timeline == null || animation.timeline === document.timeline),
	)
}
