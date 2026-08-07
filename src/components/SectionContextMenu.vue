<script setup lang="ts">
import type { Section } from '@/composables/useSpace'

const props = defineProps<{ section: Section }>()

const { boundary, portalTo } = useOverlayHost()
const { sections, notesInSection, deleteSection, reorderSection, setActiveSection } = useSpace()
const { beginRename } = useSectionEditor()
const { setMessage } = useStatusMessage()
const { selectSection } = useSelection()
const { copySectionAsMarkdown } = useNoteActions()
const { sortOf, setSort } = useNoteList()

/**
 * The three orders, named as the list will read rather than as the field they
 * sort by — "Oldest first" says what the top of the list will be, which is the
 * thing the user is choosing.
 *
 * `Manual` is first and is the document's own order: the one every drag and every
 * Alt+Arrow writes, and the only mode in which either is permitted.
 */
const SORT_OPTIONS = [
	{ mode: 'manual', label: 'Manual' },
	{ mode: 'oldest', label: 'Oldest first' },
	{ mode: 'newest', label: 'Newest first' },
] as const

const index = computed(() => sections.value.findIndex((entry) => entry.id === props.section.id))
const isFirst = computed(() => index.value <= 0)
const isLast = computed(() => index.value === sections.value.length - 1)
/** The store refuses to delete the last remaining section, so a capture target
 *  always exists. Disabled here rather than left to fail. */
const isOnly = computed(() => sections.value.length < 2)

async function remove() {
	const count = notesInSection(props.section.id).length
	const result = await deleteSection(props.section.id)
	if (!result) return
	// No confirmation dialog: the whole operation is one undo, and an undoable
	// action reads better as a reversible one than as a question.
	setMessage(
		count === 0
			? `Deleted “${props.section.name}” · Ctrl+Z to undo`
			: countMessage(count, {
					one: `Deleted “${props.section.name}” and 1 note · Ctrl+Z to undo`,
					many: (n) => `Deleted “${props.section.name}” and ${n} notes · Ctrl+Z to undo`,
				}),
	)
}

/** `index` is interpreted against the list *after* the section has been removed
 *  from it, which is why moving down is `index + 1` rather than `index + 2`. */
function move(delta: number) {
	return reorderSection(props.section.id, index.value + delta)
}
</script>

<template>
	<ContextMenuContent
		v-if="portalTo"
		:to="portalTo"
		:collision-boundary="boundary ?? undefined"
		:collision-padding="8"
		class="text-text-secondary w-52 text-meta"
	>
		<ContextMenuItem class="min-h-6" @select="beginRename(section.id, section.name)">
			Rename
		</ContextMenuItem>

		<ContextMenuItem class="min-h-6" @select="setActiveSection(section.id)">
			Make active section
		</ContextMenuItem>

		<!-- Disabled items stay rendered, so the menu does not change shape between
		     openings. -->
		<ContextMenuItem :disabled="isFirst" class="min-h-6" @select="move(-1)"
			>Move up</ContextMenuItem
		>
		<ContextMenuItem :disabled="isLast" class="min-h-6" @select="move(1)"
			>Move down</ContextMenuItem
		>

		<ContextMenuSeparator />

		<!-- Below the four entries that operate on the section itself, because these
		     two reach its *notes*. `Select all` is scoped to this section, unlike
		     Ctrl+A, and works while the section is collapsed — folding rows away
		     never narrowed what an action targets. `Copy section as Markdown` shares
		     its renderer and its suffix with the note menu's `Copy as Markdown` and
		     the `...` menu's `Copy all as Markdown`, so all three read as one format
		     at three scopes; it takes the whole section rather than whatever a query
		     left showing, matching the document-wide copy. -->
		<ContextMenuItem class="min-h-6" @select="selectSection(section.id)">
			Select all
		</ContextMenuItem>

		<ContextMenuItem class="min-h-6" @select="copySectionAsMarkdown(section.id)">
			Copy section as Markdown
		</ContextMenuItem>

		<!-- Sorting lives here rather than in a control on the header row, and the
		     reason is consistency rather than space: every other section-scoped
		     operation — rename, reorder, select all, delete — is already in this
		     menu, and a section is where you right-click to act on one. A permanent
		     button in the header row would be the only per-section control in the
		     list and would have to earn its width on every section forever.

		     Discoverability is paid for on the other side instead: `SectionHeader`
		     shows a marker whenever a section is not on Manual, so a computed order
		     is never a mystery and there is somewhere to explain why the grip has
		     gone. A submenu rather than three top-level items, so the menu does not
		     grow by three rows for a setting most sections never leave. -->
		<ContextMenuSub>
			<ContextMenuSubTrigger class="min-h-6">Sort</ContextMenuSubTrigger>
			<ContextMenuSubContent class="text-text-secondary w-44 text-meta">
				<ContextMenuItem
					v-for="option in SORT_OPTIONS"
					:key="option.mode"
					class="min-h-6"
					@select="setSort(section.id, option.mode)"
				>
					<!-- The same marker the active section and the space list use, so
					     "this is the one in effect" reads the same way everywhere. -->
					<ActiveMarker
						:active="sortOf(section.id) === option.mode"
						:label="`${option.label} sort`"
					>
						<span class="truncate">{{ option.label }}</span>
					</ActiveMarker>
				</ContextMenuItem>
			</ContextMenuSubContent>
		</ContextMenuSub>

		<ContextMenuSeparator />

		<ContextMenuItem :disabled="isOnly" variant="destructive" class="min-h-6" @select="remove">
			Delete section and its notes
		</ContextMenuItem>
	</ContextMenuContent>
</template>
