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

import { useAttachments } from './useAttachments'
import { useSystemClipboard } from './useSystemClipboard'
import { useEditorHandoff } from './useEditorHandoff'
import { useNoteDisclosure } from './useNoteDisclosure'
import { useNoteEditor } from './useNoteEditor'
import { useNoteSearch } from './useNoteSearch'
import { focusRowSoon, noteRow, sectionRow, takeRow, useSelection } from './useSelection'
import { countMessage, useStatusMessage } from './useStatusMessage'
import { useSpace } from './useSpace'

const space = useSpace()
const selection = useSelection()
const search = useNoteSearch()
const clipboard = useSystemClipboard()
const editor = useNoteEditor()
const handoff = useEditorHandoff()
const disclosure = useNoteDisclosure()
const status = useStatusMessage()
const attachments = useAttachments()

/**
 * **The one target rule, used by every action in this file.**
 *
 * The selection when the focused note is part of it, and the focused note alone
 * otherwise — *not* "the selection whenever it is non-empty". The two differ
 * exactly when focus sits outside the selection, and the looser reading would
 * let `Ctrl+Enter` open a note other than the card the user is looking at.
 *
 * **Focus on a section header takes the selection.** That is not the looser
 * reading: the dangerous case is focus on a *different note*, and that is still
 * resolved to the focused note alone below. A header row is not a note, so there
 * is no competing single target — and it is where the roving target lands when
 * the section holding the selection is collapsed, which would otherwise turn
 * every action into a silent no-op at the moment of folding.
 *
 * Materialised by walking `actionableNoteIds`: canonical document order rather
 * than `Set` insertion order, filtered by the active query — a query narrows
 * what an action targets — and deliberately **not** filtered by collapse, which
 * only folds rows away.
 */
function targetIds(): string[] {
	const order = selection.actionableNoteIds.value
	const focused = selection.focusedNoteId.value

	// `isSelected` rather than a scan and a fresh `Set` per call: `useSelection`
	// already memoises the selection as a set, and this runs once per `Move to ▸`
	// entry through `isRedundantTarget` alone.
	const takesSelection =
		focused === null ? selection.selectedIds.value.length > 0 : selection.isSelected(focused)
	if (takesSelection) return order.filter((id) => selection.isSelected(id))

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

/**
 * Runs mutating actions one at a time, each reading the document the previous
 * one produced.
 *
 * Every action resolves its own targets and positions from current state, so two
 * that overlap in flight both reason about the document as it was *before*
 * either ran. `Space` twice in quick succession sent `done: true` twice — a
 * no-op that still pushed a second undo snapshot — instead of toggling back, and
 * `Alt+Down` twice computed the same destination index twice and moved the note
 * one position rather than two. Serialising is what makes each press see the
 * result of the one before it.
 *
 * A rejection is swallowed for the *queue* only; the caller still sees it.
 */
let queue: Promise<unknown> = Promise.resolve()

function serialize<T>(run: () => Promise<T>): Promise<T> {
	const next = queue.then(run, run)
	queue = next.catch(() => undefined)
	return next
}

// --- copy --------------------------------------------------------------------

async function copyBodies(build: (bodies: readonly string[]) => string) {
	const notes = targetNotes()
	if (notes.length === 0) return

	const written = await clipboard.writeText(build(notes.map((note) => note.body)))
	status.setMessage(
		written
			? countMessage(notes.length, {
					one: 'Copied 1 note',
					many: (count) => `Copied ${count} notes`,
				})
			: 'Couldn’t write to the clipboard.',
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
function toggleDone() {
	return serialize(async () => {
		const notes = targetNotes()
		if (notes.length === 0) return
		await space.setNotesDone(
			notes.map((note) => note.id),
			!notes.every((note) => note.done),
		)
	})
}

// --- move to -----------------------------------------------------------------

function moveTo(sectionId: string) {
	return serialize(async () => {
		const ids = targetIds()
		if (ids.length === 0) return

		const applied = space.applied(await space.moveNotes(ids, sectionId))
		// A superseded document means a fresher one is already on its way and this
		// one was discarded; moving focus on the strength of it would be reasoning
		// about a document nobody is looking at.
		if (!applied) return

		// They stay selected — `move_notes` preserves relative order and appends
		// them to the end of the target — and focus lands on the first of them.
		//
		// **Unless that row is not on screen.** A collapsed destination has no row
		// for the moved note, and AC22's auto-expand deliberately excludes a move:
		// this destination was chosen rather than arrived at, so folding it open
		// would undo the choice just made. A search the note does not match leaves
		// the same hole. `focusRow` validates nothing, so a key naming no row leaves
		// the grid with no `tabindex="0"` anywhere and unreachable by Tab — the exact
		// failure the reconciliation exists to prevent. The section header is
		// rendered whenever the section is, so it is the honest fallback; if even
		// that is filtered away there is nothing to hold and the roving-target
		// watcher owns the outcome.
		const first = ids[0]
		if (first === undefined) return

		const rows = selection.rowIds.value
		const target = [noteRow(first), sectionRow(sectionId)].find((row) => rows.includes(row))
		if (target === undefined) return

		takeRow(target)
	})
}

// --- merge -------------------------------------------------------------------

function merge() {
	return serialize(async () => {
		const ids = targetIds()
		if (ids.length < 2) {
			status.setMessage('Select two or more notes to merge.')
			return
		}

		const result = await space.mergeNotes(ids)
		if (!space.applied(result)) return

		// **Read out of the returned document, not assumed from the request.** The
		// survivor is whichever of the merged ids comes first in canonical order,
		// and task-003 decides that against the document it actually merged — which
		// after a conflict re-apply is the external one, where the order can differ
		// from the one these ids were collected in. The survivor is simply the id
		// that is still there.
		const survivor = ids.find((id) => result?.value.notes.some((note) => note.id === id))
		if (survivor === undefined) return
		selection.select(survivor)
		focusRowSoon(noteRow(survivor))
	})
}

// --- delete ------------------------------------------------------------------

function deleteNotes() {
	return serialize(async () => {
		const ids = targetIds()
		if (ids.length === 0) return

		const result = await space.deleteNotes(ids)
		// Selection and focus are not fixed up here: task-004's reconciliation has
		// already pruned the dead ids and moved focus to the nearest survivor. A
		// second mechanism would only compete with it.
		if (!result) return
		status.setMessage(
			countMessage(ids.length, {
				one: 'Deleted 1 note · Ctrl+Z to undo',
				many: (count) => `Deleted ${count} notes · Ctrl+Z to undo`,
			}),
		)
	})
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
 * Commits a completed drag.
 *
 * The destination arrives as a section and an index rather than being read back
 * out of the DOM. The drag does not reorder the list as it runs — it translates
 * the row it carries and paints a line where that row would land — so at drop
 * time the DOM still holds the *old* order and reading it would compute a no-op
 * every time. `useNoteDrag` resolves the destination from geometry instead, and
 * counts its index over the section with the dragged note excluded, which is
 * exactly what `reorder_note` takes.
 */
function finishDrag(noteId: string, sectionId: string, index: number) {
	return serialize(async () => {
		if (reorderBlockedBySearch()) return

		const note = space.noteById(noteId)
		if (!note) return
		// A drag that changed nothing must not push an undo entry.
		if (note.section === sectionId && positionOf(noteId) === index) return

		if (!space.applied(await space.reorderNote(noteId, sectionId, index))) return

		selection.select(noteId)
		focusRowSoon(noteRow(noteId))
	})
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
function moveFocusedBy(delta: number) {
	// Positions are read inside the queue, so a held Alt+Down sees where the note
	// landed on the previous press rather than recomputing the same destination.
	return serialize(async () => {
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

		if (!space.applied(await space.reorderNote(noteId, sectionId, index))) return

		// `takeRow` rather than `focusRowSoon`: both halves of the roving target, not
		// just the DOM one. The note keeps its row key across a reorder, so
		// reconciliation leaves `focusedId` pointing at it and the two happen to
		// agree — but "happen to" is not a guarantee, and a held Alt+Down depends on
		// DOM focus landing back inside the grid for the *next* press to be seen at
		// all. The selection is deliberately left alone: unlike a drag, this is a
		// keyboard action on the focused note, and collapsing a multi-note selection
		// as a side effect of nudging one note is not something the user asked for.
		takeRow(noteRow(noteId))
	})
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

	const note = space.noteById(id)
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
		case 'opened-with-retained-file':
			// The handoff this replaced had a save Copper refused, so its file was
			// kept rather than deleted. Saying where is the only way back to that
			// text — the temp directory is not somewhere anyone would look.
			status.setMessage(`Opened. The earlier unsaved version is still at ${outcome.path}`)
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

// --- attachments -------------------------------------------------------------

/**
 * The focused note's attachments, in document order.
 *
 * The *focused* note rather than the target set, for the same reason `Expand`
 * and `Edit` are: opening files has no meaningful batch form, and a menu item
 * that launched five viewers at once from a multi-select would be a surprise
 * nobody asked for.
 */
function focusedAttachments() {
	const id = focusedTarget()
	return id === null ? [] : (space.noteById(id)?.attachments ?? [])
}

const canOpenAttachment = computed(() => focusedAttachments().length > 0)

/**
 * The item names what will happen, matching how `Mark as Done` flips rather
 * than describing state.
 *
 * Read off `mime` because this is a *label*, and a label may be wrong in a way
 * an action may not: Rust re-sniffs the bytes before deciding, so a file whose
 * recorded mime lies gets revealed rather than launched however this reads.
 */
const attachmentActionLabel = computed(() => {
	const first = focusedAttachments()[0]
	return first?.mime.startsWith('image/') ? 'Open Attachment' : 'Reveal in Explorer'
})

/** Opens the first attachment. A note with several is the uncommon case, and
 *  the menu is not the place to disambiguate — the cards themselves are, and
 *  each opens on double-click or Enter. */
async function openAttachment() {
	const first = focusedAttachments()[0]
	if (!first) return
	const failure = await attachments.openAttachment(first.file)
	if (failure) status.setMessage(failure)
}

async function stopHandoff(noteId: string) {
	const retained = await handoff.stopHandoff(noteId)
	if (retained !== null) {
		status.setMessage(`Stopped. The unsaved version is still at ${retained}`)
	}
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
	const count = search.resultCount.value
	if (count === 0) {
		status.setMessage('No notes match')
		return
	}
	status.setMessage(countMessage(count, { one: '1 note matches', many: (n) => `${n} notes match` }))
}

export function useNoteActions() {
	return {
		everyTargetDone,
		canMerge,
		canMoveTo,
		canExpandTarget,
		canOpenAttachment,
		attachmentActionLabel,
		openAttachment,
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
