<script setup lang="ts">
import autoAnimate, { type AnimationController } from '@formkit/auto-animate'
import { noteRow, rowElement, rowSectionId, sectionRow } from '@/composables/useSelection'

const {
	space,
	sections,
	activeSection,
	notesInSection,
	setNotesDone,
	setActiveSection,
	listAnimated,
} = useSpace()
const {
	focusedId,
	focusedNoteId,
	isSelected,
	selectedIds,
	select,
	toggle,
	extendTo,
	extendFocus,
	selectAll,
	clear,
	moveFocus,
	focusFirst,
	focusLast,
} = useSelection()
const { editingNoteId, beginEdit, cancel } = useNoteEditor()

/**
 * Interaction mode. Without it `Show more` and in-body links have no keyboard
 * path at all, because everything inside a row is `tabindex="-1"` — which is
 * what makes the one-Tab-stop claim true in the first place.
 */
const interactionRowId = ref<string | null>(null)

function focusableIn(row: HTMLElement) {
	return [...row.querySelectorAll<HTMLElement>('button, a[href]')]
}

/**
 * Anchors inside rendered Markdown carry `tabindex="-1"` from a render rule, so
 * they have to be flipped here rather than through a prop — the HTML string is
 * not Vue's to patch.
 */
function setDescendantsTabbable(key: string, tabbable: boolean) {
	const row = rowElement(key)
	if (!row) return
	for (const element of focusableIn(row)) element.tabIndex = tabbable ? 0 : -1
}

function enterInteraction() {
	const key = focusedId.value
	if (!key) return
	interactionRowId.value = key
	void nextTick(() => {
		setDescendantsTabbable(key, true)
		const row = rowElement(key)
		focusableIn(row ?? document.createElement('div'))[0]?.focus()
	})
}

function exitInteraction() {
	const key = interactionRowId.value
	if (!key) return
	setDescendantsTabbable(key, false)
	interactionRowId.value = null
	void nextTick(() => rowElement(key)?.focus())
}

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

function toggleDone(noteId: string) {
	const note = space.value?.notes.find((candidate) => candidate.id === noteId)
	if (!note) return
	void setNotesDone([noteId], !note.done)
}

function startEditing(noteId: string) {
	const current = space.value
	const note = current?.notes.find((candidate) => candidate.id === noteId)
	if (current && note) beginEdit(current, note)
}

function onPointerSelect(event: MouseEvent, noteId: string) {
	if (event.shiftKey) extendTo(noteId)
	else if (event.ctrlKey || event.metaKey) toggle(noteId)
	else select(noteId)
}

function onKeydown(event: KeyboardEvent) {
	// A reka-ui overlay that consumed the key must not also clear the selection.
	if (event.defaultPrevented) return

	const target = event.target as HTMLElement | null

	if (event.key === 'Escape') {
		// The ladder, in order: open dropdown (already returned above) →
		// interaction mode → inline editor → selection.
		if (interactionRowId.value) {
			event.preventDefault()
			exitInteraction()
		} else if (editingNoteId.value) {
			event.preventDefault()
			cancel()
		} else if (selectedIds.value.length > 0) {
			event.preventDefault()
			clear()
		}
		return
	}

	// While in interaction mode the grid's own bindings do not fire; Tab and
	// Shift+Tab cycle within the cell instead.
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

	// Ctrl+A must keep its native select-all-text behaviour in every text
	// surface, and Space on the completion circle must not toggle the row twice.
	if (target?.closest('input, textarea, a[href], button')) return

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
			enterInteraction()
			return
		case ' ':
			event.preventDefault()
			// Ctrl+Space and Shift+Space are the only keyboard path to a
			// discontiguous selection, since plain Space is taken by mark-as-done.
			if (noteId && (event.ctrlKey || event.metaKey)) toggle(noteId)
			else if (noteId && event.shiftKey) extendTo(noteId)
			else if (noteId) toggleDone(noteId)
			else if (sectionId) activateSection(sectionId)
			return
		case 'Enter':
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

// --- list animation ----------------------------------------------------------
// The imperative controller rather than `v-auto-animate`: the directive gives no
// handle to disable animation, and rows mid-transform report transformed
// offsets — which invalidates the pixel offset a scroll restore is anchored on
// and makes an external reload visibly thrash.

// Keyed on the element, not the section id. Vue re-invokes an inline function
// ref on every re-render because its identity changes — first with null, then
// with the element — so keying on the id would tear down and re-register a
// second MutationObserver over the same rowgroup on every render.
const controllers = new Map<HTMLElement, AnimationController>()

function registerRowgroup(element: unknown) {
	if (!(element instanceof HTMLElement) || controllers.has(element)) return

	// The library default of 250ms ease-in-out is too slow for the app's hottest
	// path. Reduced motion is respected by auto-animate itself.
	const controller = autoAnimate(element, { duration: 150, easing: 'ease-out' })
	if (!listAnimated.value) controller.disable()
	controllers.set(element, controller)
}

watch(listAnimated, (enabled) => {
	for (const [element, controller] of controllers) {
		if (!element.isConnected) {
			controllers.delete(element)
			continue
		}
		if (enabled) controller.enable()
		else controller.disable()
	}
})

// A document swap invalidates any interaction mode: the row it belongs to may
// not exist any more, and its descendants' tabindex would be left flipped.
watch(
	() => space.value,
	() => {
		if (interactionRowId.value) exitInteraction()
	},
)
</script>

<template>
	<div
		role="grid"
		aria-multiselectable="true"
		aria-label="Notes"
		class="min-w-0"
		@keydown="onKeydown"
	>
		<!-- One grid spanning every section, not one per section: a Shift range has
		     to extend across section boundaries, which needs a single composite
		     widget. -->
		<div
			v-for="section in sections"
			:key="section.id"
			:ref="registerRowgroup"
			role="rowgroup"
			:aria-labelledby="`section-heading-${section.id}`"
			class="section-group min-w-0"
		>
			<SectionHeader
				:section="section"
				:active="section.id === activeSection"
				:focused="focusedId === sectionRow(section.id)"
				:row-id="sectionRow(section.id)"
				@activate="activateSection(section.id)"
			/>

			<NoteCard
				v-for="note in notesInSection(section.id)"
				:key="note.id"
				:note="note"
				:row-id="noteRow(note.id)"
				:selected="isSelected(note.id)"
				:focused="focusedId === noteRow(note.id)"
				:interactive="interactionRowId === noteRow(note.id)"
				@pointer-select="onPointerSelect($event, note.id)"
				@toggle-done="toggleDone(note.id)"
			/>

			<!-- Only the *active* empty section says so. The general empty state is
			     additive; the headers stay visible either way, because hiding where a
			     capture will land is worst exactly when the list is empty. -->
			<div
				v-if="notesInSection(section.id).length === 0 && section.id === activeSection"
				role="row"
			>
				<div role="gridcell" class="text-text-secondary px-3 py-1 text-meta">
					No notes in this section yet.
				</div>
			</div>
		</div>
	</div>
</template>

<style scoped>
.section-group + .section-group {
	/* At least 2x the within-group gap, so sections read as separate groups. */
	margin-top: 24px;
}

.section-group > :deep([role='row'] + [role='row']) {
	margin-top: 4px;
}

.section-group > :deep([role='row']:first-child + [role='row']) {
	margin-top: 8px;
}
</style>
