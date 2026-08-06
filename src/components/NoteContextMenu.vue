<script setup lang="ts">
import { CHORDS } from '@/lib/chords'

const { boundary, portalTo } = useOverlayHost()
const {
	canMerge,
	canMoveTo,
	canExpandTarget,
	canOpenAttachment,
	attachmentActionLabel,
	openAttachment,
	everyTargetDone,
	copyNotes,
	copyAsList,
	toggleDone,
	expand,
	edit,
	merge,
	openInEditor,
	deleteNotes,
} = useNoteActions()
</script>

<template>
	<!-- Portalled into the panel's own in-clip host: teleported to `document.body`
	     the menu would escape the rounded rect and the panel's contextmenu policy,
	     landing over a transparent region with nothing behind it. Collision
	     detection against the panel root is what flips it upward at the last card
	     of a full list. -->
	<ContextMenuContent
		v-if="portalTo"
		:to="portalTo"
		:collision-boundary="boundary ?? undefined"
		:collision-padding="8"
		class="text-text-secondary w-56 text-meta"
	>
		<ContextMenuItem class="min-h-6" @select="copyNotes">
			Copy
			<ContextMenuShortcut>{{ CHORDS.copy.display }}</ContextMenuShortcut>
		</ContextMenuItem>

		<ContextMenuItem class="min-h-6" @select="copyAsList">
			Copy as List
			<ContextMenuShortcut>{{ CHORDS.copyAsList.display }}</ContextMenuShortcut>
		</ContextMenuItem>

		<!-- A control names the action it performs, not the state it is in — so
		     this flips only once there is nothing left to mark. -->
		<ContextMenuItem class="min-h-6" @select="toggleDone">
			{{ everyTargetDone ? 'Mark as Not Done' : 'Mark as Done' }}
			<ContextMenuShortcut>{{ CHORDS.markDone.display }}</ContextMenuShortcut>
		</ContextMenuItem>

		<!-- Disabled items stay rendered so the menu does not change shape between
		     openings. Expand and Move to have no chord and show none. -->
		<ContextMenuItem :disabled="!canExpandTarget" class="min-h-6" @select="expand">
			Expand
		</ContextMenuItem>

		<ContextMenuItem class="min-h-6" @select="edit">
			Edit
			<ContextMenuShortcut>{{ CHORDS.edit.display }}</ContextMenuShortcut>
		</ContextMenuItem>

		<ContextMenuItem :disabled="!canMerge" class="min-h-6" @select="merge">
			Merge Notes
			<ContextMenuShortcut>{{ CHORDS.merge.display }}</ContextMenuShortcut>
		</ContextMenuItem>

		<ContextMenuSub>
			<ContextMenuSubTrigger :disabled="!canMoveTo" class="min-h-6">Move to</ContextMenuSubTrigger>
			<ContextMenuSubContent class="text-text-secondary w-44 text-meta">
				<MoveToSubmenu />
			</ContextMenuSubContent>
		</ContextMenuSub>

		<ContextMenuItem class="min-h-6" @select="openInEditor">
			Edit in editor
			<ContextMenuShortcut>{{ CHORDS.openInEditor.display }}</ContextMenuShortcut>
		</ContextMenuItem>

		<!-- Rendered even with nothing to open, like Expand and Merge Notes, so the
		     menu does not change shape between openings. The label names what will
		     happen — an image opens, everything else is revealed — rather than
		     saying "Open" and then doing something else. -->
		<ContextMenuItem :disabled="!canOpenAttachment" class="min-h-6" @select="openAttachment">
			{{ attachmentActionLabel }}
		</ContextMenuItem>

		<ContextMenuSeparator />

		<!-- Not in the reference's eight, and added by the Q7 scope expansion that
		     brought note deletion into this phase. Separated and last, because it is
		     the only destructive item. -->
		<ContextMenuItem variant="destructive" class="min-h-6" @select="deleteNotes">
			Delete
			<ContextMenuShortcut>{{ CHORDS.remove.display }}</ContextMenuShortcut>
		</ContextMenuItem>
	</ContextMenuContent>
</template>
