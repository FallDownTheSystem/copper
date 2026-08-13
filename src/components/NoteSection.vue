<script setup lang="ts">
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

const { noteById, listAnimated, noteCount, countsInSection } = useSpace()
const { isCollapsed } = useSections()
const { isDragging } = useNoteDrag()
const { hasQuery } = useNoteSearch()

/** Read here rather than passed as a prop, like selection and focus: a prop
 *  would put it in the parent's render dependencies and rebuild every section on
 *  every toggle. The rows themselves are already gone by the time this is true —
 *  `useSelection` filters them out of the one walk the list is rendered from —
 *  so this only decides what stands in their place. */
const collapsed = computed(() => isCollapsed(props.section.id))

/** Empty in the *document*, not merely in the view. `noteIds` is the filtered
 *  rendering, and the done filter can hide every note a section holds — hidden
 *  notes are not absent, and the bare heading is that state's honest rendering.
 *  Only a section that truly holds nothing gets the placeholder below. */
const sectionEmpty = computed(() => countsInSection(props.section.id).total === 0)

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
// `useListTransition` records why `<TransitionGroup>` replaced auto-animate: the
// library animated from cached positions, and a section whose neighbours moved
// it carried a stale cache into its next gesture. The gate below is read per
// animation rather than watched into a controller, so there is no stand-down
// ordering to reason about.

const reduced = useReducedMotion()

// The dragged row carries a transform of its own for the length of the gesture,
// and a FLIP would put a second, independent one on the same element. Every
// section stands down rather than only the one the note came from: a drag can
// cross into another section, and the row lands there.
//
// `reduced` folds in Copper's own "Animate controls" setting, which the
// enter/leave hooks drive through the Web Animations API — the one channel
// main.css's `.reduce-motion` root gate cannot reach. The `.list-move` CSS
// transition *is* reachable, so reduced motion truncates moves twice over.
const { moveClass, onEnter, onLeave, onEnterCancelled, onLeaveCancelled } = useListTransition(
	() => listAnimated.value && !isDragging.value && !reduced.value,
)
</script>

<template>
	<!-- `data-section-id` is read by two things: this is the element a drop
	     resolves a section from, as well as the rowgroup whose children animate.

	     The rowgroup is a `<TransitionGroup>` so that every measurement is taken
	     fresh in the render pass immediately before the patch — a moved row FLIPs
	     from where it actually is, not from where a cache last saw it. `:css`
	     off because enter and leave are Web Animations owned by the hooks; the
	     move stays a CSS class, which is the only form Vue's FLIP takes. -->
	<TransitionGroup
		tag="div"
		role="rowgroup"
		:data-section-id="section.id"
		:aria-labelledby="`section-heading-${section.id}`"
		class="section-group min-w-0"
		:css="false"
		:move-class="moveClass"
		@enter="onEnter"
		@leave="onLeave"
		@enter-cancelled="onEnterCancelled"
		@leave-cancelled="onLeaveCancelled"
	>
		<!-- Neither row's selection or focus arrives as a prop: reading them here
		     would put them in this component's render dependencies, and every arrow
		     keypress would rebuild all 200 rows to change two of them. -->
		<SectionHeader
			key="heading"
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

		<!-- Every empty section says so, active or not (user ruling 2026-08-13) —
		     but never while it is merely collapsed: the notes are there, they are
		     folded away. And never for a section the done filter emptied:
		     `sectionEmpty` reads the document, so a filtered-away section keeps
		     its bare heading rather than a false "yet". The headers stay visible
		     either way, because hiding where a capture will land is worst exactly
		     when the list is empty.

		     The `noteCount`/`hasQuery` pair stands this line down while the shell's
		     EmptyState card is on screen — its condition, mirrored. On a fresh space
		     the card already names the destination section, and the same fact twice
		     at the single most important first-impression moment read as a stutter.
		     With notes elsewhere in the document the card never renders, and this
		     line is the only thing that says an empty section is empty. -->
		<div
			v-if="sectionEmpty && !collapsed && (noteCount > 0 || hasQuery)"
			key="empty"
			role="row"
		>
			<!-- `px-4` joins the leading-mark column: the completion box and the marker
			     dot land at 16px inside the region, and a line of text starting anywhere
			     else reads as a stray. -->
			<div role="gridcell" class="text-text-secondary px-4 py-1 text-meta">
				No notes in this section yet.
			</div>
		</div>
	</TransitionGroup>
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
 * landing clears the band by the same gap as the row above it. The group's is
 * 4px flat: the group is where a *pinned* heading is scrolled to (see
 * `scrollRowIntoView`), and the 4px keeps that landing a step short of flush.
 * (`focus-inset` draws inside the band's box, so nothing clips at flush any
 * more — the margin is breathing room now, not protection.)
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
