<script setup lang="ts">
import { UNDO_ACTION } from '@/composables/useNoteActions'
import type { Section } from '@/composables/useSpace'
import { CHORDS } from '@/lib/chords'

const props = defineProps<{ section: Section }>()

const { boundary, portalTo } = useOverlayHost()
const { sections, notesInSection, deleteSection, reorderSection, setActiveSection } = useSpace()
const { beginRename } = useSectionEditor()
const { setMessage } = useStatusMessage()
const { selectSection } = useSelection()
const { copySectionAsMarkdown } = useNoteActions()

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
	// action reads better as a reversible one than as a question. The chord is not
	// spelled out in the sentence, for the reason the note deletions stopped
	// spelling it out: the pill carries a button that takes that same one step.
	setMessage(
		count === 0
			? `Deleted “${props.section.name}”`
			: countMessage(count, {
					one: `Deleted “${props.section.name}” and 1 note`,
					many: (n) => `Deleted “${props.section.name}” and ${n} notes`,
				}),
		UNDO_ACTION,
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

		<ContextMenuItem :disabled="isOnly" variant="destructive" class="min-h-6" @select="remove">
			Delete section and its notes
		</ContextMenuItem>
	</ContextMenuContent>
</template>
