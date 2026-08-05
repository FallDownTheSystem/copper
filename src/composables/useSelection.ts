/**
 * Selection, the roving focus target, and the reconciliation that keeps both
 * pointing at notes that still exist.
 *
 * Deliberately one-directional: nothing here imports `useSpace` at runtime, and
 * the document arrives through `syncDocument`. That keeps the whole module pure
 * over its input and unit-testable without mocking any IPC, and it is what lets
 * `useSpace` call into it during reconciliation without a module cycle.
 *
 * Two orders exist and must not be conflated. `rowIds` is every focusable row in
 * visual order, section headers included — arrow keys traverse this.
 * `visibleNoteIds` is note rows only in flattened order — selection, ranges and
 * Ctrl+A operate on this. Conflating them is what breaks Shift+Arrow across a
 * section boundary.
 */

import { useNoteSearch } from './useNoteSearch'
import type { SpaceView } from './useSpace'

/** Row keys are prefixed rather than raw ids: note ids and section ids are only
 *  guaranteed unique within their own kind, and a hand-edited `.copper` can put
 *  the same string in both. */
const NOTE_ROW = 'n:'
const SECTION_ROW = 's:'

export function noteRow(id: string) {
	return NOTE_ROW + id
}

export function sectionRow(id: string) {
	return SECTION_ROW + id
}

export function rowNoteId(key: string | null): string | null {
	return key?.startsWith(NOTE_ROW) ? key.slice(NOTE_ROW.length) : null
}

export function rowSectionId(key: string | null): string | null {
	return key?.startsWith(SECTION_ROW) ? key.slice(SECTION_ROW.length) : null
}

const selectedIds = ref<string[]>([])
const focusedId = ref<string | null>(null)
const anchorId = ref<string | null>(null)

/** The document's own grouping, before the search filter. Both traversal orders
 *  are derived from it, so they can never disagree about which notes a section
 *  holds. */
const documentGroups = ref<{ sectionId: string; noteIds: string[] }[]>([])

const { matchedIds } = useNoteSearch()

/**
 * Both orders are filtered, and filtering only one of them is the single easiest
 * thing here to get half-right. `visibleNoteIds` drives selection, ranges and
 * Ctrl+A; `rowIds` drives the arrow keys and includes section header rows. Filter
 * only the first and `ArrowDown` still stops on the header rows of sections the
 * list has removed from the DOM, and on notes that no longer match.
 *
 * A section with no surviving note is dropped entirely, header included, which is
 * what makes a result's origin visible without a dozen empty headings.
 */
const orders = computed(() => {
	const matched = matchedIds.value
	const groups: { sectionId: string; noteIds: string[] }[] = []
	const rows: string[] = []
	const notes: string[] = []

	for (const group of documentGroups.value) {
		const members = matched ? group.noteIds.filter((id) => matched.has(id)) : group.noteIds
		if (matched && members.length === 0) continue

		groups.push({ sectionId: group.sectionId, noteIds: members })
		rows.push(sectionRow(group.sectionId))
		for (const id of members) {
			rows.push(noteRow(id))
			notes.push(id)
		}
	}

	return { groups, rows, notes }
})

/** Every note in the document, filter or no filter. The set reconciliation
 *  prunes against — "does this note still exist" is a different question from
 *  "is this note on screen", and only the first one may remove a selection. */
function documentNoteIds(): Set<string> {
	const ids = new Set<string>()
	for (const group of documentGroups.value) {
		for (const id of group.noteIds) ids.add(id)
	}
	return ids
}

/** What the list renders, derived from the same walk as the traversal orders —
 *  so what is on screen and what the arrow keys reach can never disagree. */
const visibleGroups = computed(() => orders.value.groups)
const rowIds = computed(() => orders.value.rows)
const visibleNoteIds = computed(() => orders.value.notes)

const selectedSet = computed(() => new Set(selectedIds.value))
const focusedNoteId = computed(() => rowNoteId(focusedId.value))

export type SelectionSnapshot = {
	/** The flattened note order *before* the new document was assigned. Without
	 *  it the focused note's former index is unrecoverable and the nearest-
	 *  survivor rule cannot be evaluated at all. */
	noteIds: string[]
	focusedId: string | null
	anchorId: string | null
	/** The row the DOM was actually focused on, or null if focus was elsewhere. */
	activeRowId: string | null
	/** The node itself, not just its id.
	 *
	 *  Matching by id alone reports "still there" for a row Vue recreated under a
	 *  different rowgroup — the id is the same but the element that held focus is
	 *  gone and `document.activeElement` has fallen back to the body, so the list
	 *  becomes unreachable by keyboard exactly when a note moves sections. */
	activeElement: HTMLElement | null
	/** Focus was inside a text-editing surface, which reconciliation must never
	 *  steal. */
	inTextSurface: boolean
	scroll: { noteId: string; offset: number } | null
}

/**
 * A different document is reconciled against *this* rather than the outgoing
 * snapshot, so it takes the first-load path. Lives here because this module owns
 * the shape.
 */
export function emptySnapshot(): SelectionSnapshot {
	return {
		noteIds: [],
		focusedId: null,
		anchorId: null,
		activeRowId: null,
		activeElement: null,
		inTextSurface: false,
		scroll: null,
	}
}

function setSelection(ids: string[]) {
	selectedIds.value = ids
}

// --- reads -------------------------------------------------------------------

function isSelected(noteId: string) {
	return selectedSet.value.has(noteId)
}

// --- commands ----------------------------------------------------------------

/** Replaces the selection with exactly this note. */
function select(noteId: string) {
	setSelection([noteId])
	focusedId.value = noteRow(noteId)
	anchorId.value = noteId
}

/** Adds or removes without disturbing the rest — the only path to a
 *  discontiguous selection, since Space is taken by mark-as-done. */
function toggle(noteId: string) {
	setSelection(
		selectedSet.value.has(noteId)
			? selectedIds.value.filter((id) => id !== noteId)
			: [...selectedIds.value, noteId],
	)
	focusedId.value = noteRow(noteId)
	anchorId.value = noteId
}

/** Contiguous range from the anchor through flattened note order, which spans
 *  section boundaries because the grid is one composite widget. */
function extendTo(noteId: string) {
	const notes = visibleNoteIds.value
	const anchor = anchorId.value && notes.includes(anchorId.value) ? anchorId.value : noteId
	anchorId.value = anchor

	const from = notes.indexOf(anchor)
	const to = notes.indexOf(noteId)
	if (from === -1 || to === -1) return

	setSelection(notes.slice(Math.min(from, to), Math.max(from, to) + 1))
	// The anchor deliberately stays put: extending again must grow from the same
	// origin, not from wherever the last extension ended.
	focusedId.value = noteRow(noteId)
}

function selectAll() {
	setSelection([...visibleNoteIds.value])
}

function clear() {
	setSelection([])
	anchorId.value = null
}

function focusRow(key: string | null) {
	focusedId.value = key
}

/** Landing on a note selects it; landing on a header leaves the selection
 *  alone. Every arrow, Home and End path ends here. */
function landOn(key: string | undefined) {
	if (!key) return
	const note = rowNoteId(key)
	if (note) select(note)
	else focusedId.value = key
}

/** Moves over `rowIds`, headers included, clamping at both ends rather than
 *  wrapping. */
function moveFocus(delta: number) {
	const rows = rowIds.value
	if (rows.length === 0) return

	const current = focusedId.value ? rows.indexOf(focusedId.value) : -1
	const next = Math.min(rows.length - 1, Math.max(0, current === -1 ? 0 : current + delta))
	landOn(rows[next])
}

function focusFirst() {
	landOn(rowIds.value[0])
}

function focusLast() {
	landOn(rowIds.value.at(-1))
}

/** Shift+Arrow: over notes only, skipping header rows. */
function extendFocus(delta: number) {
	const notes = visibleNoteIds.value
	if (notes.length === 0) return

	const current = focusedNoteId.value
	if (current === null) {
		// Focus is on a section header. Extending has to reach the note *adjacent*
		// to it in the direction of travel — falling back to index 0 would jump the
		// selection to the top of the document from anywhere in the list.
		const target = adjacentNoteFromRow(focusedId.value, delta)
		if (target) extendTo(target)
		return
	}

	const index = notes.indexOf(current)
	const next = Math.min(notes.length - 1, Math.max(0, index === -1 ? 0 : index + delta))
	const target = notes[next]
	if (target) extendTo(target)
}

/** Walks `rowIds` from a header row until it meets a note row. */
function adjacentNoteFromRow(rowKey: string | null, delta: number): string | null {
	const rows = rowIds.value
	const start = rowKey ? rows.indexOf(rowKey) : -1
	if (start === -1) return null

	const step = delta >= 0 ? 1 : -1
	for (let i = start + step; i >= 0 && i < rows.length; i += step) {
		const note = rowNoteId(rows[i] ?? null)
		if (note) return note
	}
	return null
}

// --- document lifecycle ------------------------------------------------------

/** Rebuilds the grouping both orders derive from. Called by `useSpace` on every
 *  apply. */
function syncDocument(space: SpaceView | null) {
	if (!space) {
		documentGroups.value = []
		return
	}

	// Grouped in one pass rather than re-walking every note per section. Same
	// result, but the cost stops being notes × sections.
	const bySection = new Map<string, string[]>()
	for (const section of space.sections) bySection.set(section.id, [])
	for (const note of space.notes) bySection.get(note.section)?.push(note.id)

	documentGroups.value = space.sections.map((section) => ({
		sectionId: section.id,
		noteIds: bySection.get(section.id) ?? [],
	}))
}

function scrollRegion() {
	return document.querySelector<HTMLElement>('[data-scroll-region]')
}

/**
 * Row keys contain a `:`, which a CSS selector would need escaped. Matching on
 * the dataset instead avoids depending on `CSS.escape` — which happy-dom and
 * older WebViews do not both provide — and cannot be broken by an id from a
 * hand-edited file.
 */
export function rowElement(key: string): HTMLElement | null {
	if (typeof document === 'undefined') return null
	for (const element of document.querySelectorAll<HTMLElement>('[data-row-id]')) {
		if (element.dataset.rowId === key) return element
	}
	return null
}

/**
 * Focus a row once Vue has patched the DOM. Focusing before the patch lands on
 * an element that is about to be replaced.
 */
export function focusRowSoon(key: string) {
	void nextTick(() => rowElement(key)?.focus())
}

/**
 * Must run *before* the new document is assigned. Afterwards `visibleNoteIds`
 * holds only the new order and the focused note's former index is gone.
 */
function snapshot(): SelectionSnapshot {
	const active = typeof document === 'undefined' ? null : document.activeElement
	const activeRow =
		active instanceof HTMLElement ? active.closest<HTMLElement>('[data-row-id]') : null
	const inTextSurface =
		active instanceof HTMLElement && ['INPUT', 'TEXTAREA'].includes(active.tagName)

	return {
		noteIds: [...visibleNoteIds.value],
		focusedId: focusedId.value,
		anchorId: anchorId.value,
		activeRowId: activeRow?.dataset.rowId ?? null,
		activeElement: activeRow,
		inTextSurface,
		scroll: captureScroll(),
	}
}

/**
 * Anchors on a visible note's id plus its pixel offset rather than raw
 * `scrollTop`, because an external edit can change the height of content above
 * the viewport and leave a restored `scrollTop` pointing somewhere else.
 */
function captureScroll(): SelectionSnapshot['scroll'] {
	const region = scrollRegion()
	if (!region) return null

	// One DOM query for the whole walk. `rowElement` re-queries every row on each
	// call, so calling it per note is quadratic in a list that reaches 200 — and
	// this runs on every applied document, not only on a reload.
	const rows = new Map<string, HTMLElement>()
	for (const element of document.querySelectorAll<HTMLElement>('[data-row-id]')) {
		const key = element.dataset.rowId
		// First match wins, exactly as `rowElement` does.
		if (key !== undefined && !rows.has(key)) rows.set(key, element)
	}

	const top = region.getBoundingClientRect().top
	for (const id of visibleNoteIds.value) {
		const element = rows.get(noteRow(id))
		if (!element) continue
		const offset = element.getBoundingClientRect().top - top
		if (offset >= 0) return { noteId: id, offset }
	}
	return null
}

/**
 * Prunes what no longer exists and relocates focus, against both the snapshot
 * and the freshly synced document.
 *
 * **Pruning asks "does this note exist?", not "is it on screen?"** — so it runs
 * against the whole document rather than the search-filtered orders. Using
 * `visibleNoteIds` here meant that any document change landing while a query was
 * active silently deleted every selected note the query happened to hide, which
 * is exactly the behaviour the plan records as deliberately rejected: a query
 * narrows what an action *targets*, never the selection itself.
 *
 * Focus relocation still runs on the filtered orders, and must: the roving
 * `tabindex="0"` has to sit on a row that is actually rendered.
 */
function reconcile(snap: SelectionSnapshot) {
	const existing = documentNoteIds()

	setSelection(selectedIds.value.filter((id) => existing.has(id)))
	if (anchorId.value && !existing.has(anchorId.value)) anchorId.value = null

	const live = new Set(visibleNoteIds.value)
	const rows = rowIds.value
	if (focusedId.value && rows.includes(focusedId.value)) {
		// The row survived — possibly reordered or moved to another section. Focus
		// follows it by id; the scroll restore below brings it back into view.
		return
	}

	const formerNote = rowNoteId(snap.focusedId)
	focusedId.value = formerNote ? nearestSurvivor(snap.noteIds, formerNote, live) : null

	// Either nothing was focused before or its whole neighbourhood is gone. Give
	// the grid a roving target anyway: with every row at tabindex="-1" the list
	// cannot be reached by Tab at all.
	if (!focusedId.value) {
		const firstNote = visibleNoteIds.value[0]
		focusedId.value = firstNote ? noteRow(firstNote) : (rows[0] ?? null)
	}
}

/** Nearest survivor by the focused note's *former* flattened index: forward
 *  first, then backward, then a clamp into the new list. */
function nearestSurvivor(
	formerOrder: string[],
	formerNoteId: string,
	live: Set<string>,
): string | null {
	const index = formerOrder.indexOf(formerNoteId)

	if (index !== -1) {
		for (let i = index + 1; i < formerOrder.length; i++) {
			const id = formerOrder[i]
			if (id && live.has(id)) return noteRow(id)
		}
		for (let i = index - 1; i >= 0; i--) {
			const id = formerOrder[i]
			if (id && live.has(id)) return noteRow(id)
		}
	}

	const notes = visibleNoteIds.value
	if (notes.length === 0) return null
	const clamped = notes[Math.min(Math.max(index, 0), notes.length - 1)]
	return clamped ? noteRow(clamped) : null
}

/**
 * The DOM half, run after `nextTick`.
 *
 * Focus moves only when the element that had it is gone. Stealing focus out of
 * a textarea mid-edit — or out of the composer right after a submit — is worse
 * than the problem it solves.
 */
function restoreDom(snap: SelectionSnapshot) {
	if (snap.scroll) restoreScroll(snap.scroll)
	if (snap.inTextSurface) return
	if (!snap.activeRowId) return
	// Identity, not id: a row that moved between sections is a *new* element with
	// the same id, and focus did not move with it.
	if (snap.activeElement?.isConnected) return

	const target = focusedId.value ? rowElement(focusedId.value) : null
	if (target) target.focus()
	else document.querySelector<HTMLElement>('[data-composer]')?.focus()
}

function restoreScroll(anchor: { noteId: string; offset: number }) {
	const region = scrollRegion()
	const element = rowElement(noteRow(anchor.noteId))
	if (!region || !element) return

	const delta = element.getBoundingClientRect().top - region.getBoundingClientRect().top
	region.scrollTop += delta - anchor.offset
}

/** Space identity changed: ids mean something else now, so nothing carries. */
function resetForNewSpace() {
	setSelection([])
	focusedId.value = null
	anchorId.value = null
}

/**
 * The search filter can unmount the row holding the roving `tabindex="0"`.
 *
 * Saying focus never moves would be unsatisfiable — the element is gone — and
 * every row is `tabindex="-1"` except the roving one, so a grid with no target
 * cannot be reached by Tab at all. It moves to the nearest remaining match by
 * the *former* row order, or out to the search field when nothing matches.
 *
 * A document change never reaches the relocation below: `reconcile` runs
 * synchronously inside `applyDocument`, so by the time this watcher flushes the
 * focused row is already one that exists.
 */
watch(rowIds, (rows, previous) => {
	const current = focusedId.value
	if (current && rows.includes(current)) return

	const held =
		typeof document !== 'undefined' &&
		document.activeElement instanceof HTMLElement &&
		document.activeElement.closest('[data-row-id]') !== null

	focusedId.value = current ? nearestRow(previous, current, rows) : (rows[0] ?? null)

	// Only chase DOM focus that was actually inside the list. Pulling it out of
	// the search field on every keystroke would make the field unusable.
	if (!held) return
	void nextTick(() => {
		const key = focusedId.value
		const target = key ? rowElement(key) : null
		if (target) target.focus()
		else document.querySelector<HTMLElement>('[data-search]')?.focus()
	})
})

/** Nearest survivor over the *row* order — forward first, then backward — so a
 *  filtered-out note hands focus to its neighbour rather than to the top. */
function nearestRow(formerRows: string[], formerKey: string, rows: string[]): string | null {
	const live = new Set(rows)
	const index = formerRows.indexOf(formerKey)

	if (index !== -1) {
		for (let i = index + 1; i < formerRows.length; i++) {
			const key = formerRows[i]
			if (key && live.has(key)) return key
		}
		for (let i = index - 1; i >= 0; i--) {
			const key = formerRows[i]
			if (key && live.has(key)) return key
		}
	}
	return rows[0] ?? null
}

export function useSelection() {
	return {
		selectedIds: readonly(selectedIds),
		focusedId: readonly(focusedId),
		focusedNoteId,
		anchorId: readonly(anchorId),
		rowIds,
		visibleNoteIds,
		visibleGroups,
		isSelected,
		select,
		toggle,
		extendTo,
		extendFocus,
		selectAll,
		clear,
		focusRow,
		moveFocus,
		focusFirst,
		focusLast,
		syncDocument,
		snapshot,
		reconcile,
		restoreDom,
		resetForNewSpace,
	}
}
