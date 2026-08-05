/**
 * Search over the notes of the active space.
 *
 * **The index is rebuilt from the applied document, never from an event.** This
 * is the one thing in this module that fails silently if it is wrong. Task-003
 * §8.4 does not emit `space-changed` for a command the frontend invoked — the
 * command's return value *is* the change — so an index subscribed to that event
 * would go stale after every local merge, move, mark-done, inline edit, undo and
 * redo, while staying perfectly correct for changes somebody else made. So
 * `useSpace.applyDocument` calls `rebuild()` directly, alongside the selection
 * and disclosure reconciliation it already drives, and that one call covers every
 * writer: the mount pull, a command's return value, a watcher reload, a capture
 * append, and Phase 6's space switch.
 *
 * The module is one-directional in the same way `useSelection` is: it imports no
 * other composable, the document arrives through `rebuild()`, and nothing here
 * knows selection exists. That is what lets `useSelection` filter its orders by
 * `matchedIds` without a cycle.
 */

import MiniSearch from 'minisearch'

import type { SpaceView } from './useSpace'

type IndexedNote = { id: string; body: string; section: string }

const SEARCH_OPTIONS = {
	// A partial last word still matches, so results narrow while typing rather
	// than vanishing until a word is finished.
	prefix: true,
	fuzzy: 0.2,
	// Every typed term must be present; two terms narrow rather than widen.
	combineWith: 'AND',
} as const

const query = ref('')

/**
 * A `MiniSearch` instance is a plain object, so a computed that reads it would
 * never re-evaluate when its contents changed. `rebuild()` bumps this and
 * `matchedIds` reads it, which makes the dependency explicit instead of relying
 * on `query` alone to retrigger — a rebuild with the query untouched is exactly
 * the case that would otherwise serve stale results.
 */
const indexRevision = ref(0)

let index = createIndex()

function createIndex() {
	return new MiniSearch<IndexedNote>({
		fields: ['body'],
		storeFields: ['section'],
		idField: 'id',
		searchOptions: SEARCH_OPTIONS,
	})
}

/**
 * Replaces the whole index rather than patching it.
 *
 * It is O(n) over tens of kilobytes, and it removes a class of index-drift bugs
 * outright: there is no add/remove/update path that can disagree with the
 * document. Worth revisiting past roughly 2,000 notes in one space.
 */
function rebuild(space: SpaceView | null) {
	index = createIndex()
	if (space) {
		index.addAll(
			space.notes.map((note) => ({ id: note.id, body: note.body, section: note.section })),
		)
	}
	indexRevision.value++
}

/** The trimmed query, since a query of only spaces matches everything and is
 *  indistinguishable from an empty field to the user. */
const activeQuery = computed(() => query.value.trim())

/**
 * `null` when no query is active — which is *not* the same as an empty set, and
 * every consumer branches on the difference: `null` means "do not filter",
 * an empty set means "filter, and nothing survived".
 */
const matchedIds = computed<Set<string> | null>(() => {
	const text = activeQuery.value
	if (text.length === 0) return null
	void indexRevision.value
	return new Set(index.search(text).map((result) => result.id))
})

/** The terms the highlighter paints. Split on whitespace to match how
 *  `combineWith: 'AND'` treats the query. */
const matchTerms = computed(() =>
	activeQuery.value.length === 0 ? [] : activeQuery.value.split(/\s+/).filter(Boolean),
)

const resultCount = computed(() => matchedIds.value?.size ?? 0)

const hasQuery = computed(() => activeQuery.value.length > 0)

function clearQuery() {
	query.value = ''
}

export function useNoteSearch() {
	return {
		query,
		activeQuery,
		hasQuery,
		matchedIds,
		matchTerms,
		resultCount,
		indexRevision: readonly(indexRevision),
		rebuild,
		clearQuery,
	}
}
