<script setup lang="ts">
import { focusRowSoon, takeRow } from '@/composables/useSelection'
import type { Section } from '@/composables/useSpace'
import { isComposing } from '@/lib/chords'

const props = defineProps<{
	section: Section
	active: boolean
	rowId: string
}>()

const emit = defineEmits<{ activate: [] }>()

const { focusedId } = useSelection()
const { renaming, draft, setDraft, endRename, cancelRename } = useSectionEditor()
const { renameSection } = useSpace()
const { isCollapsedStored, toggleCollapsed, collapseEnabled } = useSections()

const focused = computed(() => focusedId.value === props.rowId)
const headingId = computed(() => `section-heading-${props.section.id}`)
const editing = computed(() => renaming.value === props.section.id)

/** The stored state, not the effective one: while a query is active every
 *  section is expanded, but the control still has to say what pressing it does
 *  to the state the query is overriding. */
const collapsed = computed(() => isCollapsedStored(props.section.id))

/**
 * Moves the roving target onto this row as well as toggling.
 *
 * A click focuses the button it landed on, and the grid's key handler declines
 * any press whose target is a button — so without this, expanding a section by
 * mouse left the arrow keys inert until the user clicked somewhere else. The row
 * is the grid's tab stop; the control inside it never is.
 */
function toggle() {
	toggleCollapsed(props.section.id)
	takeRow(props.rowId)
}

const input = useTemplateRef<HTMLInputElement>('input')

// The field replaces the heading in place, so it has to take focus itself —
// nothing else moves focus into a control that did not exist a tick ago.
watch(editing, (open) => {
	if (!open) return
	void nextTick(() => {
		input.value?.focus()
		input.value?.select()
	})
})

/**
 * Enter and blur both land here. The session is ended *before* the write, so the
 * blur the unmounting field fires finds nothing open and returns — which is what
 * makes committing with Enter safe without a re-entry flag.
 */
async function commit() {
	if (!editing.value) return
	const write = endRename(props.section.name)
	// The row is the grid's tab stop; leaving focus on a field that is
	// unmounting drops it to the body and makes the list unreachable.
	focusRowSoon(props.rowId)
	if (write) await renameSection(write.id, write.name)
}

function onKeydown(event: KeyboardEvent) {
	// Escape is withheld from the shell's ladder rather than merely ignored: it
	// closes the candidate window, and letting the press continue up would take a
	// rung of the ladder while the user is still composing.
	if (isComposing(event)) {
		if (event.key === 'Escape') event.stopPropagation()
		return
	}

	if (event.key === 'Enter') {
		event.preventDefault()
		event.stopPropagation()
		void commit()
	} else if (event.key === 'Escape') {
		event.preventDefault()
		event.stopPropagation()
		cancelRename()
		focusRowSoon(props.rowId)
	}
}
</script>

<template>
	<!-- A `grid` may own only `row` and `rowgroup`, and a `rowgroup` only `row`,
	     so the section header is itself a row rather than an <h2> sitting between
	     rowgroups. It pays for itself: the header becomes keyboard-reachable
	     through ordinary arrow navigation instead of needing a bespoke path.
	     Header rows carry no aria-selected — they are not selectable.

	     The context menu attached here is the *section* menu. A note menu must
	     not open on a header row, which is why the trigger lives on note rows
	     only and this one carries its own content. -->
	<ContextMenu>
		<ContextMenuTrigger as-child>
			<!-- The radius rounds the focus ring and the pinned band both. A square
			     ring around a row sitting among `rounded-lg` note rows is the one shape
			     it should not be — and a capsule is the other. The row is
			     `--section-heading-height` tall, so `--radius-md` at 14px sits past half
			     its height; the small-control tier's 10px is the most it can take and
			     still ring a rectangle.

			     **The row pins itself to the top of the region while its own section is
			     being read**, which is what keeps the answer to "which section am I in"
			     on screen through a long one. `position: sticky` rather than a second
			     rendered copy: the row that rides the top edge *is* this one, so the
			     roving `tabindex`, the context menu, the collapse control and the active
			     marker all keep working up there with no duplicate to keep in step. The
			     containing block is the section's own rowgroup, so a heading is pushed
			     back out by the end of its section instead of stacking with the next
			     one — and a collapsed or search-dropped section renders no rows at all,
			     which leaves nothing to pin and no rule to withdraw.

			     **`z-1` is measured against three things rather than picked as "on
			     top".** Above the rows it covers; below the carried row's `z-10`
			     (NoteCard), so a note being dragged passes over the heading rather than
			     behind it; and below the drop indicator's `z-20` (NoteList), so the line
			     saying where that note would land is never what the heading hides. The
			     status band and the portal host sit above again at `z-20`/`z-30` in this
			     same stacking context — the panel's `isolate` root — and both live at
			     the far end of the list.

			     The band is the panel's own surface token, not a new material: the row
			     has to erase whatever scrolls under it, and `--surface` composited over
			     the panel it is already sitting on leaves 1% of that row showing. Which
			     is also why nothing looks different until something is under it. -->
			<div
				role="row"
				:data-row-id="rowId"
				data-section-row
				:tabindex="focused ? 0 : -1"
				class="focus-ring bg-surface sticky top-0 z-1 min-w-0 rounded-compact"
			>
				<div
					role="gridcell"
					class="flex min-h-(--section-heading-height) min-w-0 items-center gap-2 px-3"
				>
					<template v-if="editing">
						<label :for="`section-rename-${section.id}`" class="sr-only">Section name</label>
						<input
							:id="`section-rename-${section.id}`"
							ref="input"
							:value="draft"
							type="text"
							autocomplete="off"
							class="panel-field h-6 min-w-0 flex-1 px-1.5 text-label uppercase"
							@input="setDraft(($event.target as HTMLInputElement).value)"
							@keydown="onKeydown"
							@blur="commit"
							@contextmenu.stop
						/>
					</template>

					<template v-else>
						<!-- The name leads, so the heading starts at the row's own left edge
						     rather than behind a control, and the chevron is pushed to the
						     far end by the separator rule between them: the two things a
						     section row can be grabbed by sit at its two extremes, with the
						     whole width of the row as target in between. It still rotates to
						     point down while the section is open.

						     The name shrinks rather than holding its width — with the
						     chevron at the end of the row, an unshrinkable one would push it
						     out. The inner `truncate` is what makes that safe. -->
						<!-- **`pl-3` is an alignment, not a spacing choice.** A note row is
						     `px-3` plus a 16px completion box plus `gap-2`, so its text starts
						     36px in; this row is `px-3` plus the marker's 6px dot plus
						     `gap-1.5`, which landed the section name 6px to the left of every
						     note under it. The extra 12px of button padding closes exactly
						     that gap, so the heading and the notes it heads share one left
						     edge. Anything that changes the note row's leading columns has to
						     come back here. -->
						<h2 :id="headingId" class="min-w-0">
							<button
								type="button"
								tabindex="-1"
								:aria-current="active ? 'true' : undefined"
								class="hover:bg-surface-hover active:bg-surface-active flex min-w-0 items-center gap-1.5 rounded-inset py-1 pr-1.5 pl-3 transition-colors duration-fast"
								:class="active ? 'text-accent-text' : 'text-text-secondary'"
								@click="emit('activate')"
							>
								<!-- The only one of the three markers that cross-fades: this row is on
								     screen while it changes, unlike the two inside menus. -->
								<ActiveMarker
									:active="active"
									label="active section"
									class="transition-opacity duration-fast"
								>
									<span
										class="truncate text-label uppercase"
										:class="active ? 'font-semibold' : ''"
									>
										{{ section.name }}
									</span>
								</ActiveMarker>
							</button>
						</h2>

						<span aria-hidden="true" class="bg-separator h-px min-w-0 flex-1" />

						<!-- Withdrawn while a query is active rather than rendered inert.
						     Search already decides what is on screen and overrides collapse
						     entirely, so a control that rotated its chevron and changed
						     nothing visible would read as broken. The stored state survives
						     and comes back when the query clears; the fixed-width stand-in
						     keeps the separator from growing and shrinking as a query is
						     typed.

						     `rounded-inset` because the box is `size-5`: at 12px a 20px
						     square is a circle, and a round hover surface says radio button
						     rather than disclosure. -->
						<button
							v-if="collapseEnabled"
							type="button"
							tabindex="-1"
							:aria-expanded="!collapsed"
							:aria-label="`${collapsed ? 'Expand' : 'Collapse'} ${section.name}`"
							class="text-text-disabled hover:bg-surface-hover hover:text-text-secondary focus-ring grid size-5 shrink-0 place-items-center rounded-inset transition-colors duration-fast"
							@click="toggle"
						>
							<IconLucideChevronRight
								class="size-3.5 transition-transform duration-fast"
								:class="collapsed ? '' : 'rotate-90'"
								aria-hidden="true"
								focusable="false"
							/>
						</button>
						<span v-else aria-hidden="true" class="size-5 shrink-0" />
					</template>
				</div>
			</div>
		</ContextMenuTrigger>

		<SectionContextMenu :section="section" />
	</ContextMenu>
</template>
