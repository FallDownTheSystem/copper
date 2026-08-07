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

/** The same curve for anything that takes a CSS easing string — auto-animate. */
export const EASE_OUT_QUINT_CSS = 'cubic-bezier(0.22, 1, 0.36, 1)'
