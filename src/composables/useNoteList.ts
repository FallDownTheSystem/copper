/**
 * How the list is presented: which notes it shows, and in what order.
 *
 * Two pieces of view state live here — the done filter and the sort mode, both
 * document-wide — plus the two facts about the document that `useSelection`'s
 * walk needs in order to apply them.
 *
 * **One-directional over the document, in the same way `useSelection`,
 * `useNoteSearch` and `useSections` are.** It never assigns `space`; the document
 * arrives through `rebuild`, called from `useSpace.applyDocument` beside
 * `search.rebuild`. That is what lets `useSelection` read it inside `orders`
 * without a module cycle. The one adapter it touches is `useSettings`, and only
 * for its own two controls — nothing about the *document* flows through it.
 *
 * Module scope, not per-caller: the header's filter and sort controls, the grid's
 * traversal orders and the drag guards all have to be looking at the same state.
 *
 * **Both controls are remembered across launches** (user ruling 2026-08-12),
 * which reversed the session-only rule this header used to state. The refs here
 * stay the single source of truth the panel renders from; `settings.json` is
 * only the memory of them. Values flow out through the setters — each persists
 * itself via `useSettings.rememberListView`, fire-and-forget — and flow in
 * exactly once, through `hydrate` at boot. Nothing about either reaches the
 * `.copper` document: which notes a *reader* is looking at is not a fact about
 * the notes, and a shared or version-controlled space must not churn because
 * someone narrowed their view.
 *
 * A space switch no longer resets them, for the same ruling: a switch clearing
 * what a restart preserves would make one control obey two different rules, and
 * neither value names a section or a note that could go stale with the document.
 *
 * **Why the document data lives here and not in the walk.** `documentGroups` in
 * `useSelection` carries ids only, so neither `done` nor `created` is reachable
 * from inside `orders`. Rebuilding these two indexes from the applied document is
 * the same seam `useNoteSearch` uses for exactly the same reason, and it keeps
 * the filter and the sort re-evaluating when the *control* changes rather than
 * only when the document does.
 */

import { parseCreated, type SortMode } from '@/lib/noteTime'

import { useSettings } from './useSettings'

import type { Settings, SpaceView } from './useSpace'

export type { SortMode }

/**
 * Three views of one document, and the default is the whole of it.
 *
 * **`all` is what the panel opens on**, because the resting view of a capture
 * tool should be the document as it stands rather than an edited version of it. A
 * note that disappears the moment it is ticked is a note the user has to work out
 * a control to get back, and the panel would be answering a question about tidying
 * up that nobody asked on the way in. `todo` drops the finished notes and `done`
 * is the review scope the bulk delete acts in; both are one press away.
 *
 * The order is the cycle the button walks, and it is everything → unfinished →
 * finished. The resting view leads, so the first press is the one a reader
 * arriving at a worked-through list actually wants, and every press from there
 * narrows rather than jumping between two unrelated scopes. Three presses come
 * back where they started.
 */
export type DoneFilter = 'todo' | 'done' | 'all'

const doneFilter = ref<DoneFilter>('all')

/** The cycle, in one place: the button, its label and the empty state all have to
 *  agree about where a press goes. */
const NEXT_FILTER = {
	all: 'todo',
	todo: 'done',
	done: 'all',
} as const satisfies Record<DoneFilter, DoneFilter>

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

/**
 * Whether the view is a *subset* of the document, which is the question every
 * consumer other than the button itself is actually asking.
 *
 * Two of the three states narrow, and they narrow in opposite directions — so
 * "is the done filter on" is no longer answerable by `doneOnly`. Both keep
 * every section heading on screen and hide only notes, and both keep
 * reordering alive (user rulings 2026-08-12): a heading is how an emptied
 * section is still reached, and a filtered move anchors to its visible
 * neighbours — `useNoteActions.documentIndex`. Only `all` renders the whole
 * document.
 */
const filtersByDone = computed(() => doneFilter.value !== 'all')

/** Document-wide, and deliberately not `useNoteActions.doneCount`, which is the
 *  active section's and belongs to the bulk delete. The button offers a view of
 *  every done note there is, so the number beside it has to be that one. */
const doneTotal = computed(() => doneIds.value.size)

/**
 * The other two views' sizes, on the same document-wide scale and for the same
 * reason: the button offers all three, so one of them counted differently would
 * be a number the press does not deliver.
 *
 * `createdAt` is the note census. `rebuild` writes one entry per note in the
 * applied document — every note, not only the ones carrying a parseable date —
 * so its size is the document's note count without a second walk, and the
 * unfinished notes are what is left once the done ones come out.
 */
const allTotal = computed(() => createdAt.value.size)
const todoTotal = computed(() => allTotal.value - doneTotal.value)

const nextDoneFilter = computed(() => NEXT_FILTER[doneFilter.value])

function isDone(noteId: string) {
	return doneIds.value.has(noteId)
}

/** The membership test the walk applies per note. `all` admits everything; the
 *  other two are the two halves of the same predicate. */
function passesDoneFilter(noteId: string) {
	const mode = doneFilter.value
	return mode === 'all' || isDone(noteId) === (mode === 'done')
}

function createdOf(noteId: string): number | null {
	return createdAt.value.get(noteId) ?? null
}

/** Whether the order is computed, and therefore whether a drop index or an
 *  Alt+Arrow step means anything anywhere. The drag guards and the grip both ask
 *  this. */
const isSorted = computed(() => sortMode.value !== 'manual')

/** The write half of the controls' memory. Fire-and-forget on purpose: the ref
 *  has already changed and the panel with it, so there is nothing for a caller
 *  to await — a failed write costs the *memory* of the position, never the
 *  position itself, and `rememberListView` logs it. */
const { rememberListView } = useSettings()

function setSort(mode: SortMode) {
	if (sortMode.value === mode) return
	sortMode.value = mode
	void rememberListView({ sortMode: mode })
}

function setDoneFilter(next: DoneFilter) {
	if (doneFilter.value === next) return
	doneFilter.value = next
	void rememberListView({ doneFilter: next })
}

function cycleDoneFilter() {
	setDoneFilter(nextDoneFilter.value)
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
 * The read half of the controls' memory, and deliberately the only one: called
 * from `useSpace.load` once the file's answer is in, before the first document
 * lands. Everything after boot flows the other way, through the setters.
 *
 * Narrowed here rather than trusted, the same split `theme` and `motion` use in
 * `useSettings`: the store repairs a wrong *type*, and a name nothing recognises
 * collapses to the default on read. Writes nothing back — echoing a repaired
 * value into the file it just came from is `pullSettings`' anti-pattern.
 *
 * `null` restores the defaults, which is what the tests use where they used to
 * call `reset()`.
 */
function hydrate(settings: Pick<Settings, 'doneFilter' | 'sortMode'> | null | undefined) {
	const filter = settings?.doneFilter
	doneFilter.value = filter === 'todo' || filter === 'done' ? filter : 'all'
	const sort = settings?.sortMode
	sortMode.value = sort === 'oldest' || sort === 'newest' ? sort : 'manual'
}

export function useNoteList() {
	return {
		doneFilter: readonly(doneFilter),
		doneOnly,
		filtersByDone,
		doneTotal,
		todoTotal,
		allTotal,
		nextDoneFilter,
		setDoneFilter,
		cycleDoneFilter,
		passesDoneFilter,
		isDone,
		createdOf,
		sortMode: readonly(sortMode),
		isSorted,
		setSort,
		rebuild,
		hydrate,
	}
}
