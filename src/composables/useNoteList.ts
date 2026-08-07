/**
 * How the list is presented: which notes it shows, and in what order.
 *
 * Two pieces of view state live here — the done filter, which is document-wide,
 * and the sort mode, which is per section — plus the two facts about the document
 * that `useSelection`'s walk needs in order to apply them.
 *
 * **One-directional, in the same way `useSelection`, `useNoteSearch` and
 * `useSections` are.** It imports no adapter, `invoke`s nothing and never assigns
 * `space`; the document arrives through `rebuild`, called from
 * `useSpace.applyDocument` beside `search.rebuild`. That is what lets
 * `useSelection` read it inside `orders` without a module cycle.
 *
 * Module scope, not per-caller: the header's filter control, the section menu's
 * sort submenu, the grid's traversal orders and the drag guards all have to be
 * looking at the same state.
 *
 * **Both are view state only**, for the reason `useSections` records about
 * collapse: they live in memory for the session, reset on a space switch, and
 * nothing about either reaches the `.copper` document or `settings.json`.
 * Persisting the sort would mean a new field on every `Section`, and task-003 §Q9
 * strips unknown keys on write — a schema change for something AC12 only asks to
 * survive "as long as the app is running".
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

/** Section id → mode, absent meaning `manual`. Not a `Map` mutated in place: a
 *  change has to be a new object for the computeds reading it to re-evaluate. */
const sortModes = ref(new Map<string, SortMode>())

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

function sortOf(sectionId: string): SortMode {
	return sortModes.value.get(sectionId) ?? 'manual'
}

/** Whether this section's order is computed, and therefore whether a drop index
 *  or an Alt+Arrow step means anything in it. The drag guards and the grip both
 *  ask this. */
function isSorted(sectionId: string) {
	return sortOf(sectionId) !== 'manual'
}

function setSort(sectionId: string, mode: SortMode) {
	const next = new Map(sortModes.value)
	// `manual` is the absent state rather than a stored one, so a section put back
	// to manual leaves no entry to prune later.
	if (mode === 'manual') next.delete(sectionId)
	else next.set(sectionId, mode)
	sortModes.value = next
}

function setDoneFilter(next: DoneFilter) {
	doneFilter.value = next
}

function toggleDoneFilter() {
	setDoneFilter(doneOnly.value ? 'all' : 'done')
}

/**
 * Brings both indexes and the sort map into line with a document that has just
 * been applied.
 *
 * The sort map is pruned in the same walk, for the reason `useSections.reconcile`
 * prunes the collapse set: an entry whose section no longer exists is dead weight
 * nothing can remove, and if the id is ever reintroduced — an undone section
 * delete restores exactly the id it removed — the section comes back mysteriously
 * sorted.
 *
 * The filter is deliberately **not** reset here. A document change is not a
 * change of intent, and clearing the filter under a capture that landed while the
 * user was reviewing done notes would take the view away mid-task.
 */
function rebuild(space: SpaceView | null) {
	if (!space) {
		doneIds.value = new Set()
		createdAt.value = new Map()
		// Pruned here too, and the reason is the contract rather than a bug anyone
		// can reach today: `rebuild` promises to bring the sort map into line with
		// the document it is given, and a null document has no sections at all — so
		// every mode in it names something that does not exist. Leaving them behind
		// on this one branch would make an exported function mean two different
		// things depending on its argument, which is the kind of asymmetry the next
		// caller inherits without reading for it. `reset` is still the epoch path;
		// this is only consistency.
		sortModes.value = new Map()
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

	if (sortModes.value.size === 0) return
	const live = new Set(space.sections.map((section) => section.id))
	if ([...sortModes.value.keys()].every((id) => live.has(id))) return
	sortModes.value = new Map([...sortModes.value].filter(([id]) => live.has(id)))
}

/**
 * Space identity changed: section ids address a different document now, and the
 * filter was a question asked about the document that just went away.
 *
 * This is the reset event AC3 asks for. The panel renders every section at once —
 * `activeSection` decides where a capture lands, not what is on screen — so there
 * is no "switch to a section" gesture inside a space for a per-section reset to
 * hang on. A space switch is the real one, and it is already where `useSections`
 * drops collapse.
 */
function reset() {
	doneFilter.value = 'all'
	sortModes.value = new Map()
}

export function useNoteList() {
	return {
		doneFilter: readonly(doneFilter),
		doneOnly,
		setDoneFilter,
		toggleDoneFilter,
		isDone,
		createdOf,
		sortOf,
		isSorted,
		setSort,
		rebuild,
		reset,
	}
}
