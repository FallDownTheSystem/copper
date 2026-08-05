/**
 * The inline section-rename session.
 *
 * Module-scoped for the same reason the note editor is: exactly one rename is
 * open at a time, and the state has to outlive any single header row, which a
 * document applied mid-rename re-creates.
 *
 * State and pure transitions only — the `rename_section` call is made by the
 * component through `useSpace`, exactly as `useNoteEditor` does. That keeps the
 * single-adapter rule intact and, more usefully here, keeps this file out of an
 * import cycle with the coordinator that reconciles it.
 *
 * This is the inline-input pattern task-007 reuses for section *creation*, so
 * the shape matters beyond this file: open by id, edit a draft, commit on Enter
 * or blur, cancel on Escape, and never write an empty name.
 */

import type { SpaceView } from './useSpace'

const renaming = ref<string | null>(null)
const draft = ref('')

function beginRename(sectionId: string, currentName: string) {
	renaming.value = sectionId
	draft.value = currentName
}

function setDraft(value: string) {
	draft.value = value
}

/**
 * Ends the session and reports what to write, or `null` when there is nothing
 * to write — an unchanged name, or a field the user cleared and gave up on.
 * Clearing a field can never destroy a section.
 */
function endRename(currentName: string): { id: string; name: string } | null {
	const id = renaming.value
	const name = draft.value.trim()
	renaming.value = null
	draft.value = ''

	if (id === null || name.length === 0 || name === currentName) return null
	return { id, name }
}

function cancelRename() {
	renaming.value = null
	draft.value = ''
}

/** Called by `useSpace` on every applied document: a section renamed away
 *  underneath the field, deleted, or a space swapped, leaves nothing to edit. */
function reconcile(space: SpaceView | null) {
	const id = renaming.value
	if (id === null) return
	if (!space?.sections.some((section) => section.id === id)) cancelRename()
}

export function useSectionEditor() {
	return {
		renaming: readonly(renaming),
		draft: readonly(draft),
		beginRename,
		setDraft,
		endRename,
		cancelRename,
		reconcile,
	}
}
