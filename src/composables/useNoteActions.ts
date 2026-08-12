/**
 * The single facade the context menu, the chord layer and the card all call, so
 * no caller has to know which composable backs which action.
 *
 * It resolves targets, performs the store call, sets the status message and
 * fixes up selection and focus. Editing and the handoff are **delegations** —
 * `useNoteEditor` and `useEditorHandoff` own that state, and nothing here
 * duplicates it.
 */

import { useAttachments } from './useAttachments'
import { useSystemClipboard } from './useSystemClipboard'
import { useEditorHandoff } from './useEditorHandoff'
import { useNoteDisclosure } from './useNoteDisclosure'
import { useNoteEditor } from './useNoteEditor'
import { useNoteList } from './useNoteList'
import { useNoteSearch } from './useNoteSearch'
import { noteRow, rowNoteId, rowSectionId, sectionRow, takeRow, useSelection } from './useSelection'
import { useSettings } from './useSettings'
import { useDeviceShare } from './useDeviceShare'
import { countMessage, useStatusMessage, type StatusAction } from './useStatusMessage'
import { useSpace, type MarkdownFormat, type NoteSelection, type Section } from './useSpace'

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
const share = useDeviceShare()

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
	if (!written) {
		status.setError("Couldn't write to the clipboard.")
		return
	}
	status.setMessage(
		countMessage(count, {
			one: 'Copied 1 note',
			many: (n) => `Copied ${n} notes`,
		}),
	)
}

/**
 * Every copy affordance in this file, in one shape: say which notes, say which
 * rendering, put the answer on the clipboard.
 *
 * **This side no longer marshals a single note body.** Task-024 moved both the
 * rendering and the resolution of a selection into `copper_core::markdown` and
 * `render_notes_markdown`, so the frontend's remaining job is deciding *which*
 * notes a gesture means — `targetIds()`, a section id, or the whole document —
 * and the clipboard write itself. That is what makes the app and `copper copy`
 * one format rather than two implementations of one.
 *
 * The count comes back with the text and is not recomputed here. Both then
 * describe the same document, which a locally counted `targetIds().length`
 * could not promise if the document moved under the selection.
 *
 * The argument is `scope` rather than `selection`, which is what the command
 * calls it: `selection` is already `useSelection()` at module scope in this
 * file, and a parameter of that name would shadow it inside every copy.
 */
async function copy(scope: NoteSelection, format: MarkdownFormat) {
	const rendered = await space.renderNotesMarkdown(scope, format)
	if (!rendered) {
		status.setError("Couldn't copy those notes.")
		return
	}
	await writeCopy(rendered.text, rendered.count)
}

function copyNotes() {
	return copy({ kind: 'ids', ids: targetIds() }, 'bodies')
}

function copyAsList() {
	return copy({ kind: 'ids', ids: targetIds() }, 'list')
}

// --- copy as Markdown, in three scopes ---------------------------------------

/**
 * The `...` menu's `Copy all as Markdown`: the whole document, every section,
 * **whatever the search field holds**.
 *
 * A query narrows what an action targets everywhere else in this file, and this
 * is the deliberate exception: a "copy all" that quietly copied a filtered subset
 * would be the one export nobody could trust, and the filtered form is a
 * selection copy away.
 *
 * An empty section keeps its heading in this scope and in the section scope
 * below, and loses it in the selection scope — a rule the renderer's caller has
 * always owned and that now lives with the resolution, in `markdown.rs`.
 */
function copyDocumentAsMarkdown() {
	return copy({ kind: 'document' }, 'markdown')
}

/** The section context menu's copy: one section and all of its notes, the whole
 *  section rather than whatever a query left showing — matching the document
 *  scope above. */
function copySectionAsMarkdown(sectionId: string) {
	return copy({ kind: 'section', id: sectionId }, 'markdown')
}

/**
 * The note menu's `Copy as Markdown`: the same targets every other note action
 * resolves, grouped under the headings of the sections they came from.
 *
 * Sections contributing nothing are dropped, so copying two notes out of one
 * section produces one heading rather than the whole document's outline.
 */
function copySelectionAsMarkdown() {
	return copy({ kind: 'ids', ids: targetIds() }, 'markdown')
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

/**
 * The split half of a list paste — the popover's other offer, after
 * `PanelShell` has asked and the user has chosen separate notes.
 *
 * In the same queue as `capturePaste` and for its reason: the items resolve
 * against the document as it is when this turn comes, not as it was when the
 * popover opened. The batch itself is `useSpace.addNotes`'s — one command, one
 * undo step — so nothing here loops.
 *
 * No toast: like every other add, the result is on screen. The count the user
 * confirmed is the count that appears.
 */
function captureListPaste(items: string[]) {
	return serialize(async () => {
		space.clearActionError('composer')
		await space.addNotes(items)
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
 *
 * Exported because destructive actions live outside this file too — deleting a
 * section is one store step like any other, and its toast has to offer the same
 * press rather than spelling the chord out in prose.
 */
export const UNDO_ACTION: StatusAction = { label: 'Undo', run: () => void undo() }

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
 * `focusRow` validates nothing, so a key naming no row leaves the roving
 * target on nothing and the arrows with nowhere to resume — the exact failure
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
		// seen a valid target by the time this code assigned an invalid one. The
		// roving target named a row that did not exist, and the arrows had nowhere
		// to resume from.
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
 *   nothing was lost. The grid was left with its roving target on a row
 *   nothing was focused on, and the next arrow key went nowhere.
 * - **From the keyboard it was, and the test still answers "still connected".**
 *   The list's `<TransitionGroup>` holds a removed row in the DOM until its leave
 *   animation reports done, not until Vue patches — so at the tick `restoreDom`
 *   runs, the row that held focus is both deleted and `isConnected`. It declines,
 *   the row leaves a moment later, and focus falls to the body. That path passed
 *   intermittently depending on which side of the animation the tick landed on,
 *   which is a worse failure than not working at all.
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

// --- delete a section ----------------------------------------------------------

/**
 * `Delete section`, shared by the section context menu and the keyboard
 * confirm in `SectionHeader`.
 *
 * No confirmation dialog *here*: the whole operation is one undo, and an
 * undoable action reads better as a reversible one than as a question. The
 * keyboard path asks before calling — a bare Delete is one keypress, where a
 * menu item is already a second gesture — and both arrive at this same single
 * step. The chord is not spelled out in the sentence: the pill carries a
 * button that takes that same one step.
 *
 * The count is read before the round trip, because afterwards the section's
 * notes are gone from the document that would be asked.
 */
function removeSection(section: Section) {
	return serialize(async () => {
		const count = space.notesInSection(section.id).length
		const result = await space.deleteSection(section.id)
		if (!result) return

		// The header row left with the section, and focus may have been on it —
		// or inside the confirm popover, which unmounts with it. Reconciliation
		// has already pointed the roving target at the nearest surviving row;
		// this is the DOM half of following it.
		const target = selection.focusedId.value
		if (target !== null) takeRow(target)

		status.setMessage(
			count === 0
				? `Deleted “${section.name}”`
				: countMessage(count, {
						one: `Deleted “${section.name}” and 1 note`,
						many: (n) => `Deleted “${section.name}” and ${n} notes`,
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
 * marker — so it is the one section-sized scope the header's control can offer
 * without being arbitrary. The document-wide sweep below is the other offer.
 */
function doneInActiveSection(): string[] {
	const sectionId = space.activeSection.value
	return sectionId === null ? [] : doneInSection(sectionId)
}

/** One section's done notes, off the document — the body the active-section
 *  offer above narrows to, and what the section context menu asks for its own
 *  section, which is not always the active one. */
function doneInSection(sectionId: string): string[] {
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
	return purgeDone(doneInActiveSection)
}

/**
 * Every done note in the document, in document order — the popover's other
 * offer. Off the document rather than off `actionableNoteIds` for exactly the
 * reason `doneInActiveSection` records: an "all" narrowed by a live search
 * would be the one destructive action nobody could trust.
 */
function doneEverywhere(): string[] {
	return (space.space.value?.notes ?? []).filter((note) => note.done).map((note) => note.id)
}

/** The document-wide twin of [`doneTargets`], and an identity for the same
 *  reason: the popover's two offers both die when the done set moves. */
const allDoneTargets = computed(() => doneEverywhere())

const allDoneCount = computed(() => allDoneTargets.value.length)

/** The whole document's done notes, still one `delete_notes` call and so still
 *  one `Ctrl+Z` — the property AC7 demands of the section-scoped purge holds
 *  for the wide one by the same construction. */
function deleteAllDone() {
	return purgeDone(doneEverywhere)
}

/** The section context menu's purge: the menu's own section, named by id
 *  because the right-clicked section and the active one can differ. The same
 *  shared body as the other two scopes, so it is one command, one undo step
 *  and the same toast. */
function deleteDoneInSection(sectionId: string) {
	return purgeDone(() => doneInSection(sectionId))
}

/** The shared body of the two purges; only the target set differs. The ids are
 *  read *inside* the serialized step, so a queued purge acts on the document as
 *  it is when its turn comes, not as it was when the button was pressed. */
function purgeDone(targets: () => string[]) {
	return serialize(async () => {
		const ids = targets()
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
 * Reordering is refused for two reasons, and both are arithmetic rather than
 * taste: under either, the rendered order is a *permutation* of the document's.
 *
 * **A search** ranks its matches within each section, so the rendered order is
 * not the document's — a drop between two results names a position the ranking
 * invented, and the position it would write is one the ranking immediately
 * re-presents in its own order.
 *
 * **A non-manual sort** is the plainest form: the rendered order is computed,
 * so a drop index means nothing at all — and the position it would write is one
 * the sort immediately overrules on the next render, which reads as a drag that
 * silently sprang back.
 *
 * **The done filter used to be a third reason and deliberately is not any more**
 * (user ruling 2026-08-12). It narrows the rows but never reorders them, and the
 * drop math stopped reading a bare count off the rendered list: a move now
 * anchors to its nearest *visible* neighbour and lands directly beside it in
 * document order — `finishDrag` and `moveFocusedBy` carry the arithmetic — so
 * the hidden notes between two visible ones cannot shift the landing; they
 * simply stay where they are. A resting view the user works in stays
 * reorderable; the two transient presentations above do not.
 *
 * Both conditions take no argument, because neither is per-section: each
 * permutes every group at once, so a single check at the top of a move covers a
 * destination the caller has not even resolved yet.
 *
 * The drag grip is hidden under both conditions, so this guards the keyboard
 * path and anything that slips past that.
 */
function reorderBlocked(): boolean {
	if (search.hasQuery.value) {
		status.setMessage('Clear the search to reorder notes.')
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

/** The same two conditions as `reorderBlocked` above, without its message: the
 *  note menu disables its Move up / Move down rows instead of letting them fire
 *  and toast. The refusal messages stay with the paths that have no UI to grey
 *  out — the chord and the drag. */
const canReorder = computed(() => !search.hasQuery.value && !list.isSorted.value)

/**
 * The notes a move gesture carries: the whole selection when the note the
 * gesture names is part of it, that note alone otherwise — the target rule,
 * anchored on the carried note rather than on focus, because a drag can grab a
 * card the keyboard has never visited. It is what makes the drag and Alt+Arrow
 * agree about what "move" means over a multi-selection.
 *
 * Document order, off `actionableNoteIds` like every batch action. That order
 * is narrowed by a query — refused behind `reorderBlocked` — and by the done
 * filter, where the narrowing is exactly right: the block a filtered move
 * carries is the selection the user can see, which is all the selection there
 * is under it. Collapse deliberately does not narrow it: folding a section
 * shut never narrowed what an action targets, so a selected note in a folded
 * section travels with the block.
 */
function movedIds(anchorId: string): string[] {
	if (!selection.isSelected(anchorId)) return [anchorId]
	const ids = selection.actionableNoteIds.value.filter((id) => selection.isSelected(id))
	return ids.includes(anchorId) ? ids : [anchorId]
}

/**
 * Whether `reorder_notes` would leave the document exactly as it is — the
 * no-op a drag back to where it started must be recognised as, so it pushes no
 * undo entry.
 *
 * Answered by building the target section's resulting order and comparing,
 * rather than by comparing any one note's position: a block is unmoved only
 * when it is already *contiguous* at the destination, and contiguity is a
 * property of the whole group. Read off the document, not off `visibleGroups`
 * — `useNoteDrag` counts over the whole section, a collapsed section publishes
 * an empty visible list, and every other way the two could disagree is refused
 * by `reorderBlocked` before this runs.
 */
function blockUnmoved(ids: string[], sectionId: string, index: number): boolean {
	if (!ids.every((id) => space.noteById(id)?.section === sectionId)) return false
	const carried = new Set(ids)
	const group = space.notesInSection(sectionId).map((entry) => entry.id)
	const rest = group.filter((id) => !carried.has(id))
	const at = Math.min(Math.max(index, 0), rest.length)
	const after = [...rest.slice(0, at), ...ids, ...rest.slice(at)]
	return after.every((id, position) => id === group[position])
}

/** The target section's rendered rows, in render order — the list the drag's
 *  geometry measured and the one an Alt+Arrow hop is a statement about. A
 *  collapsed section publishes an empty list, exactly as it renders no rows. */
function visibleIn(sectionId: string): readonly string[] {
	return selection.visibleGroups.value.find((group) => group.sectionId === sectionId)?.noteIds ?? []
}

/**
 * Where a slot between rendered rows sits in the document — the index
 * `reorder_notes` takes, which counts the target section with the carried
 * block removed.
 *
 * Resolved through the rows *around* the slot rather than by counting rows
 * above it: under the done filter (user ruling 2026-08-12) the rendered list
 * is a subset of the section, so a bare count lands wherever the hidden notes
 * leave it. Anchoring instead — directly after the nearest visible non-carried
 * row above the slot, directly before the nearest below when nothing visible
 * is above — puts the block exactly beside the row the user dropped it
 * against, and the hidden notes between two visible ones stay exactly where
 * they are. With nothing narrowing the view this resolves to the same index
 * the old subtraction computed, so one rule serves every view.
 *
 * `null` when the section shows no rows at all (empty, filtered empty, or
 * collapsed): a rowless section offers no anchor, so the caller keeps its
 * incoming index as document coordinates. From real drag geometry that index
 * is always 0 — the top, which is where the indicator paints — and a direct
 * caller's nonzero index keeps meaning what `reorder_notes` says it means,
 * which is what lets a same-place drop into a collapsed section stay the no-op
 * it is.
 */
function documentIndex(
	sectionId: string,
	visible: readonly string[],
	slot: number,
	carried: ReadonlySet<string>,
): number | null {
	let above: string | null = null
	for (let i = slot - 1; i >= 0; i--) {
		const id = visible[i]!
		if (!carried.has(id)) {
			above = id
			break
		}
	}
	let below: string | null = null
	for (let i = slot; i < visible.length; i++) {
		const id = visible[i]!
		if (!carried.has(id)) {
			below = id
			break
		}
	}

	const remaining = space.notesInSection(sectionId).filter((entry) => !carried.has(entry.id))
	if (above !== null) return remaining.findIndex((entry) => entry.id === above) + 1
	if (below !== null) return remaining.findIndex((entry) => entry.id === below)
	return null
}

/**
 * Commits a completed drag — of the focused block, not only of the grabbed row.
 *
 * The destination arrives as a section and an index rather than being read back
 * out of the DOM. The drag does not reorder the list as it runs — it translates
 * the row it carries and paints a line where that row would land — so at drop
 * time the DOM still holds the *old* order and reading it would compute a no-op
 * every time. `useNoteDrag` resolves the destination from geometry instead.
 *
 * That geometry counts its slot over the target's rendered rows with only the
 * *dragged* note excluded; `documentIndex` resolves the slot to the index
 * `reorder_notes` takes, anchored to the slot's visible neighbours.
 */
function finishDrag(noteId: string, sectionId: string, index: number) {
	return serialize(async () => {
		const note = space.noteById(noteId)
		if (!note) return
		if (reorderBlocked()) return

		const ids = movedIds(noteId)
		const carried = new Set(ids)
		const visible = visibleIn(sectionId).filter((id) => id !== noteId)
		const slot = Math.min(index, visible.length)

		// A drop that changes nothing the user can see commits nothing. The
		// document-level check below cannot answer this under the done filter:
		// putting a note back into its own visible slot could still carry it
		// across the hidden notes beside it — a change the gesture never
		// expressed, invisible on screen, and a surprise waiting in the file.
		if (ids.length === 1 && note.section === sectionId) {
			const current = visibleIn(sectionId)
			const after = [...visible.slice(0, slot), noteId, ...visible.slice(slot)]
			if (after.every((id, position) => id === current[position])) return
		}

		const at = documentIndex(sectionId, visible, slot, carried) ?? index

		// A drag that changed nothing must not push an undo entry.
		if (blockUnmoved(ids, sectionId, at)) return

		if (!space.applied(await space.reorderNotes(ids, sectionId, at))) return

		// A lone note is selected on landing, as a drop has always done — collapse
		// folds a row away, it never unselects the note. A carried block is *not*
		// re-selected to the grabbed note: the block is still what the user picked,
		// and it is all still selected. Focus goes to whatever row actually
		// exists, which is the destination's header inside a folded section.
		if (ids.length === 1) selection.select(noteId)
		const target = landingRow(noteId, sectionId)
		if (target !== null) takeRow(target)
	})
}

/**
 * The keyboard equivalent of a drag, since every action has to be reachable
 * without a pointer — and it carries what a drag carries: the whole selection
 * when the focused note is part of it (`movedIds`), the focused note alone
 * otherwise. A single note is a block of one, so both run the same arithmetic.
 * At a section boundary the block crosses into the neighbouring section rather
 * than stopping, which is what the drag does too.
 *
 * On a section header the same chord carries the whole section instead — the
 * same move as the section menu's Move up / Move down, with the same
 * index-after-removal semantics. That branch deliberately skips
 * `reorderBlocked`: those refusals exist because a searched or sorted *note*
 * view is a permutation of the document, and no sort or query ever reorders
 * the headers — section order is document order in every view.
 */
function moveFocusedBy(delta: number) {
	// Positions are read inside the queue, so a held Alt+Down sees where the note
	// landed on the previous press rather than recomputing the same destination.
	return serialize(async () => {
		const headerId = rowSectionId(selection.focusedId.value)
		if (headerId !== null) {
			const sections = space.sections.value
			const at = sections.findIndex((entry) => entry.id === headerId)
			const to = at + delta
			if (at < 0 || to < 0 || to >= sections.length) return
			if (!space.applied(await space.reorderSection(headerId, to))) return
			// The header keeps its row key across the move, but DOM focus has to
			// land back on the moved row for a held Alt+Down's next press to be
			// seen — the same reason the note branch ends in `takeRow`.
			takeRow(sectionRow(headerId))
			return
		}

		const noteId = selection.focusedNoteId.value
		// Once, up front, and it covers the neighbouring section this step may cross
		// into as well: every reason to refuse is document-wide. The index arithmetic
		// below depends on it having refused.
		if (noteId === null || reorderBlocked()) return
		const note = space.noteById(noteId)
		if (!note) return

		const ids = movedIds(noteId)
		const carried = new Set(ids)

		// One step is one hop over the nearest *visible* non-carried note, and the
		// block lands directly beside it in document order. Under the done filter
		// (user ruling 2026-08-12) that is what makes the hop mean what the screen
		// shows: the hidden notes between the block and its visible neighbour stay
		// where they are, instead of the block shuffling invisibly past one of
		// them per press. With nothing narrowing the view the nearest visible note
		// IS the nearest remaining note, so this is the same hop it always was.
		// The focused note always has a row, so it is always in this list.
		const visible = visibleIn(note.section)
		const at = visible.indexOf(noteId)
		const step = delta > 0 ? 1 : -1
		let neighbour: string | null = null
		for (let i = at + step; at !== -1 && i >= 0 && i < visible.length; i += step) {
			const id = visible[i]!
			if (!carried.has(id)) {
				neighbour = id
				break
			}
		}

		let sectionId = note.section
		let index: number

		if (neighbour === null) {
			// No visible note left to hop in the direction of travel: the block
			// crosses into the neighbouring section, entering at the near end —
			// from above it lands at the top, from below at the bottom.
			const sections = space.sections.value
			const here = sections.findIndex((entry) => entry.id === note.section)
			const next = sections[here + delta]
			if (!next) return
			sectionId = next.id
			index =
				delta > 0
					? 0
					: space.notesInSection(next.id).filter((entry) => !carried.has(entry.id)).length
		} else {
			// `remaining` — the section with the carried block removed — is the list
			// `reorder_notes` interprets its index against.
			const remaining = space.notesInSection(sectionId).filter((entry) => !carried.has(entry.id))
			const pos = remaining.findIndex((entry) => entry.id === neighbour)
			index = delta > 0 ? pos + 1 : pos
		}

		if (!space.applied(await space.reorderNotes(ids, sectionId, index))) return

		// `takeRow` rather than `focusRowSoon`: both halves of the roving target, not
		// just the DOM one. The note keeps its row key across a reorder, so
		// reconciliation leaves `focusedId` pointing at it and the two happen to
		// agree — but "happen to" is not a guarantee, and a held Alt+Down depends on
		// DOM focus landing back inside the grid for the *next* press to be seen at
		// all. The selection is deliberately left alone: a carried block is still
		// the thing the user picked, and when focus sits outside the selection,
		// collapsing that selection as a side effect of nudging one note is not
		// something they asked for.
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
			status.setError(
				"Couldn't open an editor. Set the EDITOR environment variable to your editor's path.",
			)
			return
		case 'at-capacity':
			status.setError(
				`Already editing ${outcome.limit} notes externally. Finish one before opening another.`,
			)
			return
		default:
			status.setError(outcome.message)
	}
}

// --- device share (task-026) -------------------------------------------------

/**
 * Sends the target notes to the user's other machine.
 *
 * Through `serialize()` like every other action here, so a send cannot run
 * beside a delete that is removing the notes it is reading.
 *
 * **Every outcome is reported**, mirroring `openInEditor`'s switch. The two
 * worth reading the wording of:
 *
 * - `delayed` is a *success*. The relay stored the note and failed only to
 *   announce it; the next send announces it too, so nothing is lost and nothing
 *   needs doing.
 * - `unknown` is neither. The request left and its answer never arrived, so the
 *   note may well have been delivered — which is exactly why the message says
 *   sending it again would duplicate it rather than inviting a retry.
 */
function sendToOtherDevice() {
	return serialize(async () => {
		const ids = targetIds()
		if (ids.length === 0) return

		const outcome = await share.sendNotes(ids)
		switch (outcome.kind) {
			case 'sent':
				status.setMessage(
					countMessage(outcome.notes, {
						one: 'Sent 1 note to your other device',
						many: (n) => `Sent ${n} notes to your other device`,
					}),
				)
				return
			case 'delayed':
				// **Stored, not announced**, which is not the same as "on its way". The
				// relay kept the note and failed to advance its head pointer, and the
				// reader only walks up to that pointer — so it is collected once a
				// *later* send moves the pointer past it. Neither "shortly" nor "with
				// the next one": that send can fail to announce itself too.
				status.setMessage(
					countMessage(outcome.notes, {
						one: 'Sent 1 note. The relay has it; it arrives once a later send goes through.',
						many: (n) =>
							`Sent ${n} notes. The relay has them; they arrive once a later send goes through.`,
					}),
				)
				return
			case 'unknown':
				status.setError(
					`The relay did not confirm this note (${outcome.message}). It may have arrived, so sending it again would deliver it twice.`,
				)
				return
			// **One number, and it is the one the reader can act on.** The message used
			// to report the ciphertext size against the relay's raw cap and then
			// explain that attachments inflate by about a third — three numbers and a
			// multiplication, at the end of which the reader still had to work out how
			// many files to remove. `outcome.bytes` and `outcome.limit` are measured
			// after encryption, so neither is a size the reader has ever seen on disk.
			// 14 MB is that arithmetic already done, rounded down so it is always
			// true.
			case 'too-large':
				status.setError(
					'This selection is too big to share. One note carries about 14 MB of attachments, so send fewer at a time.',
				)
				return
			case 'unconfigured':
				status.setError(`Set the ${outcome.missing} in Settings → Share first.`)
				return
			default:
				status.setError(outcome.message)
		}
	})
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
 *
 * The reveal branch says exactly what the attachment card's own menu says —
 * one action, one name, in two menus a user can open seconds apart on the same
 * file. The card's reasoning carries the wording: what is shown is where
 * Copper put its copy, and the sidecar directory is not somewhere they chose.
 */
const attachmentActionLabel = computed(() => {
	const first = focusedAttachments()[0]
	return first?.mime.startsWith('image/') ? 'Open attachment' : 'Open attachment location'
})

/** Opens the first attachment. A note with several is the uncommon case, and
 *  the menu is not the place to disambiguate — the cards themselves are, and
 *  each opens on double-click or Enter. */
async function openAttachment() {
	const first = focusedAttachments()[0]
	if (!first) return
	const failure = await attachments.openAttachment(first.file)
	if (failure) status.setError(failure)
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
		captureListPaste,
		toggleDone,
		toggleNoteDone,
		moveTo,
		merge,
		deleteNotes,
		removeSection,
		doneCount,
		doneTargets,
		allDoneCount,
		allDoneTargets,
		deleteDoneInActiveSection,
		deleteAllDone,
		deleteDoneInSection,
		finishDrag,
		canReorder,
		moveFocusedBy,
		expand,
		edit,
		doubleClickNote,
		openInEditor,
		canSendToOtherDevice: share.canSend,
		sendToOtherDevice,
		stopHandoff,
		undo,
		redo,
		announceResults,
	}
}
