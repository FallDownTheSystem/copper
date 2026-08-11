<script setup lang="ts">
import { CHORDS } from '@/lib/chords'
import { focusableIn } from '@/composables/useInteractionMode'
import {
	flushReveal,
	rowElement,
	rowSectionId,
	scrollRowIntoView,
} from '@/composables/useSelection'

const { space, sections, activeSection, noteCount, noteById, notesInSection, setActiveSection } =
	useSpace()
const {
	focusedId,
	focusedNoteId,
	select,
	selectedIds,
	toggle,
	extendTo,
	extendFocus,
	selectAll,
	focusRow,
	moveFocus,
	moveFocusOnly,
	focusFirst,
	focusLast,
	rowIds,
	visibleGroups,
} = useSelection()
const { beginEdit } = useNoteEditor()
const { beginConfirm } = useSectionDelete()
const { interactionRowId, enter, reconcile } = useInteractionMode()
const { hasQuery, resultCount } = useNoteSearch()
const { setCollapsed, toggleCollapsed, collapseEnabled } = useSections()
const { filtersByDone } = useNoteList()
const { toggleDone, toggleNoteDone } = useNoteActions()
const { dropTarget, isDragging } = useNoteDrag()
const { setPanelVisible } = usePreviews()
const { setMessage } = useStatusMessage()

/**
 * The moments a reveal that could not land becomes possible again.
 *
 * `useSelection.revealRow` holds the request rather than performing it, because
 * the panel is usually hidden when a capture arrives and the list may not be in
 * the DOM at all — the settings view replaces it. Each of these is a transition
 * from "there was nothing to scroll" to "there is":
 *
 * - **Mount.** The list coming back from the settings view, or being rendered for
 *   the first time after a capture reached a panel that had never been opened.
 * - **The region gaining a height.** The window is shown and the list is laid out
 *   for the first time. **This is the case the whole feature is for, and it is the
 *   only trigger here that observes the same thing the request is waiting on** —
 *   `flushReveal` gives up on a region whose `clientHeight` is 0, and a resize is
 *   how that number stops being 0.
 * - **Becoming visible.** Kept, but not relied on: showing the panel does not
 *   unmount this tree, so `visibilitychange` is only as good as the webview's
 *   tracking of a parent window it does not own. WebView2 promises nothing here.
 * - **A drag ending.** The reveal stands aside while a row is being carried, since
 *   the drag's own auto-scroll owns the region until the drop.
 *
 * Flushing when there is nothing pending is free — the first line of `flushReveal`
 * returns.
 */
const scrollRegion = shallowRef<HTMLElement | null>(null)
onMounted(() => {
	flushReveal()
	// Resolved here rather than in `setup`: this list renders *inside* the region,
	// so on the first pass the region is not in the document yet.
	scrollRegion.value = document.querySelector<HTMLElement>('[data-scroll-region]')
	syncPreviewVisibility()
})
// Every box change, with no height test of its own: `flushReveal` already holds
// the request when the region has no height, and a second copy of that condition
// is one that can drift from it. VueUse owns the lifecycle and the
// unsupported-environment guard, as it does for the clamp probe.
useResizeObserver(scrollRegion, () => {
	flushReveal()
	syncPreviewVisibility()
})
useEventListener(document, 'visibilitychange', () => {
	if (document.visibilityState === 'visible') flushReveal()
	syncPreviewVisibility()
})

watch(isDragging, (dragging) => {
	if (!dragging) void nextTick(() => flushReveal())
})

/**
 * Whether the panel is on screen, answered for `usePreviews` off the same two
 * signals the reveal above stands on — and in that order of trust.
 *
 * A link preview is an outbound request to a stranger's server, so it may not be
 * issued while nobody is looking at the panel; the window is mounted hidden at
 * launch, which without this made a cold start fetch every link in the space. The
 * region's height is the load-bearing half: it goes from 0 to a real number when
 * the window is laid out for the first time, and it is the same transition
 * `flushReveal` waits on. `visibilitychange` is kept for the other direction —
 * the panel going away again — while being worth exactly what WebView2's tracking
 * of a parent window it does not own is worth, which is why it is not the only
 * signal here.
 */
function syncPreviewVisibility() {
	const laidOut = (scrollRegion.value?.clientHeight ?? 0) > 0
	setPanelVisible(laidOut && document.visibilityState !== 'hidden')
}

/**
 * The rendered list, paired with its section objects in one place. Derived from
 * `visibleGroups` rather than from `sections`, so what is on screen and what the
 * arrow keys traverse come out of the same filtered walk and cannot disagree.
 */
const renderedSections = computed(() => {
	const bySection = new Map(sections.value.map((section) => [section.id, section]))
	return visibleGroups.value.flatMap((group) => {
		const section = bySection.get(group.sectionId)
		return section ? [{ section, noteIds: group.noteIds }] : []
	})
})

/** A query that matched nothing. A zero count with no query is simply an empty
 *  space, which is task-004's own presentation and not this state. */
const noMatches = computed(() => hasQuery.value && resultCount.value === 0)

/**
 * The done filter left nothing on screen — in either of the two states that can.
 *
 * A separate condition from `noMatches` rather than a wider version of it,
 * because the two can disagree: a query can match three notes of which none are
 * done, and `resultCount` — which counts matches in the document, not survivors
 * of the filter — is 3. `noMatches` takes precedence when both hold, since the
 * query is the narrower explanation and clearing it is the shorter way back.
 *
 * **`noteCount` is the third condition and it is not belt-and-braces.** A space
 * with no notes at all satisfies the other two the moment the default view is one
 * that filters, and `PanelShell` already answers that case with "No notes yet" —
 * so without this the empty space would be explained twice, once correctly and
 * once by a filter that is not the reason.
 */
const filteredEmpty = computed(
	() =>
		filtersByDone.value &&
		!noMatches.value &&
		noteCount.value > 0 &&
		renderedSections.value.length === 0,
)

/** The roving target has to actually hold DOM focus, or arrow navigation moves
 *  a highlight the screen reader never follows. */
function syncDomFocus() {
	void nextTick(() => {
		const key = focusedId.value
		if (!key) return
		const element = rowElement(key)
		if (!element) return
		element.focus()
		// No `behavior: 'smooth'` — this fires on every arrow keypress. Shared with
		// the reveal path rather than calling `scrollIntoView` here: a pinned section
		// heading is the one row for which "nearest" is already true and still wrong,
		// and both paths have to answer that the same way.
		scrollRowIntoView(element, 'nearest')
	})
}

/**
 * Activation unfolds a folded section (user ruling, 2026-08-10): choosing a
 * section as the capture target implies wanting to see it, so the "open it"
 * half lives on Enter and the name click — the deliberate gestures — while
 * Space stays a pure fold toggle. Guarded so an activation during a search
 * does not silently rewrite the stored fold state the query is overriding.
 */
function activateSection(id: string) {
	if (collapseEnabled.value) setCollapsed(id, false)
	void setActiveSection(id)
}

/** A click on the completion circle names one card unambiguously, so it toggles
 *  that card rather than the selection. The selection-aware form is `Space`. */
function toggleOne(noteId: string) {
	void toggleNoteDone(noteId)
}

function startEditing(noteId: string) {
	const current = space.value
	const note = noteById(noteId)
	if (current && note) beginEdit(current, note)
}

function onPointerSelect(event: MouseEvent, noteId: string) {
	if (event.shiftKey) extendTo(noteId)
	else if (event.ctrlKey || event.metaKey) toggle(noteId)
	else select(noteId)
}

/**
 * Keeps the roving target pointed at wherever DOM focus actually lands.
 *
 * Two paths put focus on a row without going through the key handlers: a Tab
 * that *enters* the grid from outside, and a click anywhere on a band, which
 * focuses it by its own `tabindex`. Both used to leave `focusedId` stale, so
 * the next arrow moved relative to a row the user had visibly left.
 * Idempotent for the handlers' own moves — `takeRow` focuses the row it just
 * recorded, and re-recording the same key is a no-op write.
 *
 * Deliberately quiet about the selection: Ctrl+Arrow's `syncDomFocus` and a
 * click both arrive here, and neither may write it. A Tab *within* the grid
 * never reaches this handler with work to do — the keydown below moves focus
 * itself, exactly as the arrows do.
 *
 * **Outside F2 interaction mode, focus never rests on a row's controls.** A
 * click focuses the button it lands on, and the browser re-evaluates
 * `:focus-visible` on the next keypress — so clicking the completion circle
 * and then pressing Space drew a keyboard focus ring on a control the user
 * never keyboard-navigated to (user report, 2026-08-10). Handing focus back
 * to the row closes that for every control in one place. Text surfaces are
 * exempt — the rename field and the inline editor hold focus by design — and
 * so is the row F2 promoted, whose descendants holding focus is the mode.
 */
function onFocusin(event: FocusEvent) {
	const target = event.target as HTMLElement | null
	const row = target?.closest<HTMLElement>('[data-row-id]')
	const key = row?.dataset.rowId
	if (!row || !key) return
	if (key !== focusedId.value) focusRow(key)
	if (target !== row && interactionRowId.value !== key && !target?.matches('input, textarea')) {
		row.focus()
	}
}

function onKeydown(event: KeyboardEvent) {
	// A reka overlay that consumed the key must not also move the selection.
	if (event.defaultPrevented) return

	/**
	 * **Alt belongs to the shell's chord layer, and the grid must not claim it.**
	 *
	 * This is what made Alt+Arrow reordering appear to work exactly once, measured
	 * live in WebView2. The grid is a descendant of the shell, so it sees a press
	 * first; `case 'ArrowDown'` tested no modifier, so it `preventDefault`ed
	 * Alt+ArrowDown and moved the roving target instead — and the shell's handler,
	 * whose first line declines an already-prevented press, never ran. The one
	 * position it worked from was a control *inside* a row: the guard below
	 * early-returns for a button target, so the press escaped to the shell and
	 * reordered. That reorder then put focus back on the row itself, where this
	 * handler could swallow every press after it. The bug re-armed itself.
	 *
	 * A guard rather than an `altKey` test on each case, because the grid binds no
	 * Alt chord at all — the general rule is the honest one.
	 */
	if (event.altKey) return

	const target = event.target as HTMLElement | null

	// While in interaction mode the grid's own bindings do not fire; Tab and
	// Shift+Tab cycle within the cell instead. `Escape` is a rung of the shell's
	// ladder and is deliberately not handled here.
	if (interactionRowId.value) {
		if (event.key !== 'Tab') return
		const row = rowElement(interactionRowId.value)
		if (!row) return
		const focusable = focusableIn(row)
		if (focusable.length === 0) return

		event.preventDefault()
		const index = focusable.indexOf(document.activeElement as HTMLElement)
		const step = event.shiftKey ? -1 : 1
		const next = (index + step + focusable.length) % focusable.length
		focusable[next]?.focus()
		return
	}

	// Ctrl+A must keep its native select-all-text behaviour in every text surface
	// — including inside a code fence, which is selectable text — and Space on the
	// completion circle must not also toggle the row.
	if (target?.closest('input, textarea, a[href], button, pre[tabindex]')) return

	const noteId = focusedNoteId.value
	const sectionId = rowSectionId(focusedId.value)

	switch (event.key) {
		case 'ArrowDown':
		case 'ArrowUp': {
			// **Ctrl+Arrow moves the roving target and leaves the selection alone**,
			// which is what makes `Ctrl+Space` usable more than once: travelling to
			// the next note in order to toggle it would otherwise have replaced the
			// selection on the way there, and the discontiguous case below is the
			// whole reason that chord exists.
			//
			// Shift is tested first and so wins a Ctrl+Shift+Arrow. Extending is the
			// louder intent, and "move quietly" has nothing to add to a press that is
			// already growing a range.
			event.preventDefault()
			const delta = event.key === 'ArrowDown' ? 1 : -1
			if (event.shiftKey) extendFocus(delta)
			else if (event.ctrlKey || event.metaKey) moveFocusOnly(delta)
			else moveFocus(delta)
			syncDomFocus()
			return
		}
		case 'ArrowLeft':
		case 'ArrowRight':
			// The disclosure idiom, and both keys are otherwise unbound here: the grid
			// has one cell per row, so nothing horizontal traverses. Header rows only —
			// on a note row these belong to whatever has the caret.
			//
			// Inert while a query is active, matching the control itself: search
			// overrides collapse, so a press would change a state nothing is reading.
			if (!sectionId || !collapseEnabled.value) return
			event.preventDefault()
			setCollapsed(sectionId, event.key === 'ArrowLeft')
			return
		case 'Tab': {
			// Handled like an arrow, not left to the browser: inside the grid the
			// sequential move is fully determined — every row is a stop and no
			// control is — so the landing takes `landOn`'s rule directly, ring and
			// focus moving as one. (A flag read back in `onFocusin` was tried and
			// cannot work: the microtask checkpoint runs when the keydown listener
			// returns, *before* the browser's own focus move, so the flag was dead
			// by the time the arrival was visible — measured live, 2026-08-11, as
			// a selection ring left behind on the row Tab departed.)
			//
			// The browser keeps the two moves that are genuinely its own: a press
			// at either end of the list, which is how Tab leaves the grid — the
			// selection deliberately survives that, as it survives a click outside
			// the list — and any Ctrl/Meta chord, which was never a row hop.
			if (event.ctrlKey || event.metaKey) return
			const delta = event.shiftKey ? -1 : 1
			const rows = rowIds.value
			const index = focusedId.value ? rows.indexOf(focusedId.value) : -1
			const next = index + delta
			if (index === -1 || next < 0 || next >= rows.length) return
			event.preventDefault()
			moveFocus(delta)
			syncDomFocus()
			return
		}
		case 'Home':
			event.preventDefault()
			focusFirst()
			syncDomFocus()
			return
		case 'End':
			event.preventDefault()
			focusLast()
			syncDomFocus()
			return
		case 'Delete':
			// The section half of Delete only. Notes belong to the shell's chord
			// layer, and so does a focused header with a live selection: the target
			// rule reads that header as "take the selection" — it is where
			// `selectSection` deliberately parks focus — so the press must keep
			// deleting those notes. A bare header claims the key instead: it was a
			// silent no-op, and it is one keypress from a section and everything in
			// it, which is why this asks where the menu item does not.
			if (event.ctrlKey || event.metaKey || event.shiftKey) return
			if (!sectionId || selectedIds.value.length > 0) return
			event.preventDefault()
			if (sections.value.length < 2) {
				// The store refuses to delete the last section, exactly as the menu
				// item disables — but a keypress deserves an answer, not a shrug.
				setMessage('The last section cannot be deleted.')
				return
			}
			beginConfirm(
				sectionId,
				notesInSection(sectionId).map((note) => note.id),
			)
			return
		case 'F2':
			event.preventDefault()
			enter(focusedId.value)
			return
		case ' ':
			// Ctrl+Space and Shift+Space are the only keyboard path to a
			// discontiguous selection, since plain Space is taken by mark-as-done.
			if (noteId) {
				event.preventDefault()
				if (event.ctrlKey || event.metaKey) toggle(noteId)
				else if (event.shiftKey) extendTo(noteId)
				// Selection-aware, and one undoable operation whatever the count.
				else void toggleDone()
			} else if (sectionId && !event.ctrlKey && !event.metaKey && !event.shiftKey) {
				// Only an unmodified Space, because Ctrl+Space is the Windows IME
				// chord, and swallowing it here would take the candidate window away
				// from anyone typing Japanese. Space *toggles the disclosure* — the
				// user's ruling (2026-08-10); making the section active stays on
				// Enter. preventDefault runs even while search disables collapse,
				// or the press would scroll the region instead.
				event.preventDefault()
				if (collapseEnabled.value) toggleCollapsed(sectionId)
			}
			return
		case 'Enter':
			// Ctrl+Enter starts the editor handoff and belongs to the shell's chord
			// layer; only a bare Enter opens the inline editor.
			if (!CHORDS.edit.matches(event)) return
			event.preventDefault()
			if (noteId) startEditing(noteId)
			else if (sectionId) activateSection(sectionId)
			return
		case 'a':
		case 'A':
			if (!event.ctrlKey && !event.metaKey) return
			event.preventDefault()
			selectAll()
			return
		default:
	}
}

// A document change does not by itself invalidate interaction mode — toggling
// `done` with Space is a document change, and dropping the user out of the mode
// they just used a key in would be its own bug.
watch(() => space.value, reconcile)
</script>

<template>
	<!-- `data-note-list` marks the frame every drag coordinate is measured
	     against, and `relative` is what lets the drop indicator be placed in it.
	     The root scrolls with the content, so an offset measured against it stays
	     true while the region scrolls under the pointer. -->
	<div data-note-list class="relative min-w-0">
		<!-- One grid spanning every section, not one per section: a Shift range has
		     to extend across section boundaries, which needs a single composite
		     widget. During a search a section with no match is not rendered at all,
		     header row included — which is what keeps `ArrowDown` off rows that are
		     no longer on screen.

		     Absent entirely when nothing matches, rather than rendered empty: a
		     `grid` with no `row` or `rowgroup` child fails `aria-required-children`,
		     and the empty state is not a row. -->
		<div
			v-if="renderedSections.length > 0"
			role="grid"
			aria-multiselectable="true"
			aria-label="Notes"
			class="min-w-0"
			@keydown="onKeydown"
			@focusin="onFocusin"
		>
			<NoteSection
				v-for="entry in renderedSections"
				:key="entry.section.id"
				:section="entry.section"
				:note-ids="entry.noteIds"
				:active="entry.section.id === activeSection"
				:interaction-row-id="interactionRowId"
				@activate="activateSection(entry.section.id)"
				@pointer-select="onPointerSelect"
				@toggle-done="toggleOne"
			/>
		</div>

		<!-- Where the dragged note would land. Outside the grid rather than inside
		     it: a `grid` may own only `row` and `rowgroup`, so a bare div among the
		     sections would break `aria-required-children`. Purely decorative — the
		     drag has no keyboard equivalent to announce, Alt+Arrow being the
		     keyboard path to the same outcome. -->
		<div
			v-if="dropTarget"
			aria-hidden="true"
			class="bg-accent-ring pointer-events-none absolute inset-x-1 z-20 h-0.5 rounded-full"
			:style="{ top: `${dropTarget.indicatorY}px` }"
		/>

		<SearchEmptyState v-if="noMatches" />
		<DoneEmptyState v-else-if="filteredEmpty" />
	</div>
</template>
