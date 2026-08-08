<script setup lang="ts">
import autoAnimate, { type AnimationController } from '@formkit/auto-animate'

import { listMotion } from '@/lib/listMotion'
import { noteRow, sectionRow } from '@/composables/useSelection'
import type { Section } from '@/composables/useSpace'

const props = defineProps<{
	section: Section
	/** The notes of this section, already filtered by the active search. */
	noteIds: string[]
	active: boolean
	interactionRowId: string | null
}>()

const emit = defineEmits<{
	activate: []
	pointerSelect: [event: MouseEvent, noteId: string]
	toggleDone: [noteId: string]
}>()

const { noteById, listAnimated } = useSpace()
const { isCollapsed } = useSections()
const { isDragging } = useNoteDrag()

const rowgroup = useTemplateRef<HTMLElement>('rowgroup')

/** Read here rather than passed as a prop, like selection and focus: a prop
 *  would put it in the parent's render dependencies and rebuild every section on
 *  every toggle. The rows themselves are already gone by the time this is true —
 *  `useSelection` filters them out of the one walk the list is rendered from —
 *  so this only decides what stands in their place. */
const collapsed = computed(() => isCollapsed(props.section.id))

/**
 * The document's order is the only order there is.
 *
 * Reordering used to keep a second one: `@formkit/drag-and-drop` owned a mirror
 * array it reordered optimistically mid-gesture, and a watcher pushed the store's
 * order back into it on every applied document. `useNoteDrag` reorders nothing
 * while a drag runs — it translates the row it carries and commits a section and
 * an index — so there is no mirror left to keep in step, and what a section
 * renders is what the document says it holds.
 *
 * Resolved once per row rather than twice — the `v-if` and the `:note` binding
 * asked the same question — which drops the non-null assertion with it. An id the
 * document no longer has simply yields no row.
 */
const orderedNotes = computed(() =>
	props.noteIds.flatMap((id) => {
		const note = noteById(id)
		return note ? [note] : []
	}),
)

// --- list animation ----------------------------------------------------------
// The imperative controller rather than `v-auto-animate`: the directive gives no
// handle to disable animation, and rows mid-transform report transformed offsets
// — which invalidates the pixel offset a scroll restore is anchored on and makes
// an external reload visibly thrash.

let controller: AnimationController | null = null

const reduced = useReducedMotion()

function syncAnimation() {
	if (!controller) return
	// The dragged row carries a transform of its own for the length of the
	// gesture, and auto-animate would put a second, independent one on the same
	// element. Every section stands down rather than only the one the note came
	// from: a drag can cross into another section, and the row lands there.
	if (listAnimated.value && !isDragging.value && !reduced.value) controller.enable()
	else controller.disable()
}

watch([listAnimated, isDragging, reduced], syncAnimation)

onMounted(() => {
	const element = rowgroup.value
	if (!element) return
	// A plugin rather than an options object, because the options only reach the
	// FLIP: `listMotion` records what the library hard-codes for the other two
	// actions and why an arrival has to be authored to arrive at all.
	//
	// auto-animate consults `prefers-reduced-motion` itself but knows nothing of
	// Copper's own "Animate controls" setting, and it drives the Web Animations
	// API — so main.css's root gate cannot reach it either. `reduced` above is the
	// half neither of them covers.
	controller = autoAnimate(element, listMotion)
	syncAnimation()
})
</script>

<template>
	<!-- `data-section-id` is read by two things: this is the element a drop
	     resolves a section from, as well as the rowgroup auto-animate is bound
	     to. -->
	<div
		ref="rowgroup"
		role="rowgroup"
		:data-section-id="section.id"
		:aria-labelledby="`section-heading-${section.id}`"
		class="section-group min-w-0"
	>
		<!-- Neither row's selection or focus arrives as a prop: reading them here
		     would put them in this component's render dependencies, and every arrow
		     keypress would rebuild all 200 rows to change two of them. -->
		<SectionHeader
			:section="section"
			:active="active"
			:row-id="sectionRow(section.id)"
			@activate="emit('activate')"
		/>

		<NoteCard
			v-for="note in orderedNotes"
			:key="note.id"
			:note="note"
			:row-id="noteRow(note.id)"
			:interactive="interactionRowId === noteRow(note.id)"
			@pointer-select="emit('pointerSelect', $event, note.id)"
			@toggle-done="emit('toggleDone', note.id)"
		/>

		<!-- Only the *active* empty section says so, and never while it is merely
		     collapsed: the notes are there, they are folded away. The general empty
		     state is additive; the headers stay visible either way, because hiding
		     where a capture will land is worst exactly when the list is empty. -->
		<div v-if="noteIds.length === 0 && active && !collapsed" role="row">
			<div role="gridcell" class="text-text-secondary px-3 py-1 text-meta">
				No notes in this section yet.
			</div>
		</div>
	</div>
</template>

<style scoped>
.section-group + .section-group {
	/* At least 2x the within-group gap, so sections read as separate groups. */
	margin-top: 24px;
}

/**
 * Where a scroll landing has to stop, now that the heading pins itself across
 * the top of the region.
 *
 * Both numbers are landing margins rather than spacing: nothing moves, and they
 * are read only by `scrollIntoView` — the reveal of a captured note, and the
 * `block: 'nearest'` every arrow keypress performs. Without them a row scrolled
 * to the top edge lands *under* the pinned heading, which is the one place the
 * feature could hide the thing it was asked to show.
 *
 * The row's own margin is the band's full depth — the heading's height plus its
 * 8px of vertical padding — plus the 4px the rows already sit apart by, so a
 * landing clears the band by the same gap as the row above it. The
 * group's is 4px flat, and that 4px is the focus ring's halo: the group is where
 * a *pinned* heading is scrolled to (see `scrollRowIntoView`), and landing its
 * top flush against the region would clip the outer ring of a heading that
 * arrived there by keyboard.
 */
.section-group {
	scroll-margin-top: 4px;
}

.section-group > :deep([data-note-row]) {
	scroll-margin-top: calc(var(--section-heading-height) + 12px);
}

.section-group > :deep([role='row'] + [role='row']) {
	margin-top: 4px;
}

.section-group > :deep([role='row']:first-child + [role='row']) {
	margin-top: 8px;
}
</style>
