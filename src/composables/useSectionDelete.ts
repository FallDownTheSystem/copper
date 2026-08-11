/**
 * The keyboard path's section-delete confirmation.
 *
 * Module-scoped for the reason the rename session is: exactly one confirmation
 * is open at a time, and the state has to outlive any single header row, which
 * a document applied mid-confirm re-creates.
 *
 * State and pure transitions only — the `delete_section` call is made by the
 * component through `useNoteActions.removeSection`, exactly as the rename
 * field writes through `useSpace`. That is what keeps this file out of an
 * import cycle with the coordinator that reconciles it.
 *
 * The context menu's delete deliberately does not come through here: a menu
 * item is already a deliberate second gesture, and the whole operation is one
 * undo either way. This confirmation exists because a bare Delete on a focused
 * header is a single keypress away from a section and everything in it.
 */

import type { SpaceView } from './useSpace'

const confirming = ref<string | null>(null)

/**
 * The armed offer's identity: *which* notes the question covers, not how many.
 * The rendered count is live either way; what must not survive is a question
 * armed over one set answering for another — a capture landing mid-confirm is
 * the reachable case, exactly as it was for the done-purge popover.
 *
 * Sorted before joining, so a reorder within the section is not a different
 * offer. The separator is NUL because a note id cannot contain one — and it is
 * **spelled `\u0000`, never a literal byte**, for `DoneFilter`'s reason: a raw
 * control byte makes git classify the file as binary, and nothing in the gates
 * would catch it.
 */
let armedOffer = ''

function offerOf(noteIds: readonly string[]) {
	return [...noteIds].sort().join('\u0000')
}

function beginConfirm(sectionId: string, noteIds: readonly string[]) {
	confirming.value = sectionId
	armedOffer = offerOf(noteIds)
}

function closeConfirm() {
	confirming.value = null
	armedOffer = ''
}

/** Called by `useSpace` on every applied document: a section deleted away
 *  underneath the question, a space swapped, or a note added to or removed
 *  from the offer all withdraw it — the user must answer the question that is
 *  actually on screen, and the count in it just changed. */
function reconcile(space: SpaceView | null) {
	const id = confirming.value
	if (id === null) return

	if (!space?.sections.some((section) => section.id === id)) {
		closeConfirm()
		return
	}

	const ids = space.notes.filter((note) => note.section === id).map((note) => note.id)
	if (offerOf(ids) !== armedOffer) closeConfirm()
}

export function useSectionDelete() {
	return {
		confirming: readonly(confirming),
		beginConfirm,
		closeConfirm,
		reconcile,
	}
}
