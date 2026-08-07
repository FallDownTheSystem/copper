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

const { previewFor, requestPreview, previewEpoch, openAttachment } = useAttachments()
const { open: openViewer } = useImageViewer()

const button = useTemplateRef<HTMLButtonElement>('button')

// The ask, driven from a watcher rather than from the read below: `previewFor`
// is consumed by a computed, and requesting as a side effect of reading would
// write the preview cache during that computed's evaluation. `immediate` keeps
// the two inseparable — a card cannot render without having asked.
//
// The epoch is the second dependency and is not optional. `clearPreviews`
// revokes the cache under cards that are still mounted showing the same file,
// which leaves them with no preview and nothing outstanding; watching the file
// alone, a card in that state would sit on a spinner forever. Reading as a side
// effect of rendering used to cover this for free, and this is what replaces it.
watch(
	() => [props.attachment.file, previewEpoch.value] as const,
	([file]) => requestPreview(file),
	{ immediate: true },
)

// Read once, right after the immediate watcher above has already asked: a
// thumbnail that was cached is `ready` by now, and one that has to be decoded is
// not. Only the second kind should fade — a card whose image is already there at
// panel open would otherwise animate a picture the user is looking at, and every
// visible card would do it at once. Deliberately not reactive: the question is
// "was it there when this card mounted", which has exactly one answer.
const arrivedBeforeMount = previewFor(props.attachment.file).state === 'ready'

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

/**
 * Whether this attachment has a picture, which is the only signal the frontend
 * has for "is this an image" — `is_thumbnailable` is decided in Rust from the
 * bytes on disk, and the document's `mime` is hand-editable.
 *
 * A `ready` preview with a **null** url is the honest description of a `.pdf`,
 * and it has to fall to the OS path below: opening the viewer on one would show
 * an empty sheet with no way to reach the file at all.
 */
const viewable = computed(() => preview.value.state === 'ready' && preview.value.url !== null)

/**
 * The primary gesture, and task-014 moves what it means.
 *
 * Double-click and `Enter` were task-011's route to the OS viewer; they are the
 * two gestures a file has everywhere, so they belong to the thing the user is
 * most likely to want — and for a screenshot pasted into a note, that is looking
 * at it, not launching Photos over the top of Copper. The OS route keeps `Space`,
 * which was already bound here and did the same thing as `Enter`.
 *
 * Anything with no picture keeps the old behaviour on every gesture: the viewer
 * has nothing to show it, and `attachment_open` reveals it in Explorer.
 */
async function activate() {
	if (unavailable.value) return
	if (viewable.value) {
		void openViewer(props.attachment, button.value)
		return
	}
	await openInSystem()
}

async function openInSystem() {
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
	     behaves everywhere else.

	     The label names the primary gesture's destination rather than listing both:
	     a screen reader reading "View or open" on every thumbnail would be reading
	     the implementation. -->
	<button
		ref="button"
		type="button"
		:tabindex="tabIndex"
		:disabled="unavailable"
		:aria-label="
			unavailable
				? `${attachment.name} — unavailable`
				: `${viewable ? 'View' : 'Open'} ${attachment.name}, ${formatBytes(attachment.bytes)}`
		"
		class="squircle border-separator hover:bg-surface-hover outline-focus-ring flex min-h-16 w-full min-w-0 items-center gap-2 rounded-lg border p-1.5 text-left transition-colors duration-fast focus-visible:outline-2 focus-visible:-outline-offset-2 disabled:cursor-default disabled:hover:bg-transparent"
		@click.prevent
		@dblclick.stop.prevent="activate"
		@keydown.enter.prevent="activate"
		@keydown.space.prevent="openInSystem"
	>
		<span
			class="bg-surface-hover text-text-disabled grid shrink-0 place-items-center overflow-hidden rounded-sm"
			:style="boxStyle"
		>
			<!-- No `alt` text of its own: the button already carries the filename, and
			     a second announcement of the same name is noise. -->
			<img
				v-if="thumbUrl"
				:src="thumbUrl"
				alt=""
				class="size-full object-cover"
				:class="arrivedBeforeMount ? '' : 'animate-in fade-in'"
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
