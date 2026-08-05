<script setup lang="ts">
import type { Section } from '@/composables/useSpace'

const props = defineProps<{ section: Section }>()

const { boundary, portalTo } = useOverlayHost()
const { sections, notesInSection, deleteSection, reorderSection, setActiveSection } = useSpace()
const { beginRename } = useSectionEditor()
const { setMessage } = useStatusMessage()

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

		<ContextMenuItem :disabled="isOnly" variant="destructive" class="min-h-6" @select="remove">
			Delete section and its notes
		</ContextMenuItem>
	</ContextMenuContent>
</template>
