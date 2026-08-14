/**
 * Selection, the roving focus target, and the reconciliation that keeps both
 * pointing at notes that still exist.
 *
 * Deliberately one-directional: nothing here imports `useSpace` at runtime, and
 * the document arrives through `syncDocument`. That keeps the whole module pure
 * over its input and unit-testable without mocking any IPC, and it is what lets
 * `useSpace` call into it during reconciliation without a module cycle.
 *
 * Three orders exist and must not be conflated. `rowIds` is every focusable row
 * in visual order, section headers included — arrow keys traverse this.
 * `visibleNoteIds` is note rows only in flattened order — pointer selection and
 * Shift ranges operate on this, and conflating it with `rowIds` is what breaks
 * Shift+Arrow across a section boundary. `actionableNoteIds` is the same list
 * without the collapse filter — what an *action* may target, and what Ctrl+A
 * selects, because folding a section shut hides rows rather than narrowing scope.
 */

import { runningMotions } from '@/lib/motion'
import { sortByCreated } from '@/lib/noteTime'

import { useNoteList } from './useNoteList'
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

/** Shallow for the same reason `documentGroups` below is: `setSelection` is the
 *  only writer and it always replaces the array wholesale, so a deep `ref` pays
 *  a get trap and a dependency registration per id to observe a mutation that
 *  never happens — on a list that reaches 200 and is re-read by `selectedSet`,
 *  `isSelected` and every reconciliation.
 *
 *  Typed `readonly` because that is what makes the claim above enforced rather
 *  than merely stated: a `shallowRef<string[]>` lets `selectedIds.value.push()`
 *  typecheck, and an in-place mutation is precisely the thing shallowness stops
 *  anyone from seeing. */
const selectedIds = shallowRef<readonly string[]>([])
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
const { filtersByDone, passesDoneFilter, createdOf, sortMode } = useNoteList()

/**
 * Both orders are filtered, and filtering only one of them is the single easiest
 * thing here to get half-right. `visibleNoteIds` drives selection, ranges and
 * Ctrl+A; `rowIds` drives the arrow keys and includes section header rows. Filter
 * only the first and `ArrowDown` still stops on the header rows of sections the
 * list has removed from the DOM, and on notes that no longer match.
 *
 * A section a search leaves with no surviving note is dropped entirely, header
 * included, which is what makes a result's origin visible without a dozen empty
 * headings. The done filter deliberately does not get the same treatment — see
 * the walk below.
 *
 * **Collapse is applied in this same walk, and has to be.** `visibleGroups` is
 * what the list renders, so a section filtered out here is one whose rows are not
 * in the DOM — which is exactly the condition the arrow keys and the roving
 * `tabindex` must agree about. Its *header* stays, unlike a search miss: it is the
 * control that expands it again. And unlike a search miss, collapsing never
 * touches the selection — `reconcile` prunes against the whole document, so a
 * selected note inside a collapsed section is still a target for `Ctrl+C`.
 *
 * **Task-014's ranking is applied inside each section, and the sections do not
 * move.** The alternative readings were both worse. Flattening the list during a
 * search would drop the section headers that make a result's origin visible, and
 * they are focusable rows in this same order — so it would change what `rowIds`
 * means, on every keystroke, while the collapse walk, the drag guards and the
 * roving-focus watcher all read it. Reordering the *sections* by their best note
 * would make the headings themselves jump around as the query is typed. Ranking
 * within a section moves only rows that are already siblings, and leaves
 * membership — the thing the roving-focus watcher reacts to — untouched.
 *
 * It also leaves `actionable` alone. That order is the document's by contract,
 * and ranking is a presentation of the same set rather than a re-ordering of it.
 *
 * **Task-016's done filter narrows like a search, and its sort reorders like the
 * ranking.** The filter is on the `actionable` side of the split rather than the
 * collapse side, and the reasoning is the one this file already states: a query
 * narrows what an action targets, folding does not. Filtering to done is a scope
 * the user chose in order to act on it, so `Ctrl+A` there selects the done notes
 * and nothing else. (The bulk delete is the deliberate exception and reads the
 * document instead — see `deleteDoneInSection`, which must not be narrowed by a
 * search the way a selection legitimately is.)
 *
 * **Two of that filter's three states narrow, and the default is one of them.**
 * `todo` hides the done notes and `done` hides the rest, so the rule above now
 * applies to the view the panel opens in: `Ctrl+A` on an ordinary list selects
 * the unfinished notes, which is the set the visible list is offering. A section
 * left with no survivor keeps its heading under either of them (user ruling
 * 2026-08-12, reversing the search-miss treatment it used to share): the filter
 * is a resting view, and a heading is the row that lets a filtered-empty
 * section be reached and captured into. Only a search miss still drops the
 * heading.
 *
 * The sort applies to the rows only, exactly as the ranking does, and for the
 * same reason: it is a presentation of the set, and a multi-note copy out of a
 * newest-first list must still come out in document order. **An explicit sort
 * outranks the search ranking** where both apply — relevance is implicit and a
 * sort mode is something the user went and chose. It is one mode for the whole
 * document, but it is applied *inside* each group here: sections keep their own
 * membership and their own place, and only the notes within one move.
 */
const orders = computed(() => {
	const matched = matchedIds.value
	const filtered = filtersByDone.value
	const mode = sortMode.value
	const groups: { sectionId: string; noteIds: string[] }[] = []
	const rows: string[] = []
	const notes: string[] = []
	const actionable: string[] = []

	for (const group of documentGroups.value) {
		// At most one copy, and only when something is actually narrowing: with no
		// query and no filter this stays the document's own array, which is what
		// makes the reordering below careful about mutating in place.
		let members = group.noteIds
		if (matched) members = members.filter((id) => matched.has(id))
		if (filtered) members = members.filter((id) => passesDoneFilter(id))
		// Only a search drops a section with no survivor. The done filter keeps the
		// heading (user ruling 2026-08-12): it is the row that lets the section be
		// reached — focused, activated, captured into — while the filter hides its
		// notes, and a user who rests in the todo view would otherwise find a
		// finished section unreachable exactly when they want to add to it. A query
		// is different: it asks "where is this note", and a heading with no answer
		// under it is noise.
		if (matched && members.length === 0) continue

		// **`actionable` is filled before the ranking, and from the unsorted list.**
		// Its contract is document order — every consumer that acts on several notes
		// at once relies on it, and a multi-note copy or `Copy as Markdown` must not
		// come out in the order a search happened to rank them. Ranking is a
		// *presentation* of the same set, so it applies to the rows and to nothing
		// else.
		for (const id of members) actionable.push(id)

		// `sort` is stable, so notes that score the same keep the document order they
		// arrived in — which is what makes a query produce the same list twice rather
		// than one that reshuffles its ties. In place, on the array `filter` has
		// already copied; a query is what guarantees that copy exists.
		if (matched && mode === 'manual') {
			members.sort((a, b) => (matched.get(b) ?? 0) - (matched.get(a) ?? 0))
		}
		// Returns a new array, so it is safe over the document's own when nothing
		// above copied it.
		const ordered = mode === 'manual' ? members : sortByCreated(members, createdOf, mode)

		const folded = isCollapsed(group.sectionId)
		groups.push({ sectionId: group.sectionId, noteIds: folded ? [] : ordered })
		rows.push(sectionRow(group.sectionId))
		if (folded) continue
		for (const id of ordered) {
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
 * What an *action* may target: document order, narrowed by the search query and
 * by the done filter, and by nothing else.
 *
 * Distinct from `visibleNoteIds`, and the distinction is the point. A search
 * narrows what an action targets — that is what a query means — and the done
 * filter narrows it for the same reason, being a scope the user chose in order to
 * act on it. **Collapsing does not**: it folds rows away, and a note the user
 * selected before folding its section is still a note they selected. Targeting
 * `visibleNoteIds` made
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

/**
 * Over `actionableNoteIds`, not `visibleNoteIds`.
 *
 * The difference is a collapsed section, and taking the visible order made Ctrl+A
 * silently skip every note inside one — while `Ctrl+C` on the result then
 * happily targeted notes in collapsed sections that had been selected some other
 * way. Select-all now means the same thing every other action already means by
 * `actionableNoteIds`: a query narrows what an action reaches, folding a section
 * shut does not.
 */
function selectAll() {
	setSelection([...actionableNoteIds.value])
}

/**
 * The notes of one section an action may target — `actionableNoteIds`' rule
 * narrowed to a single group, so a collapsed section still answers with its
 * notes while an active query and an active done filter both narrow them.
 */
function actionableInSection(sectionId: string): string[] {
	const matched = matchedIds.value
	const group = documentGroups.value.find((entry) => entry.sectionId === sectionId)
	if (!group) return []
	const members = matched ? group.noteIds.filter((id) => matched.has(id)) : group.noteIds
	return filtersByDone.value ? members.filter((id) => passesDoneFilter(id)) : members
}

/**
 * The section context menu's `Select all`, and the first thing there that writes
 * the selection.
 *
 * Focus lands on the **header** rather than on the first note, and not only
 * because the section may be collapsed and have no note rows at all: the target
 * rule in `useNoteActions` reads a focused header as "take the selection", which
 * is exactly what a copy or a delete after this should do. Landing on the first
 * note would be indistinguishable from the user having clicked it.
 */
function selectSection(sectionId: string) {
	const ids = actionableInSection(sectionId)
	setSelection(ids)
	anchorId.value = ids[0] ?? null

	const key = sectionRow(sectionId)
	if (rowIds.value.includes(key)) takeRow(key)
}

function clear() {
	setSelection([])
	anchorId.value = null
}

function focusRow(key: string | null) {
	focusedId.value = key
}

/** Landing on a note selects it; landing on a header clears the selection.
 *  Every arrow, Home and End path ends here.
 *
 *  The header used to leave the selection alone, and what that looked like was
 *  a bug (user report, 2026-08-10): plain arrows move focus and selection
 *  together, so the note the arrow just left kept its 2px ring while the
 *  heading wore the focus outline — two things on screen claiming to be where
 *  the user is. Deliberate detours keep their selection by not coming through
 *  here: Ctrl+Arrow is `moveFocusOnly`, and the section menu's Select-all is
 *  `selectSection`, which takes the header row *after* writing the selection. */
function landOn(key: string | undefined) {
	if (!key) return
	const note = rowNoteId(key)
	if (note) select(note)
	else {
		clear()
		focusedId.value = key
	}
}

/** The row `delta` steps from the roving target over `rowIds`, headers included,
 *  clamping at both ends rather than wrapping. A list with no rows has no
 *  answer, and neither has a caller. */
function rowAt(delta: number): string | undefined {
	const rows = rowIds.value
	if (rows.length === 0) return undefined

	const current = focusedId.value ? rows.indexOf(focusedId.value) : -1
	const next = Math.min(rows.length - 1, Math.max(0, current === -1 ? 0 : current + delta))
	return rows[next]
}

/** Arrow: the roving target moves and the selection follows it. */
function moveFocus(delta: number) {
	landOn(rowAt(delta))
}

/**
 * Ctrl+Arrow: the same traversal with the selection left exactly as it is.
 *
 * **The missing half of discontiguous keyboard selection.** `Ctrl+Space` toggles
 * the focused note without disturbing the rest, but every way of *reaching*
 * another note replaced the selection on arrival — so the two could never be
 * combined and the discontiguous case was pointer-only. Focus and selection are
 * separate pieces of state here; this is the one caller that moves one without
 * the other.
 *
 * A traversal of its own rather than a flag through `landOn`, because what it
 * skips is the whole of `landOn`: no `select`, and no anchor either. The anchor
 * is the origin a `Shift` range grows from, and moving it here would make
 * "arrive somewhere quietly" silently re-aim the next `Shift+Arrow` — where
 * leaving it put means a range still grows from the note the user last acted on.
 */
function moveFocusOnly(delta: number) {
	const key = rowAt(delta)
	if (key) focusedId.value = key
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

/**
 * One of those gestures arrived: the reader owns the viewport now, and both
 * things the app was going to do to it are withdrawn.
 *
 * The pin is the obvious one. **The unflushed reveal is the other, and it has to
 * expire here or nowhere** — a request that could not find its row keeps waiting,
 * and a reader who has scrolled, clicked or arrowed since is the clearest signal
 * that its moment has passed. Without this, a note captured into a collapsed or
 * filtered-away section stayed pending indefinitely and jumped the list out from
 * under them at whatever unrelated moment finally made the row renderable.
 */
function readerTookOver() {
	pinning = false
	pendingReveal = null
}

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
			region.addEventListener(name, readerTookOver, { passive: true })
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

/** A pixel of slack on the pinned test below, for the same reason
 *  `BOTTOM_SLACK` has some: an unpinned heading and its group share one top
 *  edge exactly, but the two rects are rounded independently at a fractional
 *  device pixel ratio. */
const PINNED_SLACK = 1

/**
 * Scrolls a row into view, with the one exception a pinned heading creates.
 *
 * **A pinned heading is already at the top of the region, so asking the row to
 * scroll there is asking for nothing.** `position: sticky` moves the painted box
 * without moving the layout, and `scrollIntoView` reads the painted one — so a
 * heading riding the top edge reports itself fully visible, and both landings
 * that matter become silent no-ops: `Make active section` on the section already
 * being read would not go to its start, and an arrow key onto the heading would
 * leave it hard against the region's edge with the outer half of its focus ring
 * clipped away.
 *
 * The section's own rowgroup is the fix and not a workaround: it is the heading's
 * layout position, un-pinned, because the heading is its first child. Scrolling
 * *it* is what un-pins the heading, and `start` rather than the caller's
 * alignment because there is only one landing a pinned heading can mean — the
 * top of the section it belongs to.
 *
 * Displacement is the test rather than a comparison against the region: a
 * heading is pinned exactly when it has been pushed down inside its own group,
 * which needs no second element and is false everywhere sticky is not in play.
 */
export function scrollRowIntoView(element: HTMLElement, block: ScrollLogicalPosition) {
	const group = element.hasAttribute('data-section-row')
		? element.closest<HTMLElement>('[data-section-id]')
		: null

	if (group) {
		const displaced =
			element.getBoundingClientRect().top - group.getBoundingClientRect().top > PINNED_SLACK
		if (displaced) {
			group.scrollIntoView({ block: 'start' })
			return
		}
	}

	element.scrollIntoView({ block })
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
 * focus deliberately always means. `focusRow` alone leaves the roving target
 * on a row that nothing is focused on.
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
 *  counts as at the bottom — harmless here, because `captureScroll` refuses to
 *  record an anchor for a region with no overflow at all. */
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

	// **A region with no overflow has no scroll position worth keeping.** The
	// reader is at 0 because there is nowhere else to be — `atBottom` reads true,
	// but that bottom is a phantom, and restoring it is not the no-op it looks
	// like: activating a section folds one section while another unfolds, the
	// leaving rows keep their height mid-fold, and the region is transiently
	// taller than either settled layout. `pinToBottom` re-asserting that phantom
	// bottom every frame shoved the top sections off screen and eased back with
	// the clamp as the fold drained (the section-activation flicker, 2026-08-14).
	// A document that grows past the viewport is the reveal's job to show, not a
	// restore's.
	if (region.scrollHeight - region.clientHeight <= BOTTOM_SLACK) return null

	// Tested first among the anchors, and it has to be: the note anchor below
	// would hold the list exactly where it is, which is right for a reader who
	// has scrolled up and wrong for one sitting at the end watching their own
	// captures land.
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
 * target has to name a row that is actually rendered.
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
	// the grid a roving target anyway: the target is where the arrow keys resume,
	// and an arrow pressed with no target would have nowhere to start from.
	// (Tab needs no help — every row is a permanent stop.)
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
 * The list transition unfolds a newly inserted row from zero height across its
 * entry animation — so `scrollHeight` climbs for the whole animation.
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

		// Holding still is not the same as being finished. An unfolding row's
		// growth eases toward its end — fractions of a pixel per frame at the
		// tail, slowly enough for a stability test on its own to exit mid-entry.
		// Asking the running animations instead of guessing a duration is what
		// makes this exact.
		//
		// Motion only: the section band's row clip is scroll-driven and so is
		// *always* running, which asked the raw question would keep this loop
		// pinning until its cap on every capture.
		const running = runningMotions(region).length > 0

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
		// **A bottom the geometry cannot corroborate is a phantom, and pinning to
		// it is the section-activation flicker (2026-08-14).** The `snapshot` this
		// anchor came from runs when the store answers — milliseconds into the
		// fold a section activation starts — so its own no-overflow guard can be
		// defeated by the fold's transient height, and the anchor then rests on
		// the `stuckToBottom` latch alone, which a region that has never scrolled
		// holds vacuously true. The test is at this end because this moment knows
		// what the capture could not: a reader genuinely parked at the end of an
		// overflowing region has `scrollTop > 0`. At zero with overflow on screen,
		// the latch is lying, and the pin would chase the phantom bottom down
		// frame by frame as the fold drains.
		if (region.scrollTop <= BOTTOM_SLACK && !atBottom(region)) return
		pinToBottom(region)
		return
	}

	const element = rowElement(noteRow(anchor.noteId))
	if (!element) return

	const delta = element.getBoundingClientRect().top - region.getBoundingClientRect().top
	region.scrollTop += delta - anchor.offset
}

/**
 * A row the list owes the reader a look at — a note they just captured, a section
 * they just switched to — held until it can actually be shown.
 *
 * **Held rather than performed, because the panel is usually not on screen when
 * the note arrives.** A global capture lands in a hidden window, and a hidden
 * window's list may have no layout to scroll: the settings view swaps the list
 * out of the DOM entirely, and a panel that has not been revealed since launch has
 * a region of zero height. Both make a scroll a silent no-op, which is the one
 * outcome the request cannot have — "the user should always come back to the last
 * note they added" is a promise about the *next* time they look, not about now.
 * So every attempt that cannot land keeps the request, and it is tried again on
 * mount, when the scroll region gains a height, when the panel reports itself
 * visible, when a drag ends, and when the set of rendered rows changes.
 *
 * **It is not kept forever, and the bound is the reader rather than a clock.**
 * The row can be absent for reasons no reveal will ever fix on its own — a
 * collapsed section, a note the done filter hides, a section a query dropped —
 * so `readerTookOver` expires the request the moment they scroll, click or press
 * a key in the list. A stale reveal firing later would yank the viewport away
 * from someone who has since chosen where to look, which is a worse failure than
 * never scrolling at all.
 *
 * One slot, not a queue: two captures in a row mean the reader should be looking
 * at the second one, and a queue would walk them through history to get there.
 */
let pendingReveal: { key: string; block: ScrollLogicalPosition } | null = null

export function revealRow(key: string, block: ScrollLogicalPosition = 'nearest') {
	pendingReveal = { key, block }
	flushReveal()
}

/**
 * **Instant, never smooth, and that is not a reduced-motion compromise.**
 *
 * A smooth scroll runs over frames, and in a hidden window frames are throttled
 * or stopped altogether — so the one case this feature is *for* is the case
 * where a smooth scroll would be left half-finished. An instant scroll satisfies
 * `prefers-reduced-motion` by having nothing to reduce. And it never has a
 * moving target to chase: the motion guard below holds the flush until the list
 * transition's folds have finished, so the jump lands on geometry that is done
 * changing.
 */
export function flushReveal() {
	const wanted = pendingReveal
	if (!wanted || typeof document === 'undefined') return

	// A carried row is a gesture the reader is performing right now, and the drag's
	// own auto-scroll owns the region until they let go. Kept pending: the drop
	// puts the list back in a state where this means something again.
	if (document.querySelector('[data-dragging]')) return

	const region = scrollRegion()
	// No region, or one with no height — the settings view is up, or the panel has
	// never been laid out. Scrolling it would report success and do nothing.
	if (!region || region.clientHeight === 0) return

	// The row exists in the document but not on screen: its section is collapsed,
	// the done filter or a query has hidden it, or the patch that renders it has
	// not run yet. Kept pending — the watcher on `rowIds` below tries again the
	// moment it becomes renderable, and a reader's own gesture is what expires it.
	const element = rowElement(wanted.key)
	if (!element) return

	// **The bottom pin is better at this than a scroll is, so it keeps the case it
	// already owns.** A reader parked at the end who captures a note wants the end,
	// and `pinToBottom` re-asserts it every frame until the list stops growing —
	// where a single `scrollIntoView` lands once and is then left behind by the row
	// growing under it. Only the *last* row, and only while the pin is actually
	// running: a note inserted at the top has to be scrolled to whatever the reader
	// was doing.
	const rows = rowIds.value
	if (pinning && rows[rows.length - 1] === wanted.key) {
		pendingReveal = null
		return
	}

	// **A scroll computed against a moving layout lands where the layout will not
	// be.** The fold transition keeps a collapsing section's rows in flow while
	// their height animates to zero, so mid-fold the list is transiently taller
	// than its settled self — `scrollIntoView` happily spends that phantom height,
	// and when the rows finish leaving, the region clamps the overshoot back in
	// one frame (the section-activation flicker, 2026-08-14). Kept pending,
	// exactly like a hidden region: the reader's own gesture can still expire it,
	// and the retry lands on geometry that has stopped moving.
	const motions = runningMotions(region)
	if (motions.length > 0) {
		flushRevealWhenSettled(motions)
		return
	}

	pendingReveal = null
	// Anything else the pin is doing is now wrong: it would drag the list back to
	// the bottom over the next frames and undo this. Clearing the flag stops its
	// settle loop at the next frame rather than racing it.
	pinning = false
	scrollRowIntoView(element, wanted.block)
}

/** One settle-watch at a time: every waiter funnels back through `flushReveal`,
 *  which re-reads the world from scratch — including any motion that started
 *  while this one waited — so a second watcher could only duplicate the retry. */
let settling = false

/** `allSettled` rather than `all`: a cancelled animation rejects `finished`,
 *  and a cancellation is just another way for the layout to stop moving. */
function flushRevealWhenSettled(motions: Animation[]) {
	if (settling) return
	settling = true
	void Promise.allSettled(motions.map((motion) => motion.finished)).then(() => {
		settling = false
		flushReveal()
	})
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
	// So does an unflushed reveal: its row key names a note or a section in a
	// document nobody is looking at any more, and the id could even be reused.
	pendingReveal = null
}

/**
 * The search filter can unmount the row the roving target names — and the row
 * holding DOM focus with it.
 *
 * Saying focus never moves would be unsatisfiable — the element is gone — and
 * the roving target is where the arrow keys resume, so it must always point at
 * a row that is actually rendered. It moves to the nearest remaining match by
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

/**
 * The rendered rows changed, which is the one signal that a reveal waiting on a
 * *row* rather than on a *region* can finally land.
 *
 * The list's own triggers all watch the panel — mount, visibility, a drop — and
 * none of them fires when a section is expanded, a query is cleared or the done
 * filter widens to the view that holds the note. Those are precisely the ways a
 * captured note is in the document and not on screen, and the default view hides
 * done notes, so a note marked done on arrival starts out in one of them.
 *
 * Deferred a tick because this fires on the order, and the DOM catches up after
 * it: `rowElement` would still be looking at the previous render. Guarded on the
 * pending request so an ordinary keystroke's filtering does not queue a tick's
 * work to do nothing with.
 */
watch(rowIds, () => {
	if (!pendingReveal) return
	void nextTick(() => flushReveal())
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
		// `shallowReadonly`, not `readonly`: the deep form re-proxies the array on
		// every `.value` read, which is exactly the cost the `shallowRef` above
		// removes.
		selectedIds: shallowReadonly(selectedIds),
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
		selectSection,
		clear,
		focusRow,
		moveFocus,
		moveFocusOnly,
		focusFirst,
		focusLast,
		syncDocument,
		snapshot,
		reconcile,
		restoreDom,
		resetForNewSpace,
	}
}
