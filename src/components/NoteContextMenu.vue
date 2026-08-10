<script setup lang="ts">
import { CHORDS } from '@/lib/chords'

const { boundary, portalTo } = useOverlayHost()
const {
	canMerge,
	canMoveTo,
	canReorder,
	canExpandTarget,
	canOpenAttachment,
	attachmentActionLabel,
	openAttachment,
	everyTargetDone,
	copyNotes,
	copyAsList,
	copySelectionAsMarkdown,
	toggleDone,
	expand,
	edit,
	merge,
	moveFocusedBy,
	openInEditor,
	canSendToOtherDevice,
	sendToOtherDevice,
	deleteNotes,
} = useNoteActions()
</script>

<template>
	<!-- Portalled into the panel's own in-clip host: teleported to `document.body`
	     the menu would escape the rounded rect and the panel's contextmenu policy,
	     landing over a transparent region with nothing behind it. Collision
	     detection against the panel root is what flips it upward at the last card
	     of a full list.

	     `w-52`, which is a preferred width that actually fits: `ContextMenuContent`
	     caps every context menu at half the window, and half of a 440px panel is
	     220px — so `w-56` asked for 224px and was clamped on every single open,
	     making the ceiling the real width and this number a lie. Nothing here needs
	     the extra 16px; the widest row is `Copy as list` beside `Ctrl+Shift+C`, and
	     the chord is `shrink-0` so it is the label that would give first. It also
	     puts this menu at the section menu's width, which the two being siblings
	     always wanted. -->
	<ContextMenuContent
		v-if="portalTo"
		:to="portalTo"
		:collision-boundary="boundary ?? undefined"
		:collision-padding="8"
		class="text-text-secondary w-52 text-meta"
	>
		<ContextMenuItem class="min-h-6" @select="copyNotes">
			Copy
			<ContextMenuShortcut>{{ CHORDS.copy.display }}</ContextMenuShortcut>
		</ContextMenuItem>

		<ContextMenuItem class="min-h-6" @select="copyAsList">
			Copy as list
			<ContextMenuShortcut>{{ CHORDS.copyAsList.display }}</ContextMenuShortcut>
		</ContextMenuItem>

		<!-- Task-013's selection scope. It sits beside `Copy as list` rather than
		     replacing it: that one is a flat bulleted list with deliberately no
		     checkboxes and no headings, because it exists to be pasted into an LLM
		     prompt. This one is the structured export, and its `as Markdown` suffix
		     is shared with the section menu's entry and the `...` menu's so the
		     three read as one format at three scopes. No chord — Ctrl+Shift+C stays
		     with the list form. -->
		<ContextMenuItem class="min-h-6" @select="copySelectionAsMarkdown">
			Copy as Markdown
		</ContextMenuItem>

		<!-- A control names the action it performs, not the state it is in — so
		     this flips only once there is nothing left to mark. -->
		<ContextMenuItem class="min-h-6" @select="toggleDone">
			{{ everyTargetDone ? 'Mark as not done' : 'Mark as done' }}
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
			Merge notes
			<ContextMenuShortcut>{{ CHORDS.merge.display }}</ContextMenuShortcut>
		</ContextMenuItem>

		<!-- The pointer route to what Alt+Arrow and the drag grip already do, and
		     the place the chord is taught — the section menu's Move up / Move down
		     had both and notes had neither, while the refusal toasts referenced a
		     feature nothing announced. They act on the focused note, exactly as the
		     chord does. Disabled while search, the done filter or a sort holds the
		     rendered order apart from the document's; not disabled at the document's
		     edges, where the move is the same silent no-op the chord is. -->
		<ContextMenuItem :disabled="!canReorder" class="min-h-6" @select="moveFocusedBy(-1)">
			Move up
			<ContextMenuShortcut>{{ CHORDS.reorderUp.display }}</ContextMenuShortcut>
		</ContextMenuItem>

		<ContextMenuItem :disabled="!canReorder" class="min-h-6" @select="moveFocusedBy(1)">
			Move down
			<ContextMenuShortcut>{{ CHORDS.reorderDown.display }}</ContextMenuShortcut>
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

		<!-- Task-026. Disabled rather than hidden, following `canMerge` and
		     `canOpenAttachment`: the menu keeps its shape whether or not sharing is
		     set up, and someone who has configured it on their other machine can see
		     that the item exists here too. `canSendToOtherDevice` is false until
		     `useDeviceShare`'s first pull resolves, so it starts disabled and is never
		     wrongly enabled — the item is a network write, and the one direction it
		     must not fail in is "looked available and was not". -->
		<ContextMenuItem :disabled="!canSendToOtherDevice" class="min-h-6" @select="sendToOtherDevice">
			Send to my other device
		</ContextMenuItem>

		<!-- Rendered even with nothing to open, like Expand and Merge notes, so the
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
