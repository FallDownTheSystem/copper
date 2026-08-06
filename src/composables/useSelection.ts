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
import { useSections } from './useSections'
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
 *  holds.
 *
 *  Shallow because `syncDocument` only ever replaces it wholesale. A deep `ref`
 *  proxies every group and every id array, and `orders` walks all of them on
 *  every keystroke — paying a get trap and a dependency registration per note to
 *  observe a mutation that never happens. */
const documentGroups = shallowRef<{ sectionId: string; noteIds: string[] }[]>([])

const { matchedIds } = useNoteSearch()
const { isCollapsed } = useSections()

/**
 * Both orders are filtered, and filtering only one of them is the single easiest
 * thing here to get half-right. `visibleNoteIds` drives selection, ranges and
 * Ctrl+A; `rowIds` drives the arrow keys and includes section header rows. Filter
 * only the first and `ArrowDown` still stops on the header rows of sections the
 * list has removed from the DOM, and on notes that no longer match.
 *
 * A section with no surviving note is dropped entirely, header included, which is
 * what makes a result's origin visible without a dozen empty headings.
 *
 * **Collapse is applied in this same walk, and has to be.** `visibleGroups` is
 * what the list renders, so a section filtered out here is one whose rows are not
 * in the DOM — which is exactly the condition the arrow keys and the roving
 * `tabindex` must agree about. Its *header* stays, unlike a search miss: it is the
 * control that expands it again. And unlike a search miss, collapsing never
 * touches the selection — `reconcile` prunes against the whole document, so a
 * selected note inside a collapsed section is still a target for `Ctrl+C`.
 */
const orders = computed(() => {
	const matched = matchedIds.value
	const groups: { sectionId: string; noteIds: string[] }[] = []
	const rows: string[] = []
	const notes: string[] = []
	const actionable: string[] = []

	for (const group of documentGroups.value) {
		const members = matched ? group.noteIds.filter((id) => matched.has(id)) : group.noteIds
		if (matched && members.length === 0) continue

		// One walk for both orders. `actionable` takes every match; the collapse test
		// is the whole difference between it and the rows below.
		const folded = isCollapsed(group.sectionId)
		groups.push({ sectionId: group.sectionId, noteIds: folded ? [] : members })
		rows.push(sectionRow(group.sectionId))
		for (const id of members) {
			actionable.push(id)
			if (folded) continue
			rows.push(noteRow(id))
			notes.push(id)
		}
	}

	return { groups, rows, notes, actionable }
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

/**
 * What an *action* may target: document order, filtered by the search query and
 * by nothing else.
 *
 * Distinct from `visibleNoteIds`, and the distinction is the point. A search
 * narrows what an action targets — that is what a query means. **Collapsing does
 * not**: it folds rows away, and a note the user selected before folding its
 * section is still a note they selected. Targeting `visibleNoteIds` made
 * copy, delete, mark-done, merge, `Move to ▸` and the `$EDITOR` handoff into
 * silent no-ops the moment a section was collapsed, which is the opposite of
 * what the comment above `orders` promises.
 */
const actionableNoteIds = computed(() => orders.value.actionable)

const selectedSet = computed(() => new Set(selectedIds.value))
const focusedNoteId = computed(() => rowNoteId(focusedId.value))

/**
 * Where the scroll region was, in a form that survives a document whose content
 * changed height.
 *
 * `bottom` is a position in its own right rather than a note plus an offset: a
 * list sitting at its bottom edge has a topmost visible note like any other, and
 * holding *that* note's offset is exactly what left a note added from the
 * composer below the fold.
 */
export type ScrollAnchor = { kind: 'bottom' } | { kind: 'note'; noteId: string; offset: number }

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
	scroll: ScrollAnchor | null
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

/**
 * The latched half of "is the list parked at its bottom edge" — see
 * `isStuckToBottom` for the predicate itself.
 *
 * Held across scroll events rather than re-measured when a document arrives,
 * and that distinction *is* the fix. The region's height
 * is not constant: the composer grows as the user types and collapses again when
 * the note is submitted, so `scrollHeight - scrollTop - clientHeight` measured at
 * submit — the one instant the composer is at its tallest — reports tens of
 * pixels for a reader who has not scrolled at all. Measured that way a
 * five-line capture classified as "scrolled up", took a note anchor, and left
 * the new note below the fold.
 *
 * A scroll event fires only when `scrollTop` actually moves, which a composer
 * growing underneath the region never does. So this survives typing and is
 * released only by a reader who genuinely scrolls away.
 */
let stuckToBottom = true
let trackedRegion: HTMLElement | null = null
/** Set while `pinToBottom` is driving the region, so the scroll events its own
 *  writes and the reflows around them produce are not mistaken for a reader. */
let pinning = false

/**
 * The gestures that mean *the reader* is scrolling, as opposed to the list
 * reflowing underneath them.
 *
 * This distinction is load-bearing. Clamping a note that has just been measured
 * shrinks and regrows the list several times over ~180ms, and every one of those
 * steps fires a `scroll` event; treating those as a reader gave up the pin
 * halfway through the cascade and left the list short. `keydown` is included and
 * reaches this element from a focused row, while the composer sits outside the
 * region — so submitting a note never cancels its own pin.
 */
const RELEASE_EVENTS = ['wheel', 'touchmove', 'keydown', 'pointerdown'] as const

function scrollRegion() {
	if (typeof document === 'undefined') return null
	const region = document.querySelector<HTMLElement>('[data-scroll-region]')
	if (region && region !== trackedRegion) {
		trackedRegion = region
		stuckToBottom = atBottom(region)
		// Passive: these handlers only read. Never removed, because this is the
		// panel's one scroll surface and it outlives every document.
		region.addEventListener(
			'scroll',
			() => {
				if (pinning) return
				stuckToBottom = atBottom(region)
			},
			{ passive: true },
		)
		for (const name of RELEASE_EVENTS) {
			region.addEventListener(name, () => (pinning = false), { passive: true })
		}
	}
	return region
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
 * The roving target and DOM focus together, which is what a caller that moves
 * focus deliberately always means. `focusRow` alone leaves the grid's
 * `tabindex="0"` on a row that nothing is focused on.
 */
export function takeRow(key: string) {
	focusRow(key)
	focusRowSoon(key)
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

/** A couple of pixels of slack. At a fractional device pixel ratio the three
 *  metrics do not cancel exactly, so a region genuinely scrolled to its end
 *  reports a sub-pixel remainder. A region too short to scroll reports zero and
 *  counts as at the bottom, which is correct: pinning it is a no-op. */
const BOTTOM_SLACK = 2

function atBottom(region: HTMLElement) {
	return region.scrollHeight - region.scrollTop - region.clientHeight <= BOTTOM_SLACK
}

/**
 * The two signals are deliberately combined with `||` rather than either one
 * being trusted alone.
 *
 * The measurement is *sufficient but not necessary*, so a reader who scrolls
 * back down re-arms stickiness immediately and without depending on an event
 * having been delivered. The latch covers the one case the measurement cannot
 * see: the composer growing under the region shrinks the viewport without moving
 * `scrollTop`, which reads as "scrolled up" for a reader who never scrolled.
 */
function isStuckToBottom(region: HTMLElement) {
	return atBottom(region) || stuckToBottom
}

/**
 * Anchors on a visible note's id plus its pixel offset rather than raw
 * `scrollTop`, because an external edit can change the height of content above
 * the viewport and leave a restored `scrollTop` pointing somewhere else.
 */
function captureScroll(): ScrollAnchor | null {
	const region = scrollRegion()
	if (!region) return null

	// Tested first, and it has to be: the note anchor below would hold the list
	// exactly where it is, which is right for a reader who has scrolled up and
	// wrong for one sitting at the end watching their own captures land.
	if (isStuckToBottom(region)) return { kind: 'bottom' }

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
		if (offset >= 0) return { kind: 'note', noteId: id, offset }
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

/** Frames of an unchanged `scrollHeight` before the list counts as settled. */
const STABLE_FRAMES = 5
/** Hard stop, so a list that never stops changing cannot hold the pin forever. */
const SETTLE_CAP_MS = 2000

/**
 * Re-asserted every frame until the list stops changing height, because the
 * pin's own target keeps moving after it lands.
 *
 * auto-animate scales a newly inserted row from `.98` to `1` across its entry
 * animation, and a transformed box still contributes to its scroll container's
 * *scrollable overflow* — so `scrollHeight` climbs for the whole animation.
 * Clamping a freshly measured note shrinks and regrows the list several times on
 * top of that. Measured in WebView2 at 175% scaling: the pin landed correctly,
 * then the list grew and left `scrollTop` 12.57px below its true maximum, with
 * the new note's own bottom flush against the viewport and the list's 12px
 * bottom padding stranded below it — exactly the gap on the scrollbar.
 *
 * The exit condition is the list holding still, not a duration. A fixed window
 * was tried and is what left that 12.57px: the growth outran it whenever the
 * first frames after a launch were slow. `scrollHeight` is read once per frame
 * on a container that is being written to anyway, so this costs no extra layout.
 *
 * The loop re-reads `pinning` every frame rather than re-pinning blind, and a
 * reader's gesture clears it — so they take the list back mid-settle.
 */
function pinToBottom(region: HTMLElement) {
	region.scrollTop = region.scrollHeight
	stuckToBottom = true
	if (typeof requestAnimationFrame !== 'function') return

	pinning = true
	const cap = Date.now() + SETTLE_CAP_MS
	let lastHeight = -1
	let stable = 0

	const settle = () => {
		if (!pinning || !region.isConnected) return
		region.scrollTop = region.scrollHeight

		const height = region.scrollHeight
		if (height === lastHeight) stable++
		else {
			lastHeight = height
			stable = 0
		}

		// Holding still is not the same as being finished. auto-animate's entry
		// keyframes park the row at `scale(.98)` until the animation's halfway
		// point, so the list sits perfectly still for ~110ms and only then grows —
		// and a stability test on its own exits during that plateau. Asking the
		// running animations instead of guessing a duration is what makes this
		// exact.
		const running = region
			.getAnimations?.({ subtree: true })
			.some((animation) => animation.playState === 'running')

		if ((stable >= STABLE_FRAMES && !running) || Date.now() >= cap) {
			pinning = false
			return
		}
		requestAnimationFrame(settle)
	}

	requestAnimationFrame(settle)
}

function restoreScroll(anchor: ScrollAnchor) {
	const region = scrollRegion()
	if (!region) return

	if (anchor.kind === 'bottom') {
		pinToBottom(region)
		return
	}

	const element = rowElement(noteRow(anchor.noteId))
	if (!element) return

	const delta = element.getBoundingClientRect().top - region.getBoundingClientRect().top
	region.scrollTop += delta - anchor.offset
}

/** Space identity changed: ids mean something else now, so nothing carries. */
function resetForNewSpace() {
	setSelection([])
	focusedId.value = null
	anchorId.value = null
	// A different space opens at its end, exactly as a fresh load does, and any
	// pin still settling belongs to the document that just went away.
	stuckToBottom = true
	pinning = false
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
		actionableNoteIds,
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
