<script setup lang="ts">
import { CHORDS } from '@/lib/chords'
import { focusableIn } from '@/composables/useInteractionMode'
import { rowElement, rowSectionId } from '@/composables/useSelection'

const { space, sections, activeSection, noteById, setActiveSection, setNotesDone } = useSpace()
const {
	focusedId,
	focusedNoteId,
	select,
	toggle,
	extendTo,
	extendFocus,
	selectAll,
	moveFocus,
	focusFirst,
	focusLast,
	visibleGroups,
} = useSelection()
const { beginEdit } = useNoteEditor()
const { interactionRowId, enter, reconcile } = useInteractionMode()
const { hasQuery, resultCount } = useNoteSearch()
const { setCollapsed, collapseEnabled } = useSections()
const { doneOnly } = useNoteList()
const { toggleDone } = useNoteActions()
const { dropTarget } = useNoteDrag()

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
 * The done filter left nothing on screen.
 *
 * A separate condition from `noMatches` rather than a wider version of it,
 * because the two can disagree: a query can match three notes of which none are
 * done, and `resultCount` — which counts matches in the document, not survivors
 * of the filter — is 3. `noMatches` takes precedence when both hold, since the
 * query is the narrower explanation and clearing it is the shorter way back.
 */
const noDone = computed(
	() => doneOnly.value && !noMatches.value && renderedSections.value.length === 0,
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
		// No `behavior: 'smooth'` — this fires on every arrow keypress.
		element.scrollIntoView({ block: 'nearest' })
	})
}

function activateSection(id: string) {
	void setActiveSection(id)
}

/** A click on the completion circle names one card unambiguously, so it toggles
 *  that card rather than the selection. The selection-aware form is `Space`. */
function toggleOne(noteId: string) {
	const note = noteById(noteId)
	if (note) void setNotesDone([noteId], !note.done)
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
			event.preventDefault()
			if (event.shiftKey) extendFocus(1)
			else moveFocus(1)
			syncDomFocus()
			return
		case 'ArrowUp':
			event.preventDefault()
			if (event.shiftKey) extendFocus(-1)
			else moveFocus(-1)
			syncDomFocus()
			return
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
				// Only an unmodified Space activates a section. Ctrl+Space is the
				// Windows IME chord, and swallowing it here would take the candidate
				// window away from anyone typing Japanese.
				event.preventDefault()
				activateSection(sectionId)
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
		<DoneEmptyState v-else-if="noDone" />
	</div>
</template>
