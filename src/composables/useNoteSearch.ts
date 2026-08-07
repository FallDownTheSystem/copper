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
 *
 * **The index is a list, and task-014 is why.** Task-006 held a MiniSearch over
 * the bodies, which is a tokenised inverted index: it can answer "which notes
 * contain a word beginning with this" very fast, and it cannot answer "which
 * notes contain these characters in this order" at all, because the characters of
 * a subsequence match are not a token. Nothing else in the app used it, so the
 * dependency went with it. What remains is a walk over every body per keystroke,
 * which at this panel's sizes — a few hundred notes of a few kilobytes — is
 * cheaper than the index build it replaces.
 */

import { fuzzyMatch, fuzzyNeedle } from '@/lib/fuzzyMatch'

import type { SpaceView } from './useSpace'

type IndexedNote = { id: string; body: string }

const query = ref('')

/**
 * The applied document's notes, flattened to the two fields matching reads.
 *
 * A `shallowRef` for the reason `useSelection`'s orders are: `rebuild` only ever
 * replaces it wholesale, and a deep ref would proxy every entry to observe a
 * mutation that never happens — on a list this is re-walked for on every
 * keystroke.
 */
const notes = shallowRef<IndexedNote[]>([])

/**
 * Replaces the whole index rather than patching it.
 *
 * It is O(n) over tens of kilobytes, and it removes a class of index-drift bugs
 * outright: there is no add/remove/update path that can disagree with the
 * document.
 */
function rebuild(space: SpaceView | null) {
	notes.value = space ? space.notes.map((note) => ({ id: note.id, body: note.body })) : []
}

/** The trimmed query, since a query of only spaces matches everything and is
 *  indistinguishable from an empty field to the user. */
const activeQuery = computed(() => query.value.trim())

/**
 * The query as one folded character sequence — what both the scorer and the
 * highlighter match against.
 *
 * Whitespace is stripped rather than split on, which is the whole difference
 * between task-006's search and this one: `http req` is seven characters to find
 * in order, not two words to find separately. Published so `NoteBody` paints the
 * same sequence this module ranked by.
 */
const matchNeedle = computed(() => fuzzyNeedle(activeQuery.value))

/**
 * `null` when no query is active — which is *not* the same as an empty map, and
 * every consumer branches on the difference: `null` means "do not filter",
 * an empty map means "filter, and nothing survived".
 *
 * A `Map` rather than task-006's `Set`, and the value is the load-bearing part:
 * membership alone cannot rank, and `useSelection` orders each section's
 * survivors by the score it reads back from here. `has` still works, so the
 * consumers that only ask about membership are unchanged.
 */
const matchedIds = computed<Map<string, number> | null>(() => {
	const needle = matchNeedle.value
	if (needle.length === 0) return null

	const scores = new Map<string, number>()
	for (const note of notes.value) {
		const match = fuzzyMatch(note.body, needle)
		if (match) scores.set(note.id, match.score)
	}
	return scores
})

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
		matchNeedle,
		resultCount,
		rebuild,
		clearQuery,
	}
}
