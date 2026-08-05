/**
 * The inline edit **session** — module-scoped, because it has to outlive the row
 * that opened it.
 *
 * A `NoteEditor` rendered inside `NoteCard` cannot satisfy "the draft is
 * preserved on screen": if the note disappears from the document the row
 * unmounts and takes the editor with it, and Vue will not preserve a keyed
 * component moved between two different parents either, so moving an edited note
 * between sections destroys it the same way. A deleted note's session is
 * rendered as a recovery row outside the live-note iteration instead.
 *
 * This module holds state and pure transitions only. The `edit_note` call is
 * made by the component through `useSpace`, which keeps the single-adapter rule
 * intact and keeps this file testable with no IPC mocking.
 */

import type { NoteView, SpaceView } from './useSpace'

export type EditSession = {
	/** Space identity at the time editing began. A different document means the
	 *  note id addresses something else entirely. */
	spaceId: string
	noteId: string
	originSection: string
	originIndex: number
	/** The body this draft was forked from — the comparison point for detecting
	 *  an external change. */
	baseBody: string
	draft: string
	/** Bumped on every keystroke. Captured at submit time so a result that lands
	 *  after the user typed more cannot close the editor over newer input. */
	draftRevision: number
	composing: boolean
	/** The body as it now stands on disk, when it changed under the draft. */
	conflict: string | null
	deleted: boolean
	pending: boolean
}

const session = ref<EditSession | null>(null)

const isEditing = (noteId: string) =>
	session.value?.noteId === noteId && !session.value.deleted && session.value.conflict === null

/** A live editing row: the note still exists and is not conflicted. */
const editingNoteId = computed(() =>
	session.value && !session.value.deleted ? session.value.noteId : null,
)

/** A draft whose note was deleted externally, rendered outside the live rows. */
const recovery = computed(() => (session.value?.deleted ? session.value : null))

const conflicted = computed(() => (session.value?.conflict !== null ? session.value : null))

/** Commit is blocked while conflicted: without that, a later blur or Ctrl+Enter
 *  calls `edit_note` and overwrites the external body — exactly the data loss
 *  the conflict state exists to prevent. */
const canCommit = computed(
	() => session.value !== null && session.value.conflict === null && !session.value.deleted,
)

function beginEdit(space: SpaceView, note: NoteView) {
	session.value = {
		spaceId: space.id,
		noteId: note.id,
		originSection: note.section,
		originIndex: space.notes.findIndex((candidate) => candidate.id === note.id),
		baseBody: note.body,
		draft: note.body,
		draftRevision: 0,
		composing: false,
		conflict: null,
		deleted: false,
		pending: false,
	}
}

function setDraft(value: string) {
	if (!session.value) return
	session.value.draft = value
	session.value.draftRevision++
}

/**
 * Tracked explicitly rather than read off the event, because the guard is also
 * needed on blur and unmount — and a `FocusEvent` carries neither `isComposing`
 * nor `keyCode`.
 */
function setComposing(composing: boolean) {
	if (session.value) session.value.composing = composing
}

function cancel() {
	session.value = null
}

/** Captured at submit time; `finishCommit` compares against it. */
function beginCommit() {
	if (!session.value) return null
	session.value.pending = true
	return { body: session.value.draft, revision: session.value.draftRevision }
}

/**
 * Closes the editor only if the field is unchanged since the request was
 * issued. Both text surfaces stay editable while pending, so a user can type
 * after committing, and a success must not destroy that newer input.
 */
function finishCommit(revision: number, ok: boolean) {
	const current = session.value
	if (!current) return
	current.pending = false
	if (!ok) return

	if (current.draftRevision === revision) session.value = null
	else current.baseBody = current.draft
}

// --- external change ---------------------------------------------------------

/**
 * Called by `useSpace` on every applied document.
 *
 * A DOM-replacing reload must never fire a blur-commit of a stale draft, so the
 * session is updated here rather than being torn down and rebuilt.
 */
function reconcile(space: SpaceView | null, identityChanged: boolean) {
	const current = session.value
	if (!current) return

	if (!space || identityChanged) {
		// Ids are unique only within a document. The draft is not re-associated
		// with a coincidentally matching id in the new one — it becomes a detached
		// recovery surface.
		current.deleted = true
		current.conflict = null
		return
	}

	const note = space.notes.find((candidate) => candidate.id === current.noteId)
	if (!note) {
		current.deleted = true
		current.conflict = null
		return
	}

	current.deleted = false
	current.originSection = note.section

	if (note.body === current.baseBody) {
		// Unchanged externally — the draft survives untouched.
		current.conflict = null
		return
	}

	if (note.body === current.draft) {
		// The file caught up with what we were going to write. Nothing to resolve.
		current.baseBody = note.body
		current.conflict = null
		return
	}

	current.conflict = note.body
}

/** Discard the draft and leave the note as the file has it. */
function resolveUseExternal() {
	session.value = null
}

/**
 * Write the draft over the external body, explicitly. Returns what to write;
 * the caller performs the `edit_note` and reports back through `finishCommit`.
 */
function resolveKeepMine() {
	const current = session.value
	if (!current || current.conflict === null) return null
	current.conflict = null
	current.baseBody = current.draft
	return beginCommit()
}

function dismissRecovery() {
	session.value = null
}

export function useNoteEditor() {
	return {
		session: readonly(session),
		editingNoteId,
		recovery,
		conflicted,
		canCommit,
		isEditing,
		beginEdit,
		setDraft,
		setComposing,
		cancel,
		beginCommit,
		finishCommit,
		reconcile,
		resolveUseExternal,
		resolveKeepMine,
		dismissRecovery,
	}
}
