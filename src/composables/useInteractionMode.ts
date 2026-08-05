/**
 * Task-004's `F2` interaction mode, lifted to module scope.
 *
 * Every interactive descendant of a row is `tabindex="-1"` so the grid is one
 * Tab stop, which leaves `F2` as the only way to reach the completion circle,
 * `Show more` and rendered links. It lives here rather than inside `NoteList`
 * because it is a rung of the `Escape` ladder, and that ladder is one ordered
 * handler on the shell — it cannot reach a component-local ref.
 */

import { rowElement } from './useSelection'

const interactionRowId = ref<string | null>(null)

/** `pre[tabindex]` is in the list because a Shiki fence is a scroll container:
 *  it has to be reachable to be scrolled by keyboard, and it carries
 *  `tabindex="-1"` in navigation mode so it is not a second Tab stop. */
export function focusableIn(row: HTMLElement) {
	return [...row.querySelectorAll<HTMLElement>('button, a[href], pre[tabindex]')]
}

/**
 * Anchors inside rendered Markdown carry `tabindex="-1"` from a render rule, so
 * they have to be flipped on the DOM rather than through a prop — the HTML
 * string is not Vue's to patch.
 */
function setDescendantsTabbable(row: HTMLElement | null, tabbable: boolean) {
	if (!row) return
	for (const element of focusableIn(row)) element.tabIndex = tabbable ? 0 : -1
}

function enter(rowId: string | null) {
	if (!rowId) return
	interactionRowId.value = rowId
	void nextTick(() => {
		const row = rowElement(rowId)
		setDescendantsTabbable(row, true)
		if (row) focusableIn(row)[0]?.focus()
	})
}

function exit() {
	const key = interactionRowId.value
	if (!key) return
	setDescendantsTabbable(rowElement(key), false)
	interactionRowId.value = null
	void nextTick(() => rowElement(key)?.focus())
}

/** A row that survived a document change may have been re-rendered, which resets
 *  the tabindex of anchors inside `v-html` — those are set on the DOM, not by
 *  Vue — so they are re-promoted. A row that is actually gone forces an exit. */
function reconcile() {
	const key = interactionRowId.value
	if (!key) return
	void nextTick(() => {
		const row = rowElement(key)
		if (!row) exit()
		else setDescendantsTabbable(row, true)
	})
}

export function useInteractionMode() {
	return {
		interactionRowId: readonly(interactionRowId),
		enter,
		exit,
		reconcile,
	}
}
