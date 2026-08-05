/**
 * Per-note truncation state, kept out of `NoteCard` so it survives a re-render
 * and so task-006 can drive it from a menu without inventing a second
 * mechanism.
 *
 * `canExpand` and `expanded` are **separate** state, and that separation is the
 * whole point. Measuring overflow by comparing the clamped element's
 * `scrollHeight` to its `clientHeight` works exactly until it succeeds: once
 * expanded the two are equal, `canExpand` flips false, and `Show less`
 * disappears with no way back. So the measurement runs against an inner
 * *unconstrained* element, and the result is retained while `expanded` holds.
 */

/** At fractional Windows display scaling the two heights differ by sub-pixel
 *  amounts on content that does not actually overflow. */
const EPSILON_PX = 1

const expandable = ref(new Set<string>())
const expanded = ref(new Set<string>())

/**
 * `--note-clamp` resolved to pixels.
 *
 * Resolved once from a single probe element in `PanelShell` rather than per
 * card: the token is the same for every note, and `getComputedStyle` on a
 * custom property returns the unevaluated `calc()` expression, so it has to be
 * measured off a real box. Re-measuring per card would be one probe per row.
 */
const clampHeight = ref(0)

function setClampHeight(px: number) {
	if (px > 0 && px !== clampHeight.value) clampHeight.value = px
}

function canExpand(noteId: string) {
	return expandable.value.has(noteId)
}

function isExpanded(noteId: string) {
	return expanded.value.has(noteId)
}

/**
 * Called from a `ResizeObserver` callback, which must write state and never
 * style in the same synchronous block.
 *
 * @param contentHeight height of the unconstrained content element
 * @param clampHeight   the resolved `--note-clamp`
 */
function measure(noteId: string, contentHeight: number, clampHeight: number) {
	const overflowing = contentHeight - clampHeight > EPSILON_PX
	const next = new Set(expandable.value)

	if (overflowing) next.add(noteId)
	else if (expanded.value.has(noteId))
		return // retained: see the header comment
	else next.delete(noteId)

	if (next.size !== expandable.value.size || overflowing !== expandable.value.has(noteId)) {
		expandable.value = next
	}
}

function toggle(noteId: string) {
	const next = new Set(expanded.value)
	if (!next.delete(noteId)) next.add(noteId)
	expanded.value = next
}

/** Space identity changed: ids address a different document now. */
function reset() {
	expandable.value = new Set()
	expanded.value = new Set()
}

export function useNoteDisclosure() {
	return {
		canExpand,
		isExpanded,
		clampHeight: readonly(clampHeight),
		setClampHeight,
		measure,
		toggle,
		reset,
	}
}
