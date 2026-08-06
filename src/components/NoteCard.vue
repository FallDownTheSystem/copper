<script setup lang="ts">
import type { Note } from '@/composables/useSpace'

const props = defineProps<{
	note: Note
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
// Subscribed to here rather than passed down. As props they sat in NoteList's
// render dependencies, so a keypress that moved focus by one row rebuilt every
// row in the list. They are still the wrapping row's state either way.
const { focusedId, isSelected, select } = useSelection()
const { isHandingOff, isConflicted } = useEditorHandoff()
const { stopHandoff } = useNoteActions()
const { hasQuery } = useNoteSearch()
const { setMessage } = useStatusMessage()

/** The field is omitted from the document when empty, so it arrives undefined
 *  on every note written before this feature existed. */
const attachments = computed(() => props.note.attachments ?? [])

const selected = computed(() => isSelected(props.note.id))
const focused = computed(() => focusedId.value === props.rowId)
const editing = computed(() => isEditing(props.note.id))
/** No handle, no drag: a filtered list is a subset of its section, so an index
 *  read off it is not the index `reorder_note` takes. */
const draggable = computed(() => !editing.value && !hasQuery.value)
const descendantTabIndex = computed(() => (props.interactive ? 0 : -1))
const handingOff = computed(() => isHandingOff(props.note.id))
const conflicted = computed(() => isConflicted(props.note.id))

/**
 * Right-clicking outside the current selection replaces it with this card;
 * right-clicking inside it leaves the selection untouched — which is what makes
 * every menu item's target resolution correct without the menu having to know
 * how it was opened.
 *
 * Runs before reka's own trigger handler, which defers its work to a `nextTick`.
 */
function onContextMenu() {
	if (!selected.value) select(props.note.id)
}
</script>

<template>
	<!-- `ContextMenu` renders no element of its own, and the trigger merges onto
	     the row through `as-child`: a `grid` may own only `row` and `rowgroup` and
	     a `rowgroup` only `row`, so a wrapper here would break
	     `aria-required-children`. Note rows only — a section header row opens no
	     note menu. -->
	<ContextMenu>
		<ContextMenuTrigger as-child>
			<div
				role="row"
				:data-row-id="rowId"
				data-note-row
				:aria-selected="selected"
				:tabindex="focused ? 0 : -1"
				class="note-row group/row outline-focus-ring rounded-md focus-visible:outline-2 focus-visible:-outline-offset-2"
				:class="[
					selected ? 'row-selected ring-accent-ring ring-2 ring-inset' : '',
					'hover:bg-surface-hover transition-colors duration-fast',
				]"
				@click="emit('pointerSelect', $event)"
				@contextmenu="onContextMenu"
			>
				<!-- A grid rather than a flex row, for `content-center` alone. The row
				     track is the height of its tallest item, so centring the *track*
				     inside `min-h-11` centres a one-line note in the row while leaving a
				     note already taller than the minimum exactly where it was — and
				     `items-start` still puts the completion circle on the first line of
				     both. Flex has no equivalent: `items-center` would drag the circle to
				     the vertical middle of a tall note. -->
				<div
					class="grid min-h-11 min-w-0 grid-cols-[auto_minmax(0,1fr)] content-center items-start gap-2 px-3 py-2"
					role="gridcell"
				>
					<button
						type="button"
						:tabindex="descendantTabIndex"
						:aria-pressed="note.done"
						:aria-label="note.done ? 'Mark as not done' : 'Mark as done'"
						class="completion-circle border-text-disabled outline-focus-ring relative mt-0.5 grid size-4 shrink-0 place-items-center rounded-full border transition-colors duration-base focus-visible:outline-2 focus-visible:outline-offset-1"
						:class="note.done ? 'bg-accent-ring border-accent-ring text-white' : ''"
						@click.stop="emit('toggleDone')"
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

					<div class="min-w-0">
						<!-- A grip rather than a body-wide drag: the row already owns
						     click-to-select and the context-menu trigger, and a whole-card
						     drag would have to arbitrate with both on `pointerdown`. Kept
						     out of the tab order like every other descendant — the keyboard
						     path to reordering is Alt+Arrow, not this. -->
						<span
							v-if="draggable"
							data-drag-handle
							role="presentation"
							class="text-text-disabled hover:text-text-secondary absolute top-2 right-1 cursor-grab rounded-md p-1 opacity-0 transition-opacity duration-fast group-focus-within/row:opacity-100 group-hover/row:opacity-100 active:cursor-grabbing"
						>
							<IconLucideGripVertical class="size-4" aria-hidden="true" focusable="false" />
						</span>
						<NoteEditor v-if="editing" :row-id="rowId" />
						<NoteBody v-else :note="note" :class="note.done ? 'note-done' : ''" />

						<!-- Below the Markdown body, inside the same `min-w-0` chain — a
						     filename is a long unbreakable token and would otherwise widen
						     the document, which the panel must never scroll horizontally.
						     Hidden while the inline editor is open: the editor replaces the
						     body, and leaving the cards under it would imply they are part
						     of what is being edited. -->
						<ul
							v-if="!editing && attachments.length > 0"
							class="mt-1.5 flex min-w-0 flex-col gap-1"
						>
							<li v-for="attachment in attachments" :key="attachment.id" class="min-w-0">
								<AttachmentCard
									:attachment="attachment"
									:tab-index="descendantTabIndex"
									@message="setMessage"
								/>
							</li>
						</ul>

						<!-- Icon plus text, never colour alone. The control that ends the
						     handoff sits next to it because a handoff the user cannot end is
						     a note they cannot edit in the panel. -->
						<p
							v-if="handingOff"
							class="text-text-secondary mt-1 flex flex-wrap items-center gap-1.5 text-meta"
						>
							<IconLucideSquarePen class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
							<span>{{
								conflicted ? 'Editing externally — save refused' : 'Editing externally'
							}}</span>
							<button
								type="button"
								:tabindex="descendantTabIndex"
								class="panel-button min-h-6"
								@click.stop="stopHandoff(note.id)"
							>
								Stop
							</button>
						</p>
					</div>
				</div>
			</div>
		</ContextMenuTrigger>

		<NoteContextMenu />
	</ContextMenu>
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
