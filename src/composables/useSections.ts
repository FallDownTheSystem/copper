/**
 * Section view state: whether the switcher is open, what it is filtered by, and
 * which sections are collapsed.
 *
 * **One-directional, in the same way `useSelection` and `useNoteSearch` are.**
 * It imports no adapter, `invoke`s nothing and never assigns `space`; the
 * switcher's actions live in the components that render it, exactly as
 * `PanelMenu`'s section-creation form calls `useSpace()` itself. That is what
 * lets `useSpace` reset this on an epoch change and `useSelection` filter its
 * traversal orders by it without a module cycle — which is the shape the plan's
 * "actions that delegate to `useSpace()`" would have produced.
 *
 * Module scope, not per-caller: refs declared inside the exported function hand
 * every caller a private copy, and the disclosure control, the grid's traversal
 * orders and the document coordinator all have to be looking at the same set.
 *
 * **Collapse is view state only** (task-010 §Design). It lives in memory for the
 * session, resets on restart, and nothing about it reaches the `.copper`
 * document or `settings.json` — persisting it would need schema versioning,
 * since task-003 §Q9 strips unknown keys on write.
 */

import { useNoteSearch } from './useNoteSearch'
import type { SpaceView } from './useSpace'

/** The `Ctrl+K` switcher anchored on the composer's chip. The `...` menu's
 *  `Switch section ▸` submenu is reka's own open state — a submenu of a menu
 *  that is already up — and deliberately not this. */
const switcherOpen = ref(false)
const filterQuery = ref('')

/** Section ids the user has collapsed. Not a `Set` mutated in place: a
 *  membership change has to be a new object for the computeds reading it to
 *  re-evaluate. */
const collapsed = ref(new Set<string>())

const { hasQuery } = useNoteSearch()

/**
 * Whether a section's notes are hidden **right now**.
 *
 * An active query overrides the stored state rather than clearing it, which is
 * what makes "the collapsed state is restored when the search is cleared" fall
 * out with no save-and-restore step to get wrong. Task-006's search already
 * hides sections with no match; this composes with it instead of fighting it,
 * because a query means nothing is collapsed at all.
 */
function isCollapsed(sectionId: string) {
	return !hasQuery.value && collapsed.value.has(sectionId)
}

/** The stored state, ignoring any active query. For the disclosure control's own
 *  label, which must describe what the press will do once the query clears. */
function isCollapsedStored(sectionId: string) {
	return collapsed.value.has(sectionId)
}

function toggleCollapsed(sectionId: string) {
	const next = new Set(collapsed.value)
	if (!next.delete(sectionId)) next.add(sectionId)
	collapsed.value = next
}

function setCollapsed(sectionId: string, value: boolean) {
	if (collapsed.value.has(sectionId) === value) return
	toggleCollapsed(sectionId)
}

/**
 * Expands every section that just received a note it did not have before.
 *
 * A capture landing in a collapsed section would otherwise be invisible, which
 * is the one outcome a tool whose whole promise is "capture is silent on
 * success" cannot afford. Driven off a diff of the applied document rather than
 * off one writer's return value, so it covers the composer, the global capture,
 * an `$EDITOR` write-back, an external edit and a redo with one rule.
 *
 * Called *before* the document is assigned, so the rows exist on the same flush
 * the sticky-bottom pin measures — expanding a tick later would leave the pin
 * anchored to a height that is about to change.
 *
 * Free in the ordinary case: with nothing collapsed there is nothing to reveal
 * and the diff never runs.
 */
function revealNewNotes(previous: SpaceView | null, next: SpaceView) {
	if (collapsed.value.size === 0 || !previous) return

	const known = new Set(previous.notes.map((note) => note.id))
	let revealed: Set<string> | null = null
	for (const note of next.notes) {
		if (known.has(note.id) || !collapsed.value.has(note.section)) continue
		revealed ??= new Set(collapsed.value)
		revealed.delete(note.section)
	}
	// Only a real change replaces the set; every applied document runs this.
	if (revealed) collapsed.value = revealed
}

function openSwitcher() {
	filterQuery.value = ''
	switcherOpen.value = true
}

function closeSwitcher() {
	switcherOpen.value = false
	filterQuery.value = ''
}

/**
 * Space identity changed: ids address a different document now.
 *
 * The switcher is **closed** rather than re-pointed at the new document's
 * sections — a menu that silently swaps out every row under the pointer is a
 * worse answer than one that goes away.
 */
function reset() {
	collapsed.value = new Set()
	closeSwitcher()
}

export function useSections() {
	return {
		switcherOpen: readonly(switcherOpen),
		filterQuery,
		isCollapsed,
		isCollapsedStored,
		toggleCollapsed,
		setCollapsed,
		revealNewNotes,
		openSwitcher,
		closeSwitcher,
		reset,
	}
}
