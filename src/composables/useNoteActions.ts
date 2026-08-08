/**
 * The single facade the context menu, the chord layer and the card all call, so
 * no caller has to know which composable backs which action.
 *
 * It resolves targets, performs the store call, sets the status message and
 * fixes up selection and focus. Editing and the handoff are **delegations** —
 * `useNoteEditor` and `useEditorHandoff` own that state, and nothing here
 * duplicates it.
 */

import {
	buildCopyMarkdown,
	buildListMarkdown,
	buildSectionMarkdown,
	type MarkdownSection,
} from '@/lib/noteMarkdown'

import { useAttachments } from './useAttachments'
import { useSystemClipboard } from './useSystemClipboard'
import { useEditorHandoff } from './useEditorHandoff'
import { useNoteDisclosure } from './useNoteDisclosure'
import { useNoteEditor } from './useNoteEditor'
import { useNoteList } from './useNoteList'
import { useNoteSearch } from './useNoteSearch'
import { noteRow, rowNoteId, sectionRow, takeRow, useSelection } from './useSelection'
import { useSettings } from './useSettings'
import { countMessage, useStatusMessage, type StatusAction } from './useStatusMessage'
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
const settings = useSettings()
const list = useNoteList()

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

/**
 * The one clipboard write every copy affordance ends in, message included.
 *
 * `count` is what the toast says rather than what the text contains, so a
 * document-wide copy reports notes and not sections.
 *
 * A copy of no notes writes nothing and says nothing, whichever scope produced
 * it. Section headings alone are not worth putting on the clipboard, and
 * replacing whatever was there with them would be a small theft.
 */
async function writeCopy(text: string, count: number) {
	if (count === 0) return
	const written = await clipboard.writeText(text)
	status.setMessage(
		written
			? countMessage(count, {
					one: 'Copied 1 note',
					many: (n) => `Copied ${n} notes`,
				})
			: 'Couldn’t write to the clipboard.',
	)
}

async function copyBodies(build: (bodies: readonly string[]) => string) {
	const notes = targetNotes()
	await writeCopy(build(notes.map((note) => note.body)), notes.length)
}

function copyNotes() {
	return copyBodies(buildCopyMarkdown)
}

function copyAsList() {
	return copyBodies(buildListMarkdown)
}

// --- copy as Markdown, in three scopes ---------------------------------------

/**
 * Every section of the document, in document order — which is the display order,
 * since the store repairs each one it loads into it — carrying whichever of its
 * notes the caller says are in scope.
 *
 * The three scopes differ only in this function's argument and in what they
 * filter out of its result, which is what makes them one renderer rather than
 * three. `id` rides along for that filtering and is ignored by the renderer.
 *
 * Built per call rather than memoised: each scope copies once, on a deliberate
 * gesture, over at most a few hundred notes.
 */
function scopedSections(
	notesOf: (sectionId: string) => readonly { id: string; done: boolean; body: string }[],
): (MarkdownSection & { id: string })[] {
	return space.sections.value.map((section) => ({
		id: section.id,
		name: section.name,
		notes: notesOf(section.id).map((note) => ({ done: note.done, body: note.body })),
	}))
}

async function copyMarkdown(sections: readonly MarkdownSection[]) {
	const count = sections.reduce((total, section) => total + section.notes.length, 0)
	await writeCopy(buildSectionMarkdown(sections), count)
}

/**
 * The `...` menu's `Copy all as Markdown`: the whole document, every section,
 * **whatever the search field holds**.
 *
 * A query narrows what an action targets everywhere else in this file, and this
 * is the deliberate exception: a "copy all" that quietly copied a filtered subset
 * would be the one export nobody could trust, and the filtered form is a
 * selection copy away.
 */
function copyDocumentAsMarkdown() {
	return copyMarkdown(scopedSections((id) => space.notesInSection(id)))
}

/** The section context menu's copy: one section and all of its notes, the whole
 *  section rather than whatever a query left showing — matching the document
 *  scope above. */
function copySectionAsMarkdown(sectionId: string) {
	return copyMarkdown(
		scopedSections((id) => space.notesInSection(id)).filter((section) => section.id === sectionId),
	)
}

/**
 * The note menu's `Copy as Markdown`: the same targets every other note action
 * resolves, grouped under the headings of the sections they came from.
 *
 * Sections contributing nothing are dropped, so copying two notes out of one
 * section produces one heading rather than the whole document's outline.
 */
function copySelectionAsMarkdown() {
	const targeted = new Set(targetIds())
	return copyMarkdown(
		scopedSections((id) => space.notesInSection(id).filter((note) => targeted.has(note.id))).filter(
			(section) => section.notes.length > 0,
		),
	)
}

// --- zero-focus paste --------------------------------------------------------

/**
 * The mutating half of task-013's zero-focus paste, after `PanelShell` has read
 * the event.
 *
 * **In the same queue as every other mutating action**, which is the whole reason
 * it lives here rather than in the component. Two pastes a keystroke apart both
 * resolve against the document as it was before either ran: under top insertion
 * that is two notes racing for position 0, and either one can lose its place to a
 * response that lands out of order. A bare in-flight flag would *drop* the second
 * paste instead, which is the one outcome a capture tool may not have.
 *
 * The text arrives as an argument because `clipboardData` is live only while the
 * event is dispatching, so it cannot be read from inside a queued callback.
 * Everything after that read is an ordinary mutation.
 *
 * Cleared before the attempt and reported after it, which is the rule every
 * ingest path follows — see `Composer`'s `beginAttach`.
 */
function capturePaste(text: string) {
	return serialize(async () => {
		space.clearActionError('composer')

		if (text.trim().length > 0) {
			await space.addNote(text)
			return
		}

		// No text: an image or a file list, or nothing at all. An empty clipboard is
		// a silent no-op — `pasteAttachment` reports `handled: false` and says
		// nothing.
		const outcome = await attachments.pasteAttachment()
		if (outcome.message) space.reportActionError('composer', outcome.message)
	})
}

// --- mark as done ------------------------------------------------------------

/**
 * The toast's button, and the whole of its contract: **one undo step, the same
 * one `Ctrl+Z` takes.**
 *
 * That is enough because a batch is already one step. Marking a multi-note
 * selection done is a single `set_notes_done` and a single store snapshot, so
 * one press restores all of it.
 *
 * **What makes the pill on screen describe the step this undoes is not this
 * file.** A second reporting action replaces the pill, but most mutations report
 * nothing and still push an undo step — `submitEntry`, a zero-focus paste, a
 * drag, an Alt+Arrow, a `Move to ▸` — so "the newest toast names the newest
 * step" is true only because `useSpace.mutate` retires the standing pill for
 * every mutation that does not replace it. Without that, marking a note done and
 * then composing one left this button removing the new note under a toast that
 * said it would put a done one back.
 *
 * Anything cleverer (a stack of pills, an undo that walks back several steps)
 * would be a second undo model beside the store's, disagreeing with `Ctrl+Z` in
 * exactly the cases that are hard to reason about.
 *
 * One object rather than one per call: it closes over nothing, and the pill is
 * keyed on the message's generation rather than on this identity.
 */
const UNDO_ACTION: StatusAction = { label: 'Undo', run: () => void undo() }

/**
 * One `set_notes_done` call whatever the count, so marking five notes is one
 * undo — and one toast carrying that undo.
 *
 * **The toast exists because the note usually vanishes.** In the default view
 * done notes are not on screen at all, so marking one is an action whose only
 * visible result is a row leaving the list; in the done view the same is true of
 * unmarking one. Both directions therefore report, and both offer the way back.
 *
 * Focus is handed on for the same reason and through the same helper a delete
 * uses: the row that held it may no longer be in the list. **Which note it is
 * handed on from is decided by DOM focus first**, because the completion circle
 * acts on a card the keyboard may never have been near — see `domFocusHolder`.
 */
async function applyDone(ids: string[], done: boolean) {
	if (ids.length === 0) return

	// Read before the round trip: afterwards the element that had focus has left
	// the DOM with its row, and reconciliation has already moved the roving target
	// off the note this is asking about.
	const held = domFocusHolder() ?? selection.focusedNoteId.value
	if (!space.applied(await space.setNotesDone(ids, done))) return

	handFocusOnVanished(held)
	status.setMessage(
		done
			? countMessage(ids.length, {
					one: 'Moved 1 note to Done',
					many: (count) => `Moved ${count} notes to Done`,
				})
			: countMessage(ids.length, {
					one: 'Moved 1 note out of Done',
					many: (count) => `Moved ${count} notes out of Done`,
				}),
		UNDO_ACTION,
	)
}

/**
 * The selection-aware form. On a mixed selection everything becomes done; only
 * when everything is already done does it flip the other way.
 *
 * The targets are resolved **inside** the queue, which is the reason `applyDone`
 * does not serialise itself: `Space` twice in quick succession has to see the
 * result of the first press before deciding what the second one means, and
 * reading `targetNotes()` at call time sent `done: true` twice.
 */
function toggleDone() {
	return serialize(async () => {
		const notes = targetNotes()
		await applyDone(
			notes.map((note) => note.id),
			!notes.every((note) => note.done),
		)
	})
}

/**
 * The completion circle, which names one card unambiguously and so ignores the
 * selection — the selection-aware form is `Space`.
 *
 * Here rather than in the card for the two things this file gives it: a place in
 * the same queue as every other mutation, so two quick clicks do not both resolve
 * against the document as it was before either ran, and the toast.
 */
function toggleNoteDone(noteId: string) {
	return serialize(async () => {
		const note = space.noteById(noteId)
		if (note) await applyDone([noteId], !note.done)
	})
}

// --- where focus lands after a note moves -------------------------------------

/**
 * The row to hold after `noteId` lands in `sectionId` — the note's own row when
 * it is on screen, and its destination's header row when it is not.
 *
 * **A destination can be collapsed, and then the moved note has no row at all.**
 * AC22's auto-expand deliberately excludes a move: the destination was chosen
 * rather than arrived at, so folding it open would undo the choice just made. A
 * search the note does not match leaves the same hole, and so does a drag or an
 * Alt+Arrow that carries a note into a folded section.
 *
 * `focusRow` validates nothing, so a key naming no row leaves the grid with no
 * `tabindex="0"` anywhere and unreachable by Tab — the exact failure
 * reconciliation exists to prevent. The section header is rendered whenever the
 * section is, so it is the honest fallback; if even that is filtered away there
 * is nothing to hold and the roving-target watcher owns the outcome.
 *
 * Read *after* the document has been applied, so `rowIds` already describes the
 * list the user is about to be looking at.
 */
function landingRow(noteId: string, sectionId: string): string | null {
	const rows = selection.rowIds.value
	return [noteRow(noteId), sectionRow(sectionId)].find((row) => rows.includes(row)) ?? null
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
		// them to the end of the target — and focus lands on the first of them, or
		// on the destination's header when that row is not on screen.
		const first = ids[0]
		if (first === undefined) return

		const target = landingRow(first, sectionId)
		if (target !== null) takeRow(target)
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
		// from the one these ids were collected in. The survivor is simply the note
		// that is still there — taken as the note rather than the id because its
		// section is needed below and is knowable only from this document.
		const survivor = result?.value.notes.find((note) => ids.includes(note.id))
		if (survivor === undefined) return
		selection.select(survivor.id)

		// Through `landingRow`, like `moveTo` and the drag commit, rather than
		// straight to the survivor's own row: merging inside a **collapsed** section
		// leaves that note with no row at all. `select` had already pointed the
		// roving target at it, and nothing was coming to correct that — `rowIds`
		// changed while the document was applied, so its watcher had already run and
		// seen a valid target by the time this code assigned an invalid one. The grid
		// was left with `tabindex="0"` on nothing and unreachable by Tab.
		//
		// The section is read off the *returned* document for the same reason the
		// survivor is: after a conflict re-apply the merge happened against the
		// external document, where it may not be the section these ids were collected
		// from.
		const target = landingRow(survivor.id, survivor.section)
		if (target !== null) takeRow(target)
	})
}

// --- delete ------------------------------------------------------------------

/**
 * Gives the row reconciliation already chose the DOM half of focus, after a
 * document change that took the row holding it off screen.
 *
 * **Two causes, one condition.** A delete removes the note; marking a note done
 * in the default view — or unmarking one in the done view — leaves it in the
 * document and filters it out of the list. Both end with the focused note having
 * no row, which is why this asks `rowIds` rather than being told what was
 * deleted: the note that is gone from the *view* is the one focus has to be
 * handed on from, whichever of the two happened.
 *
 * **No second rule about where focus goes**, which is the whole point of doing it
 * this way. `useSelection.reconcile` has already moved the roving target to the
 * nearest survivor by the *former* row order — forward first, backward only when
 * the vanished note was the last one — so "the next note, or the previous one if
 * it was last" is a property of that walk and is not restated here.
 *
 * What is missing without this is only the DOM half, and both ways of asking
 * `restoreDom` for it fail on a delete:
 *
 * - **From the context menu, focus was never on a row at all.** It was inside the
 *   portalled menu, so the snapshot records no row, and `restoreDom` — which
 *   moves focus solely when the element that *had* it is gone — correctly decides
 *   nothing was lost. The grid was left with its roving `tabindex="0"` on a row
 *   nothing was focused on, and the next arrow key went nowhere.
 * - **From the keyboard it was, and the test still answers "still connected".**
 *   auto-animate takes a removed row back out of the DOM on its exit animation's
 *   `finish`, not when Vue patches — so at the tick `restoreDom` runs, the row
 *   that held focus is both deleted and `isConnected`. It declines, the row leaves
 *   a moment later, and focus falls to the body. That path passed intermittently
 *   depending on which side of the animation the tick landed on, which is a worse
 *   failure than not working at all.
 *
 * Moving focus here rather than relaxing that `isConnected` test, because the test
 * is right about what it guards — never steal focus out of something the user is
 * still using — and this caller is the one place that knows the element is going
 * away regardless of what the DOM currently says.
 *
 * Conditioned on the focused note actually being the one that left. A `Delete all
 * done` sweep that removes notes elsewhere in the list is not a reason to pull
 * focus out of the control the user just pressed.
 */
/**
 * The note whose row holds DOM focus right now, or null when focus is anywhere
 * else — the composer, a portalled menu, the body.
 *
 * **Asked because the roving target is a different question, and the completion
 * circle is where the two come apart.** `Checkbox` is a focusable control inside
 * the row and takes DOM focus on the press that toggles the note, while the
 * roving target is still wherever the keyboard left it: null in a panel nobody
 * has arrowed through yet, or another note entirely. In the default view the note
 * then vanishes from the list, `handFocusOnVanished` is asked about a note that
 * is still on screen (or about nothing), it correctly declines — and the element
 * that had focus leaves the DOM with the row, dropping focus to `<body>` where no
 * arrow key does anything.
 *
 * The row is read from the *element that has focus*, so a click on the circle
 * names the card it sits in without the card having to say so, and the keyboard
 * path answers the same as before: `Space` presses arrive on the row itself.
 * Focus inside a menu resolves to no row at all, which is right — the menu is
 * about to close and the roving target is the only thing left to follow.
 */
function domFocusHolder(): string | null {
	if (typeof document === 'undefined') return null
	const row = document.activeElement?.closest?.('[data-row-id]')
	return rowNoteId(row instanceof HTMLElement ? (row.dataset.rowId ?? null) : null)
}

function handFocusOnVanished(held: string | null) {
	if (held === null || selection.rowIds.value.includes(noteRow(held))) return
	const target = selection.focusedId.value
	if (target !== null) takeRow(target)
}

function deleteNotes() {
	return serialize(async () => {
		const ids = targetIds()
		if (ids.length === 0) return

		// Read before the round trip: afterwards reconciliation has already moved the
		// roving target off the note this is asking about.
		const held = selection.focusedNoteId.value
		const result = await space.deleteNotes(ids)
		// The *selection* is not fixed up here: task-004's reconciliation has already
		// pruned the dead ids and moved the roving target to the nearest survivor. A
		// second mechanism would only compete with it — which is why the line below
		// follows that decision rather than making one of its own.
		if (!result) return
		handFocusOnVanished(held)
		status.setMessage(
			countMessage(ids.length, {
				one: 'Deleted 1 note',
				many: (count) => `Deleted ${count} notes`,
			}),
			UNDO_ACTION,
		)
	})
}

// --- delete every done note in a section --------------------------------------

/**
 * The done notes of the **active** section, read straight off the document.
 *
 * Not off `actionableNoteIds`, and that is the whole design of this action. That
 * order is narrowed by whatever is in the search field, which is correct for
 * `Ctrl+A` and wrong for a button labelled "Delete all done" — the same objection
 * `copyDocumentAsMarkdown` records: an "all" that quietly operated on a filtered
 * subset would be the one destructive action nobody could trust. It is not
 * narrowed by collapse either, for the reason that rule exists everywhere else:
 * folding a section away is not deselecting its notes.
 *
 * Scoped to the active section per AC9. `activeSection` is the one section the
 * panel singles out — it is where a capture lands and which header carries the
 * marker — so it is the only non-arbitrary scope available for a control that
 * sits in the header rather than in a per-section menu.
 */
function doneInActiveSection(): string[] {
	const sectionId = space.activeSection.value
	if (sectionId === null) return []
	return space
		.notesInSection(sectionId)
		.filter((note) => note.done)
		.map((note) => note.id)
}

/**
 * Exactly what a confirmed press would delete, as a reactive value.
 *
 * The *ids* rather than only their count, because the count is not an identity: a
 * note marked done while another is unmarked leaves the total unchanged over a
 * different set, and a confirmation armed against the old set would then delete
 * notes the user never saw it offer. The control re-arms on this.
 */
const doneTargets = computed(() => doneInActiveSection())

/** What the confirmation names, so the control can show a count and withdraw when
 *  there is nothing to do. */
const doneCount = computed(() => doneTargets.value.length)

/**
 * One `delete_notes` call whatever the count, which is what makes AC7 true: the
 * store pushes exactly one snapshot per `mutate`, so the whole purge is a single
 * `Ctrl+Z`. Looping the singular delete would push one snapshot per note and make
 * undoing a five-note purge take five presses — the discipline `useSpace`'s batch
 * mutations already state.
 *
 * Selection is left to task-004's reconciliation, exactly as `deleteNotes`
 * leaves it, and focus follows that reconciliation the same way — but only when
 * the sweep took the focused note with it. This one is pressed from a button, so
 * most of the time it does not, and pulling focus off that button on the strength
 * of a note going away somewhere else in the list would be a surprise.
 */
function deleteDoneInActiveSection() {
	return serialize(async () => {
		const ids = doneInActiveSection()
		if (ids.length === 0) return

		const held = selection.focusedNoteId.value
		const result = await space.deleteNotes(ids)
		if (!result) return
		handFocusOnVanished(held)
		status.setMessage(
			countMessage(ids.length, {
				one: 'Deleted 1 done note',
				many: (count) => `Deleted ${count} done notes`,
			}),
			UNDO_ACTION,
		)
	})
}

// --- reorder -----------------------------------------------------------------

/**
 * Reordering is refused for three reasons, and all of them are arithmetic rather
 * than taste.
 *
 * **A search** leaves the rendered list a *subset* of its section, so an index
 * read off it means something different from the `index` `reorder_note` takes,
 * which counts positions in the whole section. Dropping a note between two
 * matches would silently move it somewhere else entirely.
 *
 * **The done filter, in either of the two states that narrow** — which is now the
 * default one as well as the done view. The subset reasoning above transfers
 * verbatim: a done-only list omits every unfinished note between two done ones,
 * so dropping a note "between" them lands it wherever those omitted notes happen
 * to leave the count, and the default view has the same hole with the halves
 * swapped. That this was missed when the filter was added is the point of stating
 * the reason as arithmetic — *any* narrowing of the rendered rows breaks the
 * index, so a new one has to join this guard rather than be judged on its own,
 * which is why the condition asks `filtersByDone` and not which half is showing.
 *
 * **A non-manual sort** is the stronger form again: the rendered order is a
 * *permutation* of the section, so a drop index means nothing at all — and the
 * position it would write is one the sort immediately overrules on the next
 * render, which reads as a drag that silently sprang back.
 *
 * All three take no argument, because none of them is per-section: each narrows
 * or permutes every group at once. The sort used to be the exception and had to be
 * asked about both ends of a move — an Alt+Arrow out of a manual section into a
 * sorted one is still a reorder whose destination index is meaningless — which is
 * why callers passed the sections they were about to touch. One document-wide mode
 * makes both ends the same answer, so a single check at the top of a move now
 * covers a destination the caller has not even resolved yet.
 *
 * The drag grip is hidden under all three conditions, so this guards the keyboard
 * path and anything that slips past that.
 */
function reorderBlocked(): boolean {
	if (search.hasQuery.value) {
		status.setMessage('Clear the search to reorder notes.')
		return true
	}
	if (list.filtersByDone.value) {
		status.setMessage('Show all notes to reorder them.')
		return true
	}
	if (list.isSorted.value) {
		// Names the control that gives reordering back, which is what AC14's label
		// has to do — the grip is `role="presentation"` and has nowhere to say it.
		status.setMessage('Set the sort to Manual to reorder notes.')
		return true
	}
	return false
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
		const note = space.noteById(noteId)
		if (!note) return
		if (reorderBlocked()) return

		// A drag that changed nothing must not push an undo entry.
		if (note.section === sectionId && positionOf(noteId) === index) return

		if (!space.applied(await space.reorderNote(noteId, sectionId, index))) return

		// Selected unconditionally — collapse folds a row away, it never unselects
		// the note — but focus goes to whatever row actually exists, which is the
		// destination's header when the note was dropped into a folded section.
		selection.select(noteId)
		const target = landingRow(noteId, sectionId)
		if (target !== null) takeRow(target)
	})
}

/**
 * The note's index within its own section in the *document*, which is what a
 * no-op drag has to be compared against.
 *
 * Read off the document rather than off `visibleGroups`, which is what the
 * docstring always claimed and the body did not do. `useNoteDrag` counts the
 * destination index over the whole section, so comparing it against a position
 * taken from the *rendered* rows compares two different coordinate systems. It
 * happened to agree whenever nothing narrowed or reordered the list, and every
 * condition that breaks that agreement is refused by `reorderBlocked` — so the
 * bug was unreachable rather than absent. Making the body match the contract is
 * what keeps it unreachable when the next filter arrives, instead of resting on a
 * guard somebody has to remember to extend.
 */
function positionOf(noteId: string): number {
	const note = space.noteById(noteId)
	if (!note) return -1
	return space.notesInSection(note.section).findIndex((entry) => entry.id === noteId)
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
		// Once, up front, and it covers the neighbouring section this step may cross
		// into as well: every reason to refuse is document-wide. The index arithmetic
		// below depends on it having refused.
		if (noteId === null || reorderBlocked()) return

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
			//
			// Counted off the *document* rather than off `neighbour.noteIds`, which is
			// what the visible walk publishes: a collapsed section publishes an empty
			// list, so an Alt+Up into one landed the note at index 0 — the top — which
			// is the opposite of what travelling upward means. Reordering is refused
			// outright while a query is active, so the document count is the whole
			// section here and never a filtered subset of it.
			index = delta > 0 ? 0 : space.notesInSection(neighbour.sectionId).length
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
		//
		// Through `landingRow` because this can cross into a collapsed section, where
		// the moved note has no row to hold.
		const target = landingRow(noteId, sectionId)
		if (target !== null) takeRow(target)
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

/**
 * What a double-click on a note body means, which is a setting.
 *
 * The gesture arrives already resolved: two clicks have run the row's own
 * pointer-select, so the note is selected and focused and both branches target
 * it with no argument. `NoteCard` decides *whether* a double-click counts —
 * excluding the grip, the controls and a drag — and this decides what it does.
 *
 * The native word-select the gesture performed on the way here is collapsed
 * rather than treated as a reason to decline. `.note-prose` is `select-text`, so
 * a body double-click always leaves a non-empty selection by the time `dblclick`
 * fires — reading `getSelection()` here would suppress the feature exactly where
 * it is meant to work — and a highlighted word left standing after "act on this
 * note" is a leftover from a gesture that meant something else.
 */
function doubleClickNote() {
	window.getSelection()?.removeAllRanges()
	if (settings.doubleClickAction.value === 'edit') edit()
	else void copyNotes()
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

	// **An open inline edit owns Ctrl+Enter for the note it is editing.** The
	// editor's textarea contains the press itself, but the conflict card's buttons
	// are inside the editor and are not a text surface, so the shell's guard lets
	// one through from there — and a handoff started off an uncommitted draft
	// forks a second writer over the same body, which is exactly what the conflict
	// state the user is standing in exists to stop.
	if (id === editor.editingNoteId.value) {
		status.setMessage('Finish the inline edit first, or press Escape to discard it.')
		return
	}

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
		copyDocumentAsMarkdown,
		copySectionAsMarkdown,
		copySelectionAsMarkdown,
		capturePaste,
		toggleDone,
		toggleNoteDone,
		moveTo,
		merge,
		deleteNotes,
		doneCount,
		doneTargets,
		deleteDoneInActiveSection,
		finishDrag,
		moveFocusedBy,
		expand,
		edit,
		doubleClickNote,
		openInEditor,
		stopHandoff,
		undo,
		redo,
		announceResults,
	}
}
