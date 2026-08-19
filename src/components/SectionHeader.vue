<script setup lang="ts">
import { PopoverAnchor } from '@/components/ui/popover'
import { focusRowSoon, takeRow } from '@/composables/useSelection'
import type { Section } from '@/composables/useSpace'
import { isComposing } from '@/lib/chords'
import { moveFocusOnArrow } from '@/lib/popoverFocus'

const props = defineProps<{
	section: Section
	active: boolean
	rowId: string
}>()

const emit = defineEmits<{ activate: [] }>()

const { renaming, draft, setDraft, endRename, cancelRename } = useSectionEditor()
const { renameSection, notesInSection, countsInSection } = useSpace()
const { isCollapsedStored, toggleCollapsed, collapseEnabled } = useSections()
const { confirming: confirmingDelete, closeConfirm } = useSectionDelete()
const { removeSection } = useNoteActions()
const { boundary, portalTo } = useOverlayHost()

const headingId = computed(() => `section-heading-${props.section.id}`)
const editing = computed(() => renaming.value === props.section.id)

/** The stored state, not the effective one: while a query is active every
 *  section is expanded, but the control still has to say what pressing it does
 *  to the state the query is overriding. */
const collapsed = computed(() => isCollapsedStored(props.section.id))

const confirmingThis = computed(() => confirmingDelete.value === props.section.id)

/** What the band says beside the name: how much is here and how much of it is
 *  finished. Withheld while the section is empty — `0/0` beside every fresh
 *  heading is a mark that says nothing, the same rule the section menu's
 *  delete count follows. */
const counts = computed(() => countsInSection(props.section.id))

/** Spoken rather than shown, `SectionSwitcher`'s rule: `1/2` is unambiguous to
 *  a reader looking at it and means nothing read aloud on its own. */
const spokenCounts = computed(() => {
	const { done, total } = counts.value
	const notes = countMessage(total, { one: '1 note', many: (n) => `${n} notes` })
	return `${notes}, ${done} done`
})

/** Live, so the question always shows the count it would delete — and
 *  `useSectionDelete`'s reconcile withdraws the whole popover the moment the
 *  set underneath it changes, so the two cannot disagree for longer than a
 *  render. */
const deleteQuestion = computed(() => {
	const count = notesInSection(props.section.id).length
	if (count === 0) return `Delete “${props.section.name}”?`
	return countMessage(count, {
		one: `Delete “${props.section.name}” and its 1 note?`,
		many: (n) => `Delete “${props.section.name}” and its ${n} notes?`,
	})
})

const cancelButton = useTemplateRef<HTMLButtonElement>('cancelButton')

/** The delete-confirm popover's anchor, handed over by reference — the template
 *  records why it cannot be an `as-child` wrapper. On the row root rather than
 *  the gridcell, because the gridcell is `ContextMenuTrigger`'s as-child target
 *  and reka's `Slot` *deletes* the target vnode's `ref` before merging props —
 *  a ref there stays null forever, and an anchorless popover opens at the
 *  popper's off-screen placeholder. */
const rowEl = useTemplateRef<HTMLElement>('rowEl')

/** Reka's own default is the first tabbable, which the DOM order below already
 *  makes the Cancel button — stated explicitly so the safe landing does not
 *  depend on markup order staying put. A held Delete cannot confirm: focus
 *  arrives on Cancel, and the repeat guard below refuses synthesised clicks. */
function onConfirmAutoFocus(event: Event) {
	event.preventDefault()
	cancelButton.value?.focus()
}

/** Escape and an outside click arrive here through reka's layer; the Cancel
 *  button lands in the same place through `cancelDelete`. */
function onConfirmOpen(open: boolean) {
	if (!open) cancelDelete()
}

function cancelDelete() {
	closeConfirm()
	// The question was asked from the row, so declining it returns there —
	// without this, focus dies with the popover and the list is unreachable.
	focusRowSoon(props.rowId)
}

/** Closing first keeps `update:open` quiet — the prop drives it, so reka emits
 *  nothing — and `removeSection` owns the focus handoff to the surviving row,
 *  because this one is about to be gone. */
function confirmDelete() {
	closeConfirm()
	void removeSection(props.section)
}

/** `DoneFilter`'s held-key guard, verbatim and for its reason: the browser
 *  synthesises a click from every repeat of an Enter keydown, and the popover
 *  autofocuses a control on open. */
function onConfirmKeydown(event: KeyboardEvent) {
	if (event.repeat && (event.key === 'Enter' || event.key === ' ')) {
		event.preventDefault()
		return
	}
	moveFocusOnArrow(event)
}

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
	<div
		ref="rowEl"
		role="row"
		:data-row-id="rowId"
		data-section-row
		tabindex="0"
		class="focus-inset section-band sticky top-0 z-1 -ml-1 -mr-[var(--region-inset-r,--spacing(1))] min-w-0 pl-1 pr-[var(--region-inset-r,--spacing(1))] pt-1 pb-2"
	>
		<!-- A `grid` may own only `row` and `rowgroup`, and a `rowgroup` only `row`,
		     so the section header is itself a row rather than an <h2> sitting between
		     rowgroups. It pays for itself: the header becomes keyboard-reachable
		     through ordinary arrow navigation instead of needing a bespoke path.
		     Header rows carry no aria-selected — they are not selectable.

		     The context menu attached here is the *section* menu. A note menu must
		     not open on a header row, which is why the trigger lives on note rows
		     only and this one carries its own content.

		     **The row element is this component's root, and that is load-bearing** —
		     the same constraint `NoteCard` documents: a `<TransitionGroup>` child
		     must resolve to a single element root, and reka-ui's renderless chain
		     (`PopperRoot` renders a slot fragment) can never be one. So the whole
		     `ContextMenu` lives inside the row, its trigger merged onto the gridcell,
		     with the delete popover anchored to the row by reference (the popover's
		     own comment says why it must be a reference and why the `Popover` wraps
		     the `ContextMenu` rather than sitting inside it). The
		     band's own `px-1 pt-1 pb-2`
		     sliver falls outside the trigger; a right-click there hits the row and
		     opens no menu, which is the cost of the row animating at all.

		     And as in `NoteCard`: every template comment sits *inside* the row,
		     because a comment beside the root makes the dev subtree a fragment
		     whose `el` is a text anchor, which `TransitionGroup` skips when it
		     installs leave and move hooks. -->
		<!-- **Square, and that is a property of the band rather than of the row.**
		     The corner used to be the small-control tier's 10px, rounding the focus
		     ring and the pinned band together. A band is what this actually is: it
		     spans the region, it rides its top edge, and a rounded rectangle riding
		     a straight edge reads as a card that has come loose rather than as a
		     heading that has stuck. `focus-inset`'s outline follows this square
		     box, which is exactly right for a band.

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

		     **The band spans the region edge to edge, which is what the negative
		     margins buy.** They reach past the scroll region's own card inset —
		     4px on the left, `--region-inset-r` on the right, which is the same
		     4px until a scrollbar starts consuming it (see main.css) — and the
		     matching paddings put the heading back where it was, so only the
		     fill moves. A band that stopped 4px short of
		     each edge read as a strip laid on the list rather than as the region's
		     own top edge, and under translucency, where the band is a different
		     material from the panel rather than more of it, those two gaps were where
		     that showed. It is also why the focus indicator is `focus-inset` rather
		     than the note row's halo: everything a halo paints falls outside the
		     band's box, and at full width every edge of that box meets the scroll
		     port — the first heading at scroll 0 kept only the bottom arc of its
		     halo. The inset outline is whole wherever the band itself is.

		     **`tabindex="0"`: every row in the grid is a Tab stop.** Tab walks
		     the list row by row, bands and notes alike (user ruling, 2026-08-11,
		     reversing the sections-only order), while the descendants of every
		     row stay at -1 and are reached through F2. The roving target still
		     decides where *arrows* resume; the
		     grid's focusin handler keeps it in step with any Tab or click, which
		     is also what makes "click anywhere on the band" focus the section —
		     the row is click-focusable by its tabindex, and `:focus-visible`
		     keeps the outline keyboard-only.

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
		<!-- The keyboard Delete's confirmation, anchored to the heading it asks
		     about. A popover rather than a state of the row for `DoneFilter`'s
		     reason: the question must name a count of any size without the row
		     lending it width. The row itself stays exactly what it was — the
		     root renders nothing and the anchor arrives by reference, so the
		     grid's aria-required-children contract sees no new child.
		     Opened only by `NoteList`'s Delete case; the context menu keeps its
		     own unconfirmed item, which is already a second gesture. Cancel is
		     first in the DOM *and* takes the autofocus explicitly, so the
		     destructive control can never be where the opening press lands.

		     **The `Popover` must wrap the `ContextMenu`, and the anchor must be
		     the `reference` prop rather than an `as-child` wrapper on the cell.**
		     Every reka popper finds its anchor by injecting the *nearest* popper
		     root above it, and `ContextMenuTrigger` registers the right-click
		     point through exactly such an internal anchor. With the `Popover`
		     between the menu root and its trigger, that point landed in the
		     popover's context; the menu's own context never received an anchor,
		     so the menu opened unpositioned at its off-screen placeholder — an
		     invisible open layer whose only effect was to swallow the next
		     click. The same nearest-root rule is why the popover's anchor sits
		     outside the `ContextMenu` and names the row by reference: wrapped
		     around the cell, it would hand the popover's anchor to the menu's
		     context in mirror image. -->
		<Popover :open="confirmingThis" @update:open="onConfirmOpen">
			<ContextMenu>
				<ContextMenuTrigger as-child>
					<!-- **`pl-1.5`, deliberately off the leading-mark column.** The heading
						     used to pay `px-4` so its dot sat on the 20px line the search
						     magnifier and the completion box share; the user reversed that
						     (2026-08-11): a heading *outdented* from the notes under it reads
						     as the level above them, which one shared column flattened. The
						     6px puts the name's pill edge on the 4px inset the note cards'
						     own boxes sit at — the heading hangs off the cards' edge, not off
						     a second arbitrary number. The right padding is 16px less the
						     scrollbar's overflow share (`--row-inset-comp`), which is what
						     keeps the chevron exactly where every other trailing control
						     sits whether or not the list scrolls. -->
					<div
						role="gridcell"
						class="flex min-h-(--section-heading-height) min-w-0 items-center gap-2 pr-[calc(--spacing(4)-var(--row-inset-comp,0px))] pl-1.5"
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
							<!-- **`-ml-1.5 pl-1.5` nets to zero: the dot is placed by the
						     gridcell, the button only decides where the pill's edge falls.**
						     The heading no longer sits on the leading-mark column the search
						     magnifier and the completion box share — the gridcell above
						     records the outdent ruling — but the split still matters for the
						     pill itself: six each side of the dot is what keeps the hover
						     surface symmetric around it, where carrying the inset on the
						     button alone would leave more room on the dot's left than
						     `pr-1.5` leaves on its right. -->
							<h2 :id="headingId" class="min-w-0">
								<button
									type="button"
									tabindex="-1"
									:aria-current="active ? 'true' : undefined"
									class="section-title hover:bg-surface-hover active:bg-surface-active focus-ring relative -ml-1.5 flex min-w-0 items-center gap-1.5 rounded-inset py-1 pr-1.5 pl-1.5 transition-colors duration-fast"
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

									<!-- After the name and inside its pill, so the two never separate
								     when the name truncates — the count is the part that must stay
								     readable, which is why it is the `shrink-0` of the pair.
								     `text-text-secondary` in both states: on an active heading the
								     accent stays the name's alone, so the count reads as annotation
								     rather than as more name. -->
									<template v-if="counts.total > 0">
										<span
											aria-hidden="true"
											data-section-counts
											class="text-text-secondary shrink-0 text-label tabular-nums"
										>
											{{ counts.done }}/{{ counts.total }}
										</span>
										<span class="sr-only">{{ spokenCounts }}</span>
									</template>
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
				</ContextMenuTrigger>

				<SectionContextMenu :section="section" />
			</ContextMenu>

			<!-- `as="template"`, so no element lands between the row and its cells;
			     the anchor is the row itself — see `rowEl` for why not the cell. -->
			<PopoverAnchor as="template" :reference="rowEl ?? undefined" />

			<PopoverContent
				v-if="portalTo"
				:to="portalTo"
				align="start"
				:collision-boundary="boundary ?? undefined"
				:collision-padding="8"
				class="w-64 text-meta"
				@open-auto-focus="onConfirmAutoFocus"
				@keydown="onConfirmKeydown"
			>
				<p class="text-text-primary">{{ deleteQuestion }}</p>
				<div class="mt-2 flex items-center justify-end gap-2">
					<button
						ref="cancelButton"
						type="button"
						data-section-delete-cancel
						class="panel-button min-h-6 px-2"
						@click="cancelDelete"
					>
						Cancel
					</button>
					<button
						type="button"
						data-section-delete-confirm
						class="panel-button text-destructive-text min-h-6 px-2"
						@click="confirmDelete"
					>
						Delete
					</button>
				</div>
			</PopoverContent>
		</Popover>
	</div>
</template>

<style scoped>
/* The title's real hit area, much larger than the pill it paints — the same
   trade the chevron makes with `hit-44`, shaped to the band instead of a
   square, because a 44px box centred on a wide, short button would leave most
   of the button's own width uncovered. It spans the heading band's full
   height — the 4px above and 8px below mirror the band's `pt-1`/`pb-2` —
   reaches the panel edge on the left, and ends a little past the pill on the
   right, so the separator's length and the chevron keep their own targets.
   The button's `relative` is what the expander resolves against. */
.section-title::before {
	content: '';
	position: absolute;
	top: -4px;
	right: -12px;
	bottom: -8px;
	left: -4px;
}
</style>
