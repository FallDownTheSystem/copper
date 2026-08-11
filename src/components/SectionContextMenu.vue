<script setup lang="ts">
import type { Section } from '@/composables/useSpace'
import { CHORDS } from '@/lib/chords'

const props = defineProps<{ section: Section }>()

const { boundary, portalTo } = useOverlayHost()
const { sections, countsInSection, reorderSection, setActiveSection } = useSpace()
const { beginRename } = useSectionEditor()
const { selectSection } = useSelection()
// `removeSection` carries the delete, the undo message and the focus handoff —
// one step shared with the keyboard confirm, so the two paths cannot drift.
// `deleteDoneInSection` shares its body with the header control's two scopes,
// so this third one is the same single command and the same undo toast.
const { copySectionAsMarkdown, removeSection, deleteDoneInSection } = useNoteActions()

const index = computed(() => sections.value.findIndex((entry) => entry.id === props.section.id))
const isFirst = computed(() => index.value <= 0)
const isLast = computed(() => index.value === sections.value.length - 1)
/** The store refuses to delete the last remaining section, so a capture target
 *  always exists. Disabled here rather than left to fail. */
const isOnly = computed(() => sections.value.length < 2)

/** `index` is interpreted against the list *after* the section has been removed
 *  from it, which is why moving down is `index + 1` rather than `index + 2`. */
function move(delta: number) {
	return reorderSection(props.section.id, index.value + delta)
}

/** What each delete takes with it. Live, so a menu opened over a section that
 *  just gained a capture shows the counts the presses would actually delete. */
const counts = computed(() => countsInSection(props.section.id))
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
		     openings. The chords are the same pair the note menu shows: on a
		     focused header row Alt+Arrow carries the section, and this is where a
		     pointer user learns that. -->
		<ContextMenuItem :disabled="isFirst" class="min-h-6" @select="move(-1)">
			Move up
			<ContextMenuShortcut>{{ CHORDS.reorderUp.display }}</ContextMenuShortcut>
		</ContextMenuItem>
		<ContextMenuItem :disabled="isLast" class="min-h-6" @select="move(1)">
			Move down
			<ContextMenuShortcut>{{ CHORDS.reorderDown.display }}</ContextMenuShortcut>
		</ContextMenuItem>

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

		<!-- No `Sort` submenu: the order is one document-wide setting now, and this
		     menu is where you act on *this* section. It lives in the header beside
		     the done filter, where it states the mode in effect without anything
		     having to be opened to read it — which is also what retired the marker
		     `SectionHeader` used to carry. -->
		<ContextMenuSeparator />

		<!-- Above the section delete because it takes less with it — the two
		     destructive rows read smallest first. Disabled rather than hidden at
		     zero, like every other row here, so the menu keeps its shape and the
		     section delete cannot inherit a press aimed at this row from memory.
		     This section's done notes, not the active section's — the header
		     control offers that scope, and the two can name different sections. -->
		<ContextMenuItem
			:disabled="counts.done === 0"
			variant="destructive"
			class="min-h-6"
			@select="deleteDoneInSection(section.id)"
		>
			Delete done notes{{ counts.done > 0 ? ` (${counts.done})` : '' }}
		</ContextMenuItem>

		<!-- `Delete section (11)`, not `Delete section and its notes`: the sentence
		     form wrapped to two lines beside the shortcut in a w-52 menu, and the
		     row read as saying delete twice. The count carries the warning the
		     sentence carried — what the press takes with it — in the width of a
		     number, and the undo toast still names it in full after. -->
		<ContextMenuItem
			:disabled="isOnly"
			variant="destructive"
			class="min-h-6"
			@select="removeSection(section)"
		>
			Delete section{{ counts.total > 0 ? ` (${counts.total})` : '' }}
			<ContextMenuShortcut>{{ CHORDS.remove.display }}</ContextMenuShortcut>
		</ContextMenuItem>
	</ContextMenuContent>
</template>
