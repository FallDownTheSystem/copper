<script setup lang="ts">
import autoAnimate, { type AnimationController } from '@formkit/auto-animate'
import { useDragAndDrop } from '@formkit/drag-and-drop/vue'

import { EASE_OUT_QUINT_CSS } from '@/lib/motion'
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
	dragEnd: [noteId: string]
}>()

const { noteById, listAnimated } = useSpace()
const { isCollapsed } = useSections()

const dragging = ref(false)

/** Read here rather than passed as a prop, like selection and focus: a prop
 *  would put it in the parent's render dependencies and rebuild every section on
 *  every toggle. The rows themselves are already gone by the time this is true —
 *  `useSelection` filters them out of the one walk the list is rendered from —
 *  so this only decides what stands in their place. */
const collapsed = computed(() => isCollapsed(props.section.id))

/**
 * One drag parent per section, which is why sections are a component at all:
 * `useDragAndDrop` binds a parent element to a values array once, and a `v-for`
 * of sections inside one component has no stable place to do that.
 *
 * Cross-section drags work through the shared `group`, so a note can be dragged
 * out of one section and into another in a single gesture.
 */
const [rowgroup, order] = useDragAndDrop<string>([...props.noteIds], {
	group: 'copper-notes',
	// Synthetic pointer drag rather than the HTML5 drag API. Tauri's
	// `dragDropEnabled` (on by default) intercepts native drag events at the
	// webview boundary on Windows, and reordering built on `dragstart`/`drop`
	// would simply never fire. This setting sidesteps the question entirely —
	// which is also the answer to the gate task-011 carries.
	nativeDrag: false,
	// A handle rather than the whole card: the row already owns click-to-select
	// and a `contextmenu` trigger, and a body-wide drag would have to arbitrate
	// with both on `pointerdown`.
	dragHandle: '[data-drag-handle]',
	// The rowgroup also holds the section header row and, when empty, a message
	// row. ARIA forbids an inner wrapper here — a `rowgroup` may own only `row` —
	// so the non-note children are excluded by predicate instead.
	draggable: (child) => child.hasAttribute('data-note-row'),
	onDragstart: () => onDragStart(),
	onDragend: (data) => onDragEnd(String(data.draggedNode.data.value)),
})

function onDragStart() {
	dragging.value = true
}

function onDragEnd(noteId: string) {
	dragging.value = false
	emit('dragEnd', noteId)
}

/**
 * The store is the source of truth; the library's array is a mirror it is
 * allowed to reorder optimistically. When the write round-trips, the two already
 * agree and this assigns nothing.
 */
watch(
	() => props.noteIds,
	(ids) => {
		if (ids.length === order.value.length && ids.every((id, at) => order.value[at] === id)) return
		order.value = [...ids]
	},
	{ immediate: true },
)

/** Resolved once per row rather than twice — the `v-if` and the `:note` binding
 *  asked the same question — which drops the non-null assertion with it. An id
 *  the document no longer has simply yields no row. */
const orderedNotes = computed(() =>
	order.value.flatMap((id) => {
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
	// A drag already animates the rows it moves. Leaving auto-animate on would put
	// two independent transforms on the same element for the whole gesture.
	if (listAnimated.value && !dragging.value && !reduced.value) controller.enable()
	else controller.disable()
}

watch([listAnimated, dragging, reduced], syncAnimation)

onMounted(() => {
	const element = rowgroup.value
	if (!element) return
	// The library default of 250ms ease-in-out is too slow for the app's hottest
	// path. auto-animate consults `prefers-reduced-motion` itself but knows
	// nothing of Copper's own "Animate controls" setting, and it drives the Web
	// Animations API — so main.css's root gate cannot reach it either. `reduced`
	// above is the half neither of them covers.
	controller = autoAnimate(element, { duration: 150, easing: EASE_OUT_QUINT_CSS })
	syncAnimation()
})
</script>

<template>
	<!-- A function ref rather than the ref object: `useDragAndDrop` hands back a
	     `Ref<HTMLElement | undefined>`, which is not one of the shapes Vue's `ref`
	     binding accepts. -->
	<div
		:ref="(element) => (rowgroup = element as HTMLElement | undefined)"
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
		<div v-if="order.length === 0 && active && !collapsed" role="row">
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

.section-group > :deep([role='row'] + [role='row']) {
	margin-top: 4px;
}

.section-group > :deep([role='row']:first-child + [role='row']) {
	margin-top: 8px;
}
</style>
