/**
 * The single facade the context menu, the chord layer and the card all call, so
 * no caller has to know which composable backs which action.
 *
 * It resolves targets, performs the store call, sets the status message and
 * fixes up selection and focus. Editing and the handoff are **delegations** —
 * `useNoteEditor` and `useEditorHandoff` own that state, and nothing here
 * duplicates it.
 */

import { buildCopyMarkdown, buildListMarkdown } from '@/lib/noteMarkdown'

import { useSystemClipboard } from './useSystemClipboard'
import { useEditorHandoff } from './useEditorHandoff'
import { useNoteDisclosure } from './useNoteDisclosure'
import { useNoteEditor } from './useNoteEditor'
import { useNoteSearch } from './useNoteSearch'
import { focusRowSoon, noteRow, rowElement, useSelection } from './useSelection'
import { noteCountLabel, useStatusMessage } from './useStatusMessage'
import { useSpace } from './useSpace'

const space = useSpace()
const selection = useSelection()
const search = useNoteSearch()
const clipboard = useSystemClipboard()
const editor = useNoteEditor()
const handoff = useEditorHandoff()
const disclosure = useNoteDisclosure()
const status = useStatusMessage()

/**
 * **The one target rule, used by every action in this file.**
 *
 * The selection when the focused note is part of it, and the focused note alone
 * otherwise — *not* "the selection whenever it is non-empty". The two differ
 * exactly when focus sits outside the selection, and the looser reading would
 * let `Ctrl+Enter` open a note other than the card the user is looking at.
 *
 * Materialised by walking `visibleNoteIds`, which buys two properties at once:
 * canonical document order rather than `Set` insertion order, and — because that
 * order is already filtered by the active query — targets that can never include
 * a note the user cannot see.
 */
function targetIds(): string[] {
	const order = selection.visibleNoteIds.value
	const focused = selection.focusedNoteId.value

	if (focused !== null && selection.selectedIds.value.includes(focused)) {
		const selected = new Set(selection.selectedIds.value)
		return order.filter((id) => selected.has(id))
	}

	return focused !== null && order.includes(focused) ? [focused] : []
}

function targetNotes() {
	return space.notesByIds(targetIds())
}

const targetCount = computed(() => targetIds().length)

/** `Mark as Done` names the action it performs, so it flips only when there is
 *  nothing left to mark. */
const everyTargetDone = computed(() => {
	const notes = targetNotes()
	return notes.length > 0 && notes.every((note) => note.done)
})

const canMerge = computed(() => targetCount.value >= 2)
const canMoveTo = computed(() => space.sections.value.length >= 2)

/**
 * `Expand` and `Edit` are single-note actions with no meaningful batch form, so
 * they take the *focused* note rather than the target set. AC10 has already made
 * the right-clicked card the focused one, so the menu and the keyboard agree.
 */
function focusedTarget(): string | null {
	const id = selection.focusedNoteId.value
	return id !== null && selection.visibleNoteIds.value.includes(id) ? id : null
}

/** The `Expand` item drives the same disclosure the `Show more` button does, so
 *  it is unavailable for exactly the notes that button is absent from. */
const canExpandTarget = computed(() => {
	const id = focusedTarget()
	return id !== null && disclosure.canExpand(id)
})

/** Every section already holding all of the targets — the `Move to` entries that
 *  would do nothing. */
function isRedundantTarget(sectionId: string) {
	const notes = targetNotes()
	return notes.length > 0 && notes.every((note) => note.section === sectionId)
}

// --- copy --------------------------------------------------------------------

async function copyBodies(build: (bodies: readonly string[]) => string) {
	const notes = targetNotes()
	if (notes.length === 0) return

	const written = await clipboard.writeText(build(notes.map((note) => note.body)))
	status.setMessage(
		written ? noteCountLabel('Copied', notes.length) : 'Couldn’t write to the clipboard.',
	)
}

function copyNotes() {
	return copyBodies(buildCopyMarkdown)
}

function copyAsList() {
	return copyBodies(buildListMarkdown)
}

// --- mark as done ------------------------------------------------------------

/**
 * One `set_notes_done` call whatever the count, so marking five notes is one
 * undo. On a mixed selection everything becomes done; only when everything is
 * already done does it flip the other way.
 */
async function toggleDone() {
	const notes = targetNotes()
	if (notes.length === 0) return
	await space.setNotesDone(
		notes.map((note) => note.id),
		!notes.every((note) => note.done),
	)
}

// --- move to -----------------------------------------------------------------

async function moveTo(sectionId: string) {
	const ids = targetIds()
	if (ids.length === 0) return

	const result = await space.moveNotes(ids, sectionId)
	if (!result) return

	// They stay selected — `move_notes` preserves relative order and appends them
	// to the end of the target — and focus lands on the first of them.
	const first = ids[0]
	if (first === undefined) return
	selection.focusRow(noteRow(first))
	focusRowSoon(noteRow(first))
}

// --- merge -------------------------------------------------------------------

async function merge() {
	const ids = targetIds()
	if (ids.length < 2) {
		status.setMessage('Select two or more notes to merge.')
		return
	}

	const result = await space.mergeNotes(ids)
	if (!result) return

	// The survivor is whichever comes first in canonical order — task-003 decides
	// that, and `ids` is already in canonical order, so this reads the answer
	// rather than recomputing it.
	const survivor = ids[0]
	if (survivor === undefined) return
	selection.select(survivor)
	focusRowSoon(noteRow(survivor))
}

// --- delete ------------------------------------------------------------------

async function deleteNotes() {
	const ids = targetIds()
	if (ids.length === 0) return

	const result = await space.deleteNotes(ids)
	// Selection and focus are not fixed up here: task-004's reconciliation has
	// already pruned the dead ids and moved focus to the nearest survivor. A
	// second mechanism would only compete with it.
	if (result) status.setMessage(`${noteCountLabel('Deleted', ids.length)} · Ctrl+Z to undo`)
}

// --- reorder -----------------------------------------------------------------

/**
 * Reordering is refused while a search is active, and the reason is arithmetic
 * rather than taste: the rendered list is a *subset* of its section, so an index
 * read off it means something different from the `index` `reorder_note` takes,
 * which counts positions in the whole section. Dropping a note between two
 * matches would silently move it somewhere else entirely.
 *
 * The drag handle is hidden while a query is active, so this guards the keyboard
 * path and anything that slips past that.
 */
function reorderBlockedBySearch(): boolean {
	if (!search.hasQuery.value) return false
	status.setMessage('Clear the search to reorder notes.')
	return true
}

/**
 * Commits a completed drag by reading the list back out of the DOM.
 *
 * The DOM after a drop already *is* the intended final order, and `reorder_note`
 * interprets `index` against the target section with the note removed — which is
 * exactly the position it occupies in that final order. So this needs no diff
 * against the previous order and no knowledge of where the drag started.
 */
async function finishDrag(noteId: string) {
	if (reorderBlockedBySearch()) return
	const row = rowElement(noteRow(noteId))
	const group = row?.closest<HTMLElement>('[data-section-id]')
	const sectionId = group?.dataset.sectionId
	if (!group || sectionId === undefined) return

	const index = [...group.querySelectorAll<HTMLElement>('[data-note-row]')].findIndex(
		(element) => element.dataset.rowId === noteRow(noteId),
	)
	if (index === -1) return

	const note = space.noteById(noteId)
	// A drag that changed nothing must not push an undo entry.
	if (note && note.section === sectionId && positionOf(noteId) === index) return

	const result = await space.reorderNote(noteId, sectionId, index)
	if (!result) return

	selection.select(noteId)
	focusRowSoon(noteRow(noteId))
}

/** The note's index within its own section in the *document*, which is what a
 *  no-op drag has to be compared against. */
function positionOf(noteId: string): number {
	for (const group of selection.visibleGroups.value) {
		const at = group.noteIds.indexOf(noteId)
		if (at !== -1) return at
	}
	return -1
}

/**
 * The keyboard equivalent of a drag, since every action has to be reachable
 * without a pointer. At a section boundary it crosses into the neighbouring
 * section rather than stopping, which is what the drag does too.
 */
async function moveFocusedBy(delta: number) {
	const noteId = selection.focusedNoteId.value
	if (noteId === null || reorderBlockedBySearch()) return

	const groups = selection.visibleGroups.value
	const groupIndex = groups.findIndex((group) => group.noteIds.includes(noteId))
	const group = groups[groupIndex]
	if (!group) return

	const at = group.noteIds.indexOf(noteId)
	const next = at + delta

	let sectionId = group.sectionId
	let index = next

	if (next < 0 || next >= group.noteIds.length) {
		const neighbour = groups[groupIndex + delta]
		if (!neighbour) return
		sectionId = neighbour.sectionId
		// Entering from above lands at the top; entering from below lands at the
		// bottom — the note keeps travelling in the direction it was going.
		index = delta > 0 ? 0 : neighbour.noteIds.length
	}

	const result = await space.reorderNote(noteId, sectionId, index)
	if (result) focusRowSoon(noteRow(noteId))
}

// --- expand and edit ---------------------------------------------------------

function expand() {
	const id = focusedTarget()
	if (id !== null) disclosure.toggle(id)
}

function edit() {
	const id = focusedTarget()
	const current = space.space.value
	if (id === null || !current) return

	const note = current.notes.find((candidate) => candidate.id === id)
	if (note) editor.beginEdit(current, note)
}

// --- editor handoff ----------------------------------------------------------

async function openInEditor() {
	const ids = targetIds()
	if (ids.length === 0) return
	if (ids.length > 1) {
		status.setMessage('Editing in an external editor works on one note at a time.')
		return
	}

	const id = ids[0]
	if (id === undefined) return

	const outcome = await handoff.openInEditor(id)
	switch (outcome.kind) {
		case 'opened':
			return
		case 'no-editor':
			status.setMessage(
				'Couldn’t open an editor. Set the EDITOR environment variable to your editor’s path.',
			)
			return
		case 'at-capacity':
			status.setMessage(
				`Already editing ${outcome.limit} notes externally. Finish one before opening another.`,
			)
			return
		default:
			status.setMessage(outcome.message)
	}
}

async function stopHandoff(noteId: string) {
	await handoff.stopHandoff(noteId)
}

// --- undo --------------------------------------------------------------------

async function undo() {
	if ((await space.undo()) === 'empty') status.setMessage('Nothing to undo.')
}

async function redo() {
	if ((await space.redo()) === 'empty') status.setMessage('Nothing to redo.')
}

// --- search ------------------------------------------------------------------

/** Announced through the status line, which is the panel's only polite live
 *  region for in-panel actions. */
function announceResults() {
	if (!search.hasQuery.value) {
		status.clear()
		return
	}
	status.setMessage(noteCountLabel('Found', search.resultCount.value))
}

export function useNoteActions() {
	return {
		targetIds,
		targetNotes,
		targetCount,
		everyTargetDone,
		canMerge,
		canMoveTo,
		canExpandTarget,
		isRedundantTarget,
		copyNotes,
		copyAsList,
		toggleDone,
		moveTo,
		merge,
		deleteNotes,
		finishDrag,
		moveFocusedBy,
		expand,
		edit,
		openInEditor,
		stopHandoff,
		undo,
		redo,
		announceResults,
	}
}
