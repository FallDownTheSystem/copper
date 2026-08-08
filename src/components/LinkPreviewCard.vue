<script setup lang="ts">
/**
 * One link's Open Graph card, below the note body it belongs to.
 *
 * **The picture is bytes, not a URL.** `usePreviews` receives a PNG over IPC and
 * wraps it in an object URL, so this `<img>` never points at a third party —
 * which is the same rule `useMarkdown` enforces by refusing to emit an `<img>`
 * for a Markdown image at all. A card that took `og:image` as an `src` would
 * reopen exactly the read-receipt hole that rule exists to close, every time the
 * note was rendered rather than once when it was fetched.
 */
import { openUrl } from '@tauri-apps/plugin-opener'

const props = defineProps<{ url: string }>()

const { previewFor } = usePreviews()

const state = computed(() => previewFor(props.url))
const preview = computed(() => (state.value.state === 'ready' ? state.value.preview : null))
const imageUrl = computed(() => (state.value.state === 'ready' ? state.value.imageUrl : null))

/**
 * The picture's box, reserved at a constant size from the moment the card
 * mounts.
 *
 * This is `AttachmentCard`'s `THUMB_HEIGHT` reasoning applied to a slower
 * arrival. The metadata and the picture come back on two separate round trips,
 * so a box that sized itself to the image would move every note below it when
 * the second one landed — and task-004's sticky-bottom pin measures
 * `scrollHeight` and re-asserts, so the reader who had scrolled would be the one
 * it moved. A fixed box means the arrival changes nothing vertically at all.
 *
 * The card as a whole appears only once the metadata is ready. A skeleton that
 * resolved to "no preview" would flash placeholder furniture under every link in
 * a note — and most links have no card, because most pages carry no Open Graph
 * tags and a fetch can simply fail.
 */
const THUMB_WIDTH = 96
const THUMB_HEIGHT = 64

/**
 * The whole card opens the page, and so does its picture — `openUrl`, not the
 * fullscreen image viewer.
 *
 * The viewer is keyed to the `Attachment` type and would need one fabricated to
 * accept this, but the interaction is the weaker one regardless: a downscaled
 * social card blown up to fill the panel shows strictly less than the page it
 * points at, and the reason a reader clicks a preview is to go there.
 */
function open() {
	void openUrl(props.url)
}
</script>

<template>
	<!-- `tabindex="-1"`, matching every anchor `useMarkdown` emits into the prose
	     above. The card is a second route to a link that is already on screen, so
	     giving it a Tab stop would add one to every previewed link in the list and
	     break the grid's one-Tab-stop contract for no reach the reader did not
	     already have.

	     `aria-label` rather than letting the contents be read: the card's title
	     and description are a copy of the page's, and a screen reader meeting them
	     after the link itself should be told what the box *is* first. -->
	<button
		v-if="preview"
		type="button"
		tabindex="-1"
		:aria-label="`Open ${preview.title ?? url}`"
		class="squircle border-separator hover:bg-surface-hover focus-ring flex w-full min-w-0 items-stretch gap-2 overflow-hidden rounded-lg border p-1.5 text-left transition-colors duration-fast"
		@click.stop="open"
	>
		<span
			v-if="preview.image"
			class="bg-surface-hover text-text-disabled grid shrink-0 place-items-center overflow-hidden rounded-md"
			:style="{ width: `${THUMB_WIDTH}px`, height: `${THUMB_HEIGHT}px` }"
		>
			<!-- No `alt` of its own: the button already names the destination, and an
			     Open Graph image is decoration for a title that is right beside it. -->
			<img
				v-if="imageUrl"
				:src="imageUrl"
				alt=""
				class="size-full object-cover"
				draggable="false"
			/>
			<IconLucideLink v-else class="size-4" aria-hidden="true" focusable="false" />
		</span>

		<!-- `min-w-0` on the column and `truncate`/`line-clamp` on the lines: a page
		     title is a long unbreakable string often enough that without it the card
		     would widen the note and the panel would scroll horizontally, which it
		     must never do. -->
		<span class="flex min-w-0 flex-1 flex-col justify-center gap-0.5">
			<span v-if="preview.siteName" class="text-text-secondary block truncate text-meta">
				{{ preview.siteName }}
			</span>
			<span v-if="preview.title" class="text-text-primary block truncate text-meta font-medium">
				{{ preview.title }}
			</span>
			<span
				v-if="preview.description"
				class="text-text-secondary block text-meta line-clamp-2 text-pretty"
			>
				{{ preview.description }}
			</span>
		</span>
	</button>
</template>
