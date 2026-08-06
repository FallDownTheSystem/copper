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

/** Shallow because nothing here mutates a Set in place: every change below
 *  builds a replacement and assigns it. A deep `ref` wraps each Set in a
 *  reactive proxy and makes every `has()` register a per-key dependency —
 *  bookkeeping for a mutation that never happens, paid once per note per
 *  `ResizeObserver` callback, and redundant besides: replacing the ref already
 *  notifies everything reading it. */
const expandable = shallowRef(new Set<string>())
const expanded = shallowRef(new Set<string>())

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
 * @param clampPx       the resolved `--note-clamp`
 */
function measure(noteId: string, contentHeight: number, clampPx: number) {
	const overflowing = contentHeight - clampPx > EPSILON_PX

	// Retained while expanded, or `Show less` removes itself with no way back.
	if (!overflowing && expanded.value.has(noteId)) return
	// The copy is only worth making when membership actually changes. This runs
	// once per note per ResizeObserver callback, and copying a 200-entry Set to
	// discover nothing moved is the cost of every reflow.
	if (overflowing === expandable.value.has(noteId)) return

	const next = new Set(expandable.value)
	if (overflowing) next.add(noteId)
	else next.delete(noteId)
	expandable.value = next
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
