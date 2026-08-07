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
			<div
				role="row"
				:data-row-id="rowId"
				data-section-row
				:tabindex="focused ? 0 : -1"
				class="min-w-0 outline-focus-ring focus-visible:outline-2 focus-visible:-outline-offset-2"
			>
				<div role="gridcell" class="flex min-h-6 min-w-0 items-center gap-2 px-3">
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
						<!-- The name leads and the chevron follows it, so the heading starts
						     at the row's own left edge rather than behind a control. The
						     chevron is a disclosure sitting beside what it discloses, and
						     still rotates to point down while the section is open.

						     `shrink` rather than `shrink-0`, and that is what the reordering
						     costs: with the chevron behind it, a long name could only push
						     the separator away, but ahead of it an unshrinkable name would
						     push the chevron itself out of the row. The name truncates now,
						     which is what the inner `truncate` was always for. -->
						<h2 :id="headingId" class="min-w-0">
							<button
								type="button"
								tabindex="-1"
								:aria-current="active ? 'true' : undefined"
								class="hover:bg-surface-hover active:bg-surface-active flex min-w-0 items-center gap-1.5 rounded-md px-1.5 py-1 transition-colors duration-fast"
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

						<!-- Withdrawn while a query is active rather than rendered inert.
						     Search already decides what is on screen and overrides collapse
						     entirely, so a control that rotated its chevron and changed
						     nothing visible would read as broken. The stored state survives
						     and comes back when the query clears; the fixed-width stand-in
						     keeps the separator from shifting sideways in the meantime. -->
						<button
							v-if="collapseEnabled"
							type="button"
							tabindex="-1"
							:aria-expanded="!collapsed"
							:aria-label="`${collapsed ? 'Expand' : 'Collapse'} ${section.name}`"
							class="text-text-disabled hover:bg-surface-hover hover:text-text-secondary outline-focus-ring grid size-5 shrink-0 place-items-center rounded transition-colors duration-fast focus-visible:outline-2 focus-visible:-outline-offset-1"
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

						<span aria-hidden="true" class="bg-separator h-px min-w-0 flex-1" />
					</template>
				</div>
			</div>
		</ContextMenuTrigger>

		<SectionContextMenu :section="section" />
	</ContextMenu>
</template>
