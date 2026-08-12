<script setup lang="ts">
import { formatCreated, formatRelative } from '@/lib/noteTime'
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
const { isSelected, select } = useSelection()
const { isHandingOff, isConflicted } = useEditorHandoff()
const { stopHandoff, doubleClickNote } = useNoteActions()
const { hasQuery } = useNoteSearch()
const { setMessage } = useStatusMessage()
const { beginDrag, consumeDragClick, draggingNoteId } = useNoteDrag()
const { isSorted } = useNoteList()
const { showCreated } = useSettings()
const { now } = useRelativeTime()

/** The field is omitted from the document when empty, so it arrives undefined
 *  on every note written before this feature existed. */
const attachments = computed(() => props.note.attachments ?? [])

const selected = computed(() => isSelected(props.note.id))
const editing = computed(() => isEditing(props.note.id))

/**
 * True while a drag is carrying the selection this row belongs to — every
 * selected row except the one under the pointer. A drag whose grabbed note is
 * selected commits the whole selection (`useNoteActions.movedIds`), and the
 * rows that will travel have to say so *during* the gesture, not surprise on
 * the drop. Cheap to be reactive, unlike the pointer position the drag module
 * keeps out of refs: it changes at drag start and end, never per move.
 */
const carried = computed(
	() =>
		draggingNoteId.value !== null &&
		draggingNoteId.value !== props.note.id &&
		selected.value &&
		isSelected(draggingNoteId.value),
)
/** No handle, no drag: a searched list is ranked and a sorted one is computed,
 *  so under either the rendered order is a permutation of the document and a
 *  drop between two rows names a position the document does not have. The done
 *  filter deliberately keeps the grip (user ruling 2026-08-12): it narrows the
 *  rows but never reorders them, and the drop anchors to its visible
 *  neighbours — `useNoteActions.documentIndex` carries the math, and
 *  `reorderBlocked` refuses the other two again for the keyboard path. */
const draggable = computed(() => !editing.value && !hasQuery.value && !isSorted.value)

/**
 * The creation date, when the setting asks for it and the stored value is
 * readable.
 *
 * `null` covers both halves of AC20: a `created` that cannot be parsed renders
 * **nothing** rather than a placeholder. A dash would claim the note has no date
 * when what is true is that the one it has cannot be read, and inventing a
 * plausible one would be worse still.
 *
 * **The line is relative and the title is absolute**, which is the split rather
 * than a fallback: "3h ago" is what the reader of a capture list actually wants —
 * how fresh is this — and it is a strictly worse label for the one question it
 * cannot answer, which day. The exact instant is a hover away on the same
 * element, and `datetime` carries it to a machine either way.
 */
const createdLabel = computed(() =>
	showCreated.value ? formatRelative(props.note.created, now.value) : null,
)
const createdTitle = computed(() => (showCreated.value ? formatCreated(props.note.created) : null))
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
	<div
		role="row"
		:data-row-id="rowId"
		data-note-row
		:data-carried="carried ? '' : undefined"
		:aria-selected="selected"
		tabindex="0"
		class="note-row group/row rounded-lg"
		:class="[
			selected
				? 'row-selected ring-accent-ring ring-2 ring-inset focus-visible:outline-hidden'
				: 'focus-inset',
			'hover:bg-surface-hover transition-colors duration-fast',
		]"
		@click="emit('pointerSelect', $event)"
		@dblclick="onDoubleClick"
		@contextmenu="onContextMenu"
	>
		<!-- **The row element is this component's root, and that is load-bearing.**
		     The card is a `<TransitionGroup>` child, and the group can hand its
		     enter/leave/move work only to a child that resolves to a single element
		     root. `ContextMenu` can never be that root: reka-ui's renderless chain
		     bottoms out in a `PopperRoot` whose render is a slot *fragment*, and Vue
		     refuses to carry transition hooks through a fragment ("renders
		     non-element root node that cannot be animated" — the 2026-08-11
		     dead-animation bug). So the menu lives inside the row, and its trigger
		     merges onto the gridcell through `as-child`. The cell is the row's whole
		     box, so the right-click surface is unchanged — and no wrapper element
		     appears anywhere, which `aria-required-children` requires: a `grid` may
		     own only `row` and `rowgroup`, a `rowgroup` only `row`. The menu content
		     teleports to the overlay host either way. Note rows only — a section
		     header row opens no note menu.

		     Every template comment sits *inside* the row for the same reason. A
		     comment beside the root survives into dev builds and turns the
		     component's subtree into a root fragment whose `el` is a text anchor
		     — and Vue 3.6's `TransitionGroup` skips any previous child whose `el`
		     is not an `Element` when it installs leave and move hooks. Enter has
		     no such check, so the symptom is rows that unfold in but vanish
		     without folding out, in dev only (production strips comments). -->
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
		     which is exactly when the focus ring is drawn.

		     **What is drawn there is `focus-inset` — the same crisp 2px accent
		     outline the section band wears.** This row wore a soft outer halo for
		     a long time, and the reasoning behind it is now known to be void: the
		     1px `focus-ring` edge "read almost white" on dark rows because a stuck
		     Chromium transition held every outline at `currentColor` (measured
		     2026-08-10 — see the `transition-colors` rule in main.css), not
		     because a crisp edge is wrong on a row. The halo itself was the next
		     complaint: at 50% alpha over the dark surface it read as a muddy brown
		     band (user screenshot, same day). One outline language everywhere —
		     rows, bands, checkbox — and the selection ring is its visual twin, so
		     "you are here" always looks the same.

		     **The selected branch still needs `focus-visible:outline-hidden`.**
		     Withholding `focus-inset` by swapping the class out does not withhold
		     the browser's own ring — an element that matches `:focus-visible`
		     with no author outline gets the default ring, which on the dark panel
		     is a crisp white outline. `outline-hidden` rather than `outline-none`
		     because in forced colors the transparent outline it declares is
		     forced visible and is the selected row's only indicator there.

		     **`tabindex="0"`, unconditionally.** Every row in the grid is a
		     sequential stop, notes included (user ruling, 2026-08-11, reversing
		     the sections-only order): Tab walks the list row by row, moved by
		     `NoteList`'s keydown exactly as an arrow is, so a landing selects
		     the note. The attribute is what lets a Tab at either end still
		     enter and leave the grid. The descendants stay at -1 — F2 is still
		     the way in. Arrows land here through `takeRow`; the grid's focusin
		     handler keeps the roving target pointed at whatever row focus
		     actually reaches. -->
		<ContextMenu>
			<ContextMenuTrigger as-child>
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
				     off-centre.

					     **16px each side rather than 12, and the number is not this row's
					     to choose.** (The right side gives up the scrollbar's overflow
					     share, `--row-inset-comp`, so the text column and the grip hold
					     still when the list starts to scroll — see main.css.) With the list's own `px-1` outside it the completion box
					     lands 20px from the panel edge, which is where the search field's
					     magnifier sits — the leading marks share that column. The section
					     heading's dot used to be brought onto it too and deliberately left
					     (user ruling, 2026-08-11): a heading outdented from its notes is
					     the visual hierarchy, so only the box and the magnifier align now.
					     The 8px this takes off the text column is what the alignment
					     costs; the grip's hit strip below follows it. -->
				<div
					class="grid min-h-11 min-w-0 grid-cols-[auto_minmax(0,1fr)_1rem] content-center items-start gap-2 py-2 pl-4 pr-[calc(--spacing(4)-var(--row-inset-comp,0px))]"
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
								conflicted ? 'Editing externally, save refused' : 'Editing externally'
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
							<time :datetime="note.created" :title="createdTitle ?? undefined">
								{{ createdLabel }}
							</time>
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
			</ContextMenuTrigger>

			<NoteContextMenu />
		</ContextMenu>
	</div>
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

   The width is exactly the gutter it lives in — the row's 16px right padding,
   plus the 16px grip column, plus the 8px column gap — so a generous grab area
   still never covers a word of the note. It tracks both of those numbers and has
   been wrong about each in turn: 2.5rem was right for a 20px grip column and 4px
   too wide once the column narrowed, and 2.25rem was right for a 12px padding and
   4px too short once the row moved to `px-4` for the leading-mark alignment. Both
   times the strip stopped agreeing with the gutter it is supposed to be.

   **Fine pointers only, and `touch-action` with it.** Both halves of this exist
   to help someone aiming a cursor at a 16px mark. On a touchscreen they do the
   opposite: a full-height strip down the right edge of every row that refuses to
   pan would make the list unscrollable exactly where a thumb naturally rests. A
   coarse pointer gets the plain 16px mark and keeps its scrolling; reordering by
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
		/* Less the scrollbar's share of the row's right padding, so the strip
		   keeps agreeing with the gutter it spans when the padding yields to the
		   scrollbar (see `--row-inset-comp` in main.css). */
		width: calc(2.5rem - var(--row-inset-comp, 0px));
	}
}

/* Lifted while it is carried, and still lifted on the way back. The row is
   translated with nothing else moving, so it needs a surface of its own — over a
   bare row the two texts would simply overlap — and a stacking order above the
   rows it passes. `data-settling` is `useNoteDrag`'s second attribute and exists
   only for this rule: an abandoned row animates home over 150ms, and dropping the
   surface at the start of that trip would send it back underneath its neighbours.

   `will-change` because this is the one element in the panel written to on every
   frame, and it carries a 14px blur while it moves — without the promotion the
   compositor re-rasterises that shadow against the rows underneath each time the
   transform changes. Scoped to the attributes, so the layer exists for the length
   of the gesture rather than standing permanently on 200 rows. */
.note-row[data-dragging],
.note-row[data-settling] {
	position: relative;
	z-index: 10;
	background: var(--surface);
	box-shadow: 0 4px 14px oklch(0 0 0 / 0.18);
	will-change: transform;
}

/* The carry only. A row returning home is not being held, and the pointer that
   was holding it has usually been released by then. */
.note-row[data-dragging] {
	cursor: grabbing;
}

/* The rest of a carried selection. Dimmed for the length of the gesture, so
   the block that will travel on the drop reads as in hand rather than left
   behind — the selection ring alone claimed nothing about the drag. */
.note-row[data-carried] {
	opacity: 0.45;
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
