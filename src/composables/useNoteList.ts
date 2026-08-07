/**
 * How the list is presented: which notes it shows, and in what order.
 *
 * Two pieces of view state live here — the done filter and the sort mode, both
 * document-wide — plus the two facts about the document that `useSelection`'s
 * walk needs in order to apply them.
 *
 * **One-directional, in the same way `useSelection`, `useNoteSearch` and
 * `useSections` are.** It imports no adapter, `invoke`s nothing and never assigns
 * `space`; the document arrives through `rebuild`, called from
 * `useSpace.applyDocument` beside `search.rebuild`. That is what lets
 * `useSelection` read it inside `orders` without a module cycle.
 *
 * Module scope, not per-caller: the header's filter and sort controls, the grid's
 * traversal orders and the drag guards all have to be looking at the same state.
 *
 * **Both are view state only**, for the reason `useSections` records about
 * collapse: they live in memory for the session, reset on a space switch, and
 * nothing about either reaches the `.copper` document or `settings.json`. That
 * survived the sort becoming document-wide: a single mode would now fit
 * `settings.json` where a per-section map needed a schema change, but AC12 only
 * asks that it last "as long as the app is running", and a sort that outlived a
 * restart would greet the user with a list whose order they cannot account for.
 *
 * **Why the document data lives here and not in the walk.** `documentGroups` in
 * `useSelection` carries ids only, so neither `done` nor `created` is reachable
 * from inside `orders`. Rebuilding these two indexes from the applied document is
 * the same seam `useNoteSearch` uses for exactly the same reason, and it keeps
 * the filter and the sort re-evaluating when the *control* changes rather than
 * only when the document does.
 */

import { parseCreated, type SortMode } from '@/lib/noteTime'

import type { SpaceView } from './useSpace'

export type { SortMode }

/** Two states rather than three. "Active" is the complement of "done" and the
 *  unfiltered list already leads with it; a third control would divide the same
 *  set twice and give the header a segmented control where a toggle does. */
export type DoneFilter = 'all' | 'done'

const doneFilter = ref<DoneFilter>('all')

/**
 * One order for the whole document, applied *within* each section.
 *
 * **The scope of a sort and the scope of the setting are different questions**,
 * and only the second one changed: notes are still ordered inside their own
 * section and sections never interleave. What went away is the per-section map,
 * and with it a state where two sections on screen at once are ordered by
 * different rules — visible nowhere except on the header of each, and a
 * permanent question about which of them the drag grip disappeared for.
 */
const sortMode = ref<SortMode>('manual')

/**
 * The two document facts the walk needs, rebuilt wholesale on every applied
 * document.
 *
 * `shallowRef` for the reason `useNoteSearch`'s index is one: `rebuild` only ever
 * replaces them, and a deep ref would proxy every entry to observe a mutation
 * that never happens — on structures re-read by `orders` on every keystroke.
 *
 * `createdAt` holds the *parsed* value, so a sort walks numbers rather than
 * re-parsing a string per comparison, and `null` — absent or unparseable — is
 * resolved once per document instead of once per render.
 */
const doneIds = shallowRef(new Set<string>())
const createdAt = shallowRef(new Map<string, number | null>())

const doneOnly = computed(() => doneFilter.value === 'done')

function isDone(noteId: string) {
	return doneIds.value.has(noteId)
}

function createdOf(noteId: string): number | null {
	return createdAt.value.get(noteId) ?? null
}

/** Whether the order is computed, and therefore whether a drop index or an
 *  Alt+Arrow step means anything anywhere. The drag guards and the grip both ask
 *  this. */
const isSorted = computed(() => sortMode.value !== 'manual')

function setSort(mode: SortMode) {
	sortMode.value = mode
}

function setDoneFilter(next: DoneFilter) {
	doneFilter.value = next
}

function toggleDoneFilter() {
	setDoneFilter(doneOnly.value ? 'all' : 'done')
}

/**
 * Brings both indexes into line with a document that has just been applied.
 *
 * **Neither control is touched here**, and the sort's exemption is now the same
 * one the filter always had rather than a second rule. A document change is not a
 * change of intent: clearing the filter under a capture that landed while the
 * user was reviewing done notes would take the view away mid-task, and the sort
 * is a statement about how to read whatever the document turns out to hold.
 *
 * While the modes were per section this function also had to prune them — an
 * entry naming a deleted section was dead weight, and an undone section delete
 * restores exactly the id it removed, so the section came back mysteriously
 * sorted. One document-wide mode names no section and cannot go stale, which is
 * the pruning walk and its whole failure mode gone rather than relocated.
 */
function rebuild(space: SpaceView | null) {
	if (!space) {
		doneIds.value = new Set()
		createdAt.value = new Map()
		return
	}

	const done = new Set<string>()
	const created = new Map<string, number | null>()
	for (const note of space.notes) {
		if (note.done) done.add(note.id)
		created.set(note.id, parseCreated(note.created))
	}
	doneIds.value = done
	createdAt.value = created
}

/**
 * Space identity changed: both controls were answers about the document that just
 * went away.
 *
 * This is the reset event AC3 asks for. The panel renders every section at once —
 * `activeSection` decides where a capture lands, not what is on screen — so there
 * is no "switch to a section" gesture inside a space for a per-section reset to
 * hang on. A space switch is the real one, and it is already where `useSections`
 * drops collapse.
 */
function reset() {
	doneFilter.value = 'all'
	sortMode.value = 'manual'
}

export function useNoteList() {
	return {
		doneFilter: readonly(doneFilter),
		doneOnly,
		setDoneFilter,
		toggleDoneFilter,
		isDone,
		createdOf,
		sortMode: readonly(sortMode),
		isSorted,
		setSort,
		rebuild,
		reset,
	}
}
