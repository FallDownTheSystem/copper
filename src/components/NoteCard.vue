<script setup lang="ts">
import type { Note } from '@/composables/useSpace'

const props = defineProps<{
	note: Note
	selected: boolean
	focused: boolean
	rowId: string
	/** Interaction mode: this row's descendants are tabbable and the grid's own
	 *  bindings do not fire. */
	interactive: boolean
}>()

const emit = defineEmits<{
	pointerSelect: [event: MouseEvent]
	toggleDone: []
}>()

const { isEditing } = useNoteEditor()

const editing = computed(() => isEditing(props.note.id))
const descendantTabIndex = computed(() => (props.interactive ? 0 : -1))
</script>

<template>
	<div
		role="row"
		:data-row-id="rowId"
		:aria-selected="selected"
		:tabindex="focused ? 0 : -1"
		class="note-row outline-focus-ring rounded-md focus-visible:outline-2 focus-visible:-outline-offset-2"
		:class="[
			selected ? 'row-selected ring-accent-ring ring-2 ring-inset' : '',
			'hover:bg-surface-hover transition-colors duration-fast',
		]"
		@click="emit('pointerSelect', $event)"
	>
		<div class="flex min-h-11 min-w-0 items-start gap-2 px-3 py-2" role="gridcell">
			<button
				type="button"
				:tabindex="descendantTabIndex"
				:aria-pressed="note.done"
				:aria-label="note.done ? 'Mark as not done' : 'Mark as done'"
				class="completion-circle border-text-disabled outline-focus-ring relative mt-0.5 grid size-4 shrink-0 place-items-center rounded-full border transition-colors duration-base focus-visible:outline-2 focus-visible:outline-offset-1"
				:class="note.done ? 'bg-accent-ring border-accent-ring text-white' : ''"
				@click.stop="emit('toggleDone')"
				@keydown.stop
			>
				<!-- No entrance animation on the glyph: the toggle is bound to Space,
				     repeats, and a scale-in on a keyboard-repeated control reads as
				     lag. Only background-color transitions. -->
				<svg
					v-if="note.done"
					viewBox="0 0 16 16"
					class="size-3"
					aria-hidden="true"
					focusable="false"
				>
					<path
						d="M3.5 8.5 6.5 11.5 12.5 5"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
					/>
				</svg>
			</button>

			<div class="min-w-0 flex-1">
				<NoteEditor v-if="editing" :row-id="rowId" />
				<NoteBody v-else :note="note" :class="note.done ? 'note-done' : ''" />
			</div>
		</div>
	</div>
</template>

<style scoped>
/* Bounds layout cost without `content-visibility: auto`, which reports
   provisional dimensions for a skipped subtree and would make the disclosure
   measurement decide "not overflowing" until the card scrolled into view. */
.note-row {
	contain: layout;
}

/* Done notes drop to the secondary colour with a faint rule through the text
   layer only, so code fences and tables are not struck through. */
.note-row :deep(.note-done) {
	color: var(--text-secondary);
	font-weight: 500;
}

.note-row :deep(.note-done .note-prose p),
.note-row :deep(.note-done .note-prose li) {
	text-decoration: line-through;
	text-decoration-color: color-mix(in oklab, currentColor 45%, transparent);
}
</style>
