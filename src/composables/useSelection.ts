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
const rowIds = ref<string[]>([])
const visibleNoteIds = ref<string[]>([])

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
	/** Focus was inside a text-editing surface, which reconciliation must never
	 *  steal. */
	inTextSurface: boolean
	scroll: { noteId: string; offset: number } | null
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

/** Moves over `rowIds`, headers included, clamping at both ends rather than
 *  wrapping. Landing on a note selects it; landing on a header leaves the
 *  selection alone. */
function moveFocus(delta: number) {
	const rows = rowIds.value
	if (rows.length === 0) return

	const current = focusedId.value ? rows.indexOf(focusedId.value) : -1
	const next = Math.min(rows.length - 1, Math.max(0, current === -1 ? 0 : current + delta))
	const key = rows[next]
	if (!key) return

	const note = rowNoteId(key)
	if (note) select(note)
	else focusedId.value = key
}

function focusFirst() {
	const key = rowIds.value[0]
	if (!key) return
	const note = rowNoteId(key)
	if (note) select(note)
	else focusedId.value = key
}

function focusLast() {
	const key = rowIds.value.at(-1)
	if (!key) return
	const note = rowNoteId(key)
	if (note) select(note)
	else focusedId.value = key
}

/** Shift+Arrow: over notes only, skipping header rows. */
function extendFocus(delta: number) {
	const notes = visibleNoteIds.value
	if (notes.length === 0) return

	const current = focusedNoteId.value
	const index = current ? notes.indexOf(current) : -1
	const next = Math.min(notes.length - 1, Math.max(0, index === -1 ? 0 : index + delta))
	const target = notes[next]
	if (target) extendTo(target)
}

// --- document lifecycle ------------------------------------------------------

/** Rebuilds both orders from a document. Called by `useSpace` on every apply. */
function syncDocument(space: SpaceView | null) {
	if (!space) {
		rowIds.value = []
		visibleNoteIds.value = []
		return
	}

	const rows: string[] = []
	const notes: string[] = []
	for (const section of space.sections) {
		rows.push(sectionRow(section.id))
		for (const note of space.notes) {
			if (note.section !== section.id) continue
			rows.push(noteRow(note.id))
			notes.push(note.id)
		}
	}

	rowIds.value = rows
	visibleNoteIds.value = notes
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

	const top = region.getBoundingClientRect().top
	for (const id of visibleNoteIds.value) {
		const element = rowElement(noteRow(id))
		if (!element) continue
		const offset = element.getBoundingClientRect().top - top
		if (offset >= 0) return { noteId: id, offset }
	}
	return null
}

/**
 * Prunes what no longer exists and relocates focus, against both the snapshot
 * and the freshly synced document.
 */
function reconcile(snap: SelectionSnapshot) {
	const live = new Set(visibleNoteIds.value)

	setSelection(selectedIds.value.filter((id) => live.has(id)))
	if (anchorId.value && !live.has(anchorId.value)) anchorId.value = null

	const rows = rowIds.value
	if (focusedId.value && rows.includes(focusedId.value)) {
		// The row survived — possibly reordered or moved to another section. Focus
		// follows it by id; the scroll restore below brings it back into view.
		return
	}

	const formerNote = rowNoteId(snap.focusedId)
	focusedId.value = formerNote ? nearestSurvivor(snap.noteIds, formerNote) : null

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
function nearestSurvivor(formerOrder: string[], formerNoteId: string): string | null {
	const live = new Set(visibleNoteIds.value)
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
	if (rowElement(snap.activeRowId)) return

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

export function useSelection() {
	return {
		selectedIds: readonly(selectedIds),
		focusedId: readonly(focusedId),
		focusedNoteId,
		anchorId: readonly(anchorId),
		rowIds: readonly(rowIds),
		visibleNoteIds: readonly(visibleNoteIds),
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
