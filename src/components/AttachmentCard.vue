<script setup lang="ts">
import type { Attachment } from '@/composables/useAttachments'

const props = defineProps<{
	attachment: Attachment
	/** Task-004's interaction mode: `0` makes this reachable by Tab inside the
	 *  focused row, `-1` keeps it out of the tab order like every other in-card
	 *  control. */
	tabIndex: number
}>()

const emit = defineEmits<{ message: [string] }>()

const { previewFor, openAttachment } = useAttachments()

const preview = computed(() => previewFor(props.attachment.file))
const unavailable = computed(() => preview.value.state === 'missing')
const thumbUrl = computed(() => (preview.value.state === 'ready' ? preview.value.url : null))

/**
 * The leading box's size, fixed in **height** and derived in width from the
 * dimensions stored at ingest.
 *
 * This is what keeps a thumbnail arriving from reflowing the list. Task-004's
 * sticky-bottom pin measures `scrollHeight` and re-asserts until the list
 * settles, and a card that grew when its image finished loading would move the
 * content under a reader who had already scrolled. A constant height means the
 * arrival changes nothing vertically at all, whatever the image turns out to
 * be — which is a stronger guarantee than reserving space from `width`/`height`
 * alone, since those are advisory and absent for every non-image.
 */
const THUMB_HEIGHT = 56
const MAX_THUMB_WIDTH = 148

const boxStyle = computed(() => {
	const { width, height } = props.attachment
	const ratio = width && height && height > 0 ? width / height : 1
	const scaled = Math.round(THUMB_HEIGHT * ratio)
	return {
		width: `${Math.min(Math.max(scaled, THUMB_HEIGHT), MAX_THUMB_WIDTH)}px`,
		height: `${THUMB_HEIGHT}px`,
	}
})

async function open() {
	if (unavailable.value) return
	const failure = await openAttachment(props.attachment.file)
	if (failure) emit('message', failure)
}
</script>

<template>
	<!-- A button so the keyboard path is the platform's rather than a hand-rolled
	     one. `@click.prevent` neutralises the button's own activation without
	     stopping propagation, so a single click still reaches the row and selects
	     the note — opening is deliberately the *double*-click, matching how a file
	     behaves everywhere else. -->
	<button
		type="button"
		:tabindex="tabIndex"
		:disabled="unavailable"
		:aria-label="
			unavailable
				? `${attachment.name} — unavailable`
				: `Open ${attachment.name}, ${formatBytes(attachment.bytes)}`
		"
		class="border-separator hover:bg-surface-hover outline-focus-ring flex min-h-16 w-full min-w-0 items-center gap-2 rounded-md border p-1.5 text-left transition-colors duration-fast focus-visible:outline-2 focus-visible:-outline-offset-2 disabled:cursor-default disabled:hover:bg-transparent"
		@click.prevent
		@dblclick.stop.prevent="open"
		@keydown.enter.prevent="open"
		@keydown.space.prevent="open"
	>
		<span
			class="bg-surface-hover text-text-disabled grid shrink-0 place-items-center overflow-hidden rounded"
			:style="boxStyle"
		>
			<!-- No `alt` text of its own: the button already carries the filename, and
			     a second announcement of the same name is noise. -->
			<img
				v-if="thumbUrl"
				:src="thumbUrl"
				alt=""
				class="size-full object-cover"
				draggable="false"
			/>
			<IconLucideTriangleAlert
				v-else-if="unavailable"
				class="text-destructive size-5"
				aria-hidden="true"
				focusable="false"
			/>
			<IconLucideFile v-else class="size-5" aria-hidden="true" focusable="false" />
		</span>

		<span class="min-w-0 flex-1">
			<!-- The **original** filename, which is metadata. The stored name is a
			     content hash and means nothing to anyone. -->
			<span class="text-text-primary block truncate text-meta">{{ attachment.name }}</span>
			<span v-if="unavailable" class="text-destructive mt-0.5 block text-meta line-clamp-2">
				{{ preview.state === 'missing' ? preview.reason : '' }}
			</span>
			<span v-else class="text-text-secondary mt-0.5 block text-meta">
				{{ formatBytes(attachment.bytes) }}
			</span>
		</span>
	</button>
</template>
