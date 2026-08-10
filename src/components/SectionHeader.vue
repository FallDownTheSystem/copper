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
 * is the grid's tab stop; the control inside it never is. `activate` needs the
 * same move for the same reason: the name is also a button, and a click on it
 * left the arrows just as inert.
 */
function toggle() {
	toggleCollapsed(props.section.id)
	takeRow(props.rowId)
}

function activate() {
	emit('activate')
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
			<!-- **Square, and that is a property of the band rather than of the row.**
			     The corner used to be the small-control tier's 10px, rounding the focus
			     ring and the pinned band together. A band is what this actually is: it
			     spans the region, it rides its top edge, and a rounded rectangle riding
			     a straight edge reads as a card that has come loose rather than as a
			     heading that has stuck. The ring it also rounded is gone — `focus-halo`
			     draws no edge to round.

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

			     **The band spans the region edge to edge, which is what `-mx-1 px-1`
			     buys.** The margin reaches past the scroll region's own `px-1` — the 4px
			     rhythm the cards are inset by — and the padding puts the heading back
			     where it was, so only the fill moves. A band that stopped 4px short of
			     each edge read as a strip laid on the list rather than as the region's
			     own top edge, and under translucency, where the band is a different
			     material from the panel rather than more of it, those two gaps were where
			     that showed. What it costs is the focus halo: at full width its outer 4px
			     falls outside the scroll port and is clipped left and right, the way a
			     pinned heading's top edge already is.

			     **The band's paint is not here.** `section-band` is a bare hook; every
			     visual rule for it lives in main.css, and that location is a bug fix,
			     not taste — the scoped-style compiler mis-rewrites
			     `:global(.translucent) .section-band` down to bare `.translucent`,
			     which shipped 0.1.1 painting the band's frost across the whole root
			     element. The rules themselves say the band paints nothing at rest in
			     every mode, wears a `--surface-solid` plate only while actually stuck
			     over the opaque ground, and over the translucent ground wears nothing
			     ever — the rows passing beneath clip themselves out at the band's
			     bottom edge instead. The whole story, and its fallbacks, sits with
			     those rules.

			     **`pt-1 pb-2` is asymmetric because the two paddings do different
			     jobs.** The 8px below the heading is the stuck plate's dissolve tail —
			     room the heading never sits on, where the plate thins to nothing so a
			     line of text passing under fades out instead of meeting a hard
			     boundary — and it is also the offset the translucent mode's clip line
			     inherits, since the rows vanish at the band's bottom edge. The 4px
			     above is only breathing room, keeping the pinned row off the text
			     scrolling past. -->
			<div
				role="row"
				:data-row-id="rowId"
				data-section-row
				:tabindex="focused ? 0 : -1"
				class="focus-halo section-band sticky top-0 z-1 -mx-1 min-w-0 px-1 pt-1 pb-2"
			>
				<div
					role="gridcell"
					class="flex min-h-(--section-heading-height) min-w-0 items-center gap-2 px-4"
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
						<!-- **`-ml-1.5 pl-1.5` nets to zero, and that is the point: the dot is
						     placed by the gridcell, the button only decides where the pill's
						     edge falls.** What lines up is the leading mark of every row in
						     the panel, and it is measured from the *panel* edge rather than
						     from this row: the search field's magnifier sits at 20px and is
						     the anchor the other three were brought onto — this dot, the note
						     row's completion box, and the active-section chip's icon. Here
						     that 20 is the row's own `px-1` giving back the 4px the `-mx-1`
						     bleed took, plus the gridcell's `px-4`.

						     Every split of that 16 lands the dot; only this one leaves the
						     hover surface even. Carrying it all on the button would put more
						     room beside the dot on its left than `pr-1.5` leaves on its right,
						     and a dot with a lopsided pill around it reads as misplaced even
						     when it is exactly where it belongs. Six each side is what makes
						     it symmetric, and the note row carries the same `px-4` on its own
						     gridcell — anything that changes that number has to come back
						     here. -->
						<h2 :id="headingId" class="min-w-0">
							<button
								type="button"
								tabindex="-1"
								:aria-current="active ? 'true' : undefined"
								class="hover:bg-surface-hover active:bg-surface-active focus-ring -ml-1.5 flex min-w-0 items-center gap-1.5 rounded-inset py-1 pr-1.5 pl-1.5 transition-colors duration-fast"
								:class="active ? 'text-accent-text' : 'text-text-secondary'"
								@click="activate"
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
						     rather than disclosure.

						     `hit-44` because the painted box is 20px and a pointer target is
						     not: the expander reaches past the visible square without moving
						     anything or covering anything. The rule against it — adjacent
						     controls whose expanded areas would make each other unhittable —
						     does not reach here, since the only thing beside it is an
						     `aria-hidden` rule. -->
						<button
							v-if="collapseEnabled"
							type="button"
							tabindex="-1"
							:aria-expanded="!collapsed"
							:aria-label="`${collapsed ? 'Expand' : 'Collapse'} ${section.name}`"
							:title="`${collapsed ? 'Expand' : 'Collapse'} ${section.name}`"
							class="text-text-disabled hover:bg-surface-hover hover:text-text-secondary focus-ring hit-44 relative grid size-5 shrink-0 place-items-center rounded-inset transition-colors duration-fast"
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
