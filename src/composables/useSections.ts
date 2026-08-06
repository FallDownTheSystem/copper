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

/** The two places the switcher can be showing from. */
export type SwitcherHost = 'chip' | 'menu'

/**
 * Which host currently has the switcher open, if either.
 *
 * One ref rather than a boolean per host, because both hosts have to run the
 * *same* lifecycle — the filter is cleared on every open and every close, and an
 * epoch change closes whichever is up. Two independent booleans is what left the
 * `...` submenu uncontrolled: its filter survived every dismissal, so reopening
 * it showed a pre-filtered list, and a stale no-match query showed only
 * `Create section "<old query>"` with Enter creating it.
 *
 * A single shared boolean would not do either: both hosts bind their `open` to
 * this, so one flag would open the chip's dropdown and the submenu at once.
 */
const openHost = ref<SwitcherHost | null>(null)
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
 * Brings the collapse set back into line with a document that has just been
 * applied. Two jobs, one walk, because both are "which ids still mean what they
 * meant".
 *
 * **Pruning.** A collapsed id whose section no longer exists is dead weight that
 * nothing can ever remove: it defeats the `size === 0` fast path below for the
 * rest of the session, and if the id is ever reintroduced — an undone delete
 * restores exactly the id it removed — the section comes back mysteriously
 * folded shut.
 *
 * **Revealing.** Every section that just received a note it did not have before
 * is expanded. A capture landing in a collapsed section would otherwise be
 * invisible, which is the one outcome a tool whose whole promise is "capture is
 * silent on success" cannot afford. Driven off a diff of the applied document
 * rather than off one writer's return value, so it covers the composer, the
 * global capture, an `$EDITOR` write-back, an external edit and a redo with one
 * rule. A note that merely *moved* into a collapsed section is not new and does
 * not reveal it — that destination was chosen.
 *
 * Called *before* the document is assigned, so revealed rows exist on the same
 * flush the sticky-bottom pin measures; expanding a tick later would leave the
 * pin anchored to a height that is about to change.
 *
 * Free in the ordinary case: with nothing collapsed there is nothing to do.
 */
function reconcile(previous: SpaceView | null, next: SpaceView) {
	if (collapsed.value.size === 0) return

	// Copied only once something actually changes; every applied document runs
	// this, and copying a set to discover nothing moved is the cost of every edit.
	let updated: Set<string> | null = null
	const edit = () => (updated ??= new Set(collapsed.value))

	const live = new Set(next.sections.map((section) => section.id))
	for (const id of collapsed.value) {
		if (!live.has(id)) edit().delete(id)
	}

	if (previous) {
		const known = new Set(previous.notes.map((note) => note.id))
		for (const note of next.notes) {
			if (known.has(note.id) || !collapsed.value.has(note.section)) continue
			edit().delete(note.section)
		}
	}

	if (updated) collapsed.value = updated
}

/** Whether the switcher is showing at all, for the shell and for tests. */
const switcherOpen = computed(() => openHost.value !== null)

/** Whether *this* host is the one showing it. Each host binds its own `open` to
 *  this, so opening one cannot open the other. */
function isSwitcherOpenIn(host: SwitcherHost) {
	return openHost.value === host
}

function openSwitcher(host: SwitcherHost = 'chip') {
	filterQuery.value = ''
	openHost.value = host
}

/** Closing is host-scoped so a stale event from the host that is *not* showing
 *  cannot dismiss the one that is. Called with no host, it closes whatever is
 *  open — which is what an epoch change wants. */
function closeSwitcher(host?: SwitcherHost) {
	if (host !== undefined && openHost.value !== host) return
	openHost.value = null
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
	// No host argument: whichever one is showing goes, which is what "closed, not
	// re-pointed" means when the document underneath it has been replaced.
	closeSwitcher()
}

export function useSections() {
	return {
		switcherOpen,
		isSwitcherOpenIn,
		filterQuery,
		isCollapsed,
		isCollapsedStored,
		toggleCollapsed,
		setCollapsed,
		reconcile,
		openSwitcher,
		closeSwitcher,
		reset,
	}
}
