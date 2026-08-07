<script setup lang="ts">
import { formatCreated } from '@/lib/noteTime'
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
const { stopHandoff, doubleClickNote } = useNoteActions()
const { hasQuery } = useNoteSearch()
const { setMessage } = useStatusMessage()
const { beginDrag, consumeDragClick, draggingNoteId } = useNoteDrag()
const { isSorted, doneOnly } = useNoteList()
const { showCreated } = useSettings()

/** The field is omitted from the document when empty, so it arrives undefined
 *  on every note written before this feature existed. */
const attachments = computed(() => props.note.attachments ?? [])

const selected = computed(() => isSelected(props.note.id))
const focused = computed(() => focusedId.value === props.rowId)
const editing = computed(() => isEditing(props.note.id))
/** No handle, no drag: a searched or done-filtered list is a subset of each
 *  section and a sorted one is a permutation of it, so in none of those cases is
 *  an index read off the rendered rows the index `reorder_note` takes.
 *  `useNoteActions.reorderBlocked` refuses all three again for the keyboard path,
 *  and carries the reasoning. */
const draggable = computed(
	() => !editing.value && !hasQuery.value && !doneOnly.value && !isSorted.value,
)

/**
 * The creation date, when the setting asks for it and the stored value is
 * readable.
 *
 * `null` covers both halves of AC20: a `created` that cannot be parsed renders
 * **nothing** rather than a placeholder. A dash would claim the note has no date
 * when what is true is that the one it has cannot be read, and inventing a
 * plausible one would be worse still.
 */
const createdLabel = computed(() => (showCreated.value ? formatCreated(props.note.created) : null))
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

/**
 * A completed drag ends with the pointer going down and up on the grip, which is
 * a `click` by every definition the browser has — and the grip sits inside the
 * row, whose own click selects. Swallowed only when a drag actually happened, so
 * a plain click on the grip still selects the row it belongs to.
 */
function onGripClick(event: MouseEvent) {
	if (consumeDragClick()) event.stopPropagation()
}

/**
 * Task-013's double-click action, which is a setting: copy or edit.
 *
 * **The body, not the row's controls.** Everything with its own meaning is
 * excluded by target rather than by each control stopping propagation: the
 * completion box and the `Stop` button already `@click.stop`, but `dblclick` is
 * a separate event they say nothing about. `button` covers the attachment card
 * too — it is one, deliberately, and opening the file *is* its double-click. A
 * link is excluded because following it is what a double-click there means.
 *
 * **The grip is excluded twice over.** A completed drag ends with a `pointerup`
 * on the grip that the browser counts as a click, and `useNoteDrag` arms
 * `dragClickPending` for exactly that — but the flag is one-shot and belongs to
 * the click handler above, so consuming it here would swallow the click that
 * selects. Declining by target, and declining while a drag is still live, leaves
 * that mechanism alone.
 */
function onDoubleClick(event: MouseEvent) {
	if (draggingNoteId.value !== null) return
	const target = event.target as HTMLElement | null
	if (target?.closest('button, a[href], input, textarea, [data-drag-handle]')) return
	doubleClickNote()
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
			<!-- **One ring at a time: the focus ring is withheld from a selected row.**
			     The two used to stack — the selection ring is `ring-inset` and painted
			     the band from 0 to 2px inside the edge, and the focus ring was pushed
			     out to `-outline-offset-4` so it took the band from 2 to 4px and sat
			     beside it rather than over it. Two concentric rings 2px apart is what
			     a selected row looked like the moment it was focused, which on a
			     `rounded-lg` row reads as a doubled border rather than as two states.

			     It also arrived without warning: `:focus-visible` does not match a row
			     focused by the click that selected it, and then the *next* key — Shift
			     on its own is enough, since the browser re-evaluates the heuristic on
			     any keypress — made a second outline appear around a row nothing had
			     happened to.

			     What is lost is knowing *which* selected row holds focus, and it is
			     affordable here: plain arrows move focus and selection together, and
			     the case where they separate — Ctrl+Arrow — leaves the row unselected,
			     which is exactly when the focus ring is drawn. Back at
			     `-outline-offset-2` now that it has the edge to itself, matching the
			     section header row above it. -->
			<div
				role="row"
				:data-row-id="rowId"
				data-note-row
				:aria-selected="selected"
				:tabindex="focused ? 0 : -1"
				class="note-row group/row rounded-lg"
				:class="[
					selected ? 'row-selected ring-accent-ring ring-2 ring-inset' : 'focus-ring',
					'hover:bg-surface-hover transition-colors duration-fast',
				]"
				@click="emit('pointerSelect', $event)"
				@dblclick="onDoubleClick"
				@contextmenu="onContextMenu"
			>
				<!-- A grid rather than a flex row, for `content-center` alone. The row
				     track is the height of its tallest item, so centring the *track*
				     inside `min-h-11` centres a one-line note in the row while leaving a
				     note already taller than the minimum exactly where it was — and
				     `items-start` still puts the completion circle on the first line of
				     both. Flex has no equivalent: `items-center` would drag the circle to
				     the vertical middle of a tall note.

				     The third column is the grip's, and it is a column rather than an
				     overlay because that is what keeps a long line from running
				     underneath it: the gutter is reserved by the layout at all times, so
				     nothing depends on a `pr-*` matching the grip's width by hand. It
				     holds its width while the grip is invisible, so revealing the grip on
				     hover shifts no text either.

				     **`1rem`, which is the completion box's width, and that is the whole
				     point.** The two outer columns are now the same 16px with the same
				     `gap-2` beside them, so the text column is centred in the row and the
				     grip's inset from the right edge is the box's inset from the left.
				     At `1.25rem` the column was 4px wider than the mark it holds, and
				     `justify-center` split the difference — the grip sat 2px further in
				     than the box opposite it, and every line of every note was 4px
				     off-centre. -->
				<div
					class="grid min-h-11 min-w-0 grid-cols-[auto_minmax(0,1fr)_1rem] content-center items-start gap-2 px-3 py-2"
					role="gridcell"
				>
					<!-- A rounded square rather than task-004's circle, so the squircle
					     corner has something to shape — `corner-shape` does nothing to a
					     circle. The entrance-animation objection task-004 recorded here
					     still holds and is still satisfied: the mark is force-mounted and
					     never enters, so a Space repeat retargets a `pathLength` that is
					     already on screen rather than replaying an element appearing.
					     See `useLinecap` for why an empty box paints nothing at all. -->
					<Checkbox
						:tabindex="descendantTabIndex"
						:model-value="note.done"
						:aria-label="note.done ? 'Mark as not done' : 'Mark as done'"
						class="completion-box mt-0.5"
						@click.stop
						@update:model-value="emit('toggleDone')"
					/>

					<div class="min-w-0">
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

						<!-- **Below the body, and last in the column.** The date describes the
						     note as a whole, so it reads as the card's footer; putting it
						     between the body and its own attachments would separate content
						     that belongs together. Above the first line was the other option
						     the spec allowed and costs more: it pushes every note's first
						     line down by a row, in a list that has to stay legible at 200
						     notes.

						     One `text-meta` line at `mt-1` is the same vertical cost the
						     handoff notice above already pays, and it reuses that line's
						     colour rather than `--text-disabled`: this is information, not a
						     decorative mark, and 12px at the disabled tone is the one
						     combination too faint to read. Hidden while the editor is open,
						     like the attachment list and for the same reason — the editor
						     replaces the body, and adjuncts left under it read as part of
						     what is being edited.

						     `<time>` carries the machine-readable instant, which is the
						     stored RFC3339 string rather than the formatted local text. -->
						<p v-if="!editing && createdLabel" class="text-text-secondary mt-1 text-meta">
							<time :datetime="note.created">{{ createdLabel }}</time>
						</p>
					</div>

					<!-- A grip rather than a body-wide drag: the row already owns
					     click-to-select and the context-menu trigger, and a whole-card drag
					     would have to arbitrate with both on `pointerdown`. Kept out of the
					     tab order like every other descendant — the keyboard path to
					     reordering is Alt+Arrow, not this.

					     The visible mark stays small; `.note-grip` below is what the
					     pointer can actually hit. `touch-none` is what stops a touch or pen
					     drag from scrolling the region instead of moving the note. -->
					<span
						v-if="draggable"
						data-drag-handle
						role="presentation"
						class="note-grip text-text-disabled hover:text-text-secondary flex cursor-grab justify-center pt-1 opacity-0 transition-opacity duration-fast group-focus-within/row:opacity-100 group-hover/row:opacity-100"
						@pointerdown="beginDrag(note.id, $event)"
						@click="onGripClick"
					>
						<IconLucideGripVertical class="size-4" aria-hidden="true" focusable="false" />
					</span>
				</div>
			</div>
		</ContextMenuTrigger>

		<NoteContextMenu />
	</ContextMenu>
</template>

<style scoped>
/* Bounds layout cost without `content-visibility: auto`, which reports
   provisional dimensions for a skipped subtree and would make the disclosure
   measurement decide "not overflowing" until the card scrolled into view.

   It is also what makes the row the containing block for the grip's hit-area
   pseudo-element below — layout containment establishes one for absolutely
   positioned descendants. */
.note-row {
	contain: layout;
}

/* The grip's real hit area, which is much larger than the 16px mark it paints.
   Absolutely positioned and so resolved against `.note-row` rather than against
   the grid cell: that is what lets it span the row's *whole* height, padding
   included, without the layout knowing about it.

   It is bounded by the row on purpose, and cannot use `hit-44`: that utility
   centres a fixed 44px box on the control, which for a grip sitting near the top
   of a 44px row would reach up into the row above and make the two rows' grips
   fight over the same pixels. This one is exactly as tall as its own row and no
   taller.

   The width is exactly the gutter it lives in — the row's 12px right padding,
   plus the 16px grip column, plus the 8px column gap — so a generous grab area
   still never covers a word of the note. It follows the column: at 20px this was
   2.5rem, and leaving it there after narrowing the column would put 4px of the
   strip over the text.

   **Fine pointers only, and `touch-action` with it.** Both halves of this exist
   to help someone aiming a cursor at a 16px mark. On a touchscreen they do the
   opposite: a full-height strip down the right edge of every row that refuses to
   pan would make the list unscrollable exactly where a thumb naturally rests. A
   coarse pointer gets the plain 20px mark and keeps its scrolling; reordering by
   keyboard is Alt+Arrow either way. */
@media (pointer: fine) {
	.note-grip {
		touch-action: none;
	}

	.note-grip::before {
		content: '';
		position: absolute;
		top: 0;
		right: 0;
		bottom: 0;
		width: 2.25rem;
	}
}

/* Lifted while it is carried. The row is translated under the pointer with
   nothing else moving, so it needs a surface of its own — over a bare row the two
   texts would simply overlap — and a stacking order above the rows it passes.
   Nothing here animates over time: the transform tracks the pointer 1:1, so there
   is no duration for reduced motion to have an opinion about. */
.note-row[data-dragging] {
	position: relative;
	z-index: 10;
	background: var(--surface);
	box-shadow: 0 4px 14px oklch(0 0 0 / 0.18);
	cursor: grabbing;
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
