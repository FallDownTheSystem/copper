<script setup lang="ts">
import { openUrl } from '@tauri-apps/plugin-opener'
import { applyHighlight, releaseHighlight } from '@/lib/searchHighlight'
import type { Note } from '@/composables/useSpace'

const props = defineProps<{ note: Note }>()

const { renderNote, noteLinks } = useMarkdown()
const { canExpand, isExpanded, measure, toggle, clampHeight } = useNoteDisclosure()
const { matchNeedle } = useNoteSearch()
const { previewFor, requestPreview, previewEpoch, enabled: previewsEnabled } = usePreviews()

const html = computed(() => renderNote(props.note))
const bodyId = computed(() => `note-body-${props.note.id}`)
const expanded = computed(() => isExpanded(props.note.id))
const expandable = computed(() => canExpand(props.note.id))

/** The measurement runs against this *unconstrained* element rather than the
 *  clamped box. Comparing the clamped element's scrollHeight to its clientHeight
 *  works exactly until it succeeds: once expanded the two are equal, the
 *  disclosure decides the note no longer overflows, and `Show less` removes
 *  itself with no way back. */
const contentRef = useTemplateRef<HTMLElement>('content')

function remeasure() {
	const element = contentRef.value
	if (!element || clampHeight.value <= 0) return
	// Writes state only, never style, in the same synchronous block.
	measure(props.note.id, element.getBoundingClientRect().height, clampHeight.value)
}

// VueUse owns the lifecycle and the unsupported-environment guard — happy-dom
// and older WebViews do not both provide ResizeObserver.
useResizeObserver(contentRef, remeasure)

// The probe resolves after the first paint, and the rendered HTML changes when
// the highlighter swaps in — both change the answer.
watch([clampHeight, html], () => void nextTick(remeasure))

/**
 * Search matches are painted as ranges over the live DOM rather than as `<mark>`
 * elements, so the cached HTML string this body was rendered from stays
 * byte-identical across a search. It runs after the patch because the ranges
 * address text nodes the render has just replaced.
 */
watch(
	[html, matchNeedle],
	() => void nextTick(() => applyHighlight(contentRef.value, matchNeedle.value)),
	{ immediate: true },
)

onBeforeUnmount(() => releaseHighlight(contentRef.value))

// --- link previews -----------------------------------------------------------

/** Taken from the token stream rather than from the rendered DOM, so a preview
 *  can only ever be fetched for a link that survived the render-time scheme
 *  allowlist — and never for one inside a code fence, which looks like a URL and
 *  is not a link. */
const links = computed(() => noteLinks(props.note))

/**
 * The ask, driven from a watcher rather than from the read below, exactly as
 * `AttachmentCard` does it: `previewFor` is consumed by a computed, and
 * requesting as a side effect of reading would write the preview cache during
 * that computed's evaluation.
 *
 * `previewsEnabled` is a dependency and not just a guard — switching the setting
 * on has to make every mounted note ask, and there is no other signal that would
 * reach one. The epoch is the mirror of that: switching it off drops the state
 * under cards that are still mounted, and this is what makes them notice.
 */
watch(
	() => [links.value, previewsEnabled.value, previewEpoch.value] as const,
	([hrefs, on]) => {
		if (!on) return
		for (const href of hrefs) requestPreview(href)
	},
	{ immediate: true },
)

/**
 * Only the links that have something to show.
 *
 * Filtering here rather than letting each card decide is what keeps the list out
 * of the DOM entirely when nothing resolved — a `<ul>` with a top margin and no
 * visible children would add a gap under every note containing a link, which is
 * most of them.
 */
const cards = computed(() => links.value.filter((href) => previewFor(href).state === 'ready'))

/**
 * Defence in depth only. The scheme allowlist is enforced at render time, so an
 * unsafe link has no `href` for this selector to match — which is the point:
 * a click handler covers neither middle-click, the anchor context menu,
 * drag-and-drop, nor native navigation.
 */
function onClick(event: MouseEvent) {
	const anchor = (event.target as HTMLElement | null)?.closest?.('a[href]')
	if (!anchor) return

	event.preventDefault()
	const href = anchor.getAttribute('href')
	if (href) void openUrl(href)
}

/**
 * Middle-click fires `auxclick`, never `click`, so the handler above does not
 * see it and the press falls through to the WebView's own new-window handling.
 * The middle button only: the right button also arrives here, and preventing it
 * would take away the context menu.
 */
function onAuxClick(event: MouseEvent) {
	if (event.button !== 1) return
	onClick(event)
}
</script>

<template>
	<div class="min-w-0">
		<div
			:id="bodyId"
			class="min-w-0"
			:class="expandable && !expanded ? 'note-clamped' : ''"
			:style="expandable && !expanded ? { maxHeight: 'var(--note-clamp)' } : undefined"
		>
			<!-- eslint-disable-next-line vue/no-v-html -- markdown-it runs with
			     html:false and every link scheme is filtered at render time; see
			     useMarkdown for why that boundary is at render time and not here. -->
			<div
				ref="content"
				class="note-prose select-text"
				v-html="html"
				@click="onClick"
				@auxclick="onAuxClick"
			/>
		</div>

		<!-- **Outside the clamp, above `Show more`.** Inside it the cards would be
		     the first thing the clamp hid, so a note long enough to be worth
		     collapsing would be exactly the note whose previews never appeared. They
		     sit above the disclosure button because they belong to the body rather
		     than to the control that reveals the rest of it.

		     Hidden while the inline editor is open without needing to say so:
		     `NoteCard` renders `NoteEditor` *instead of* this component. -->
		<ul v-if="cards.length > 0" class="mt-1.5 flex min-w-0 flex-col gap-1">
			<li v-for="href in cards" :key="href" class="min-w-0">
				<LinkPreviewCard :url="href" />
			</li>
		</ul>

		<button
			v-if="expandable"
			type="button"
			tabindex="-1"
			:aria-expanded="expanded"
			:aria-controls="bodyId"
			class="text-text-secondary hover:bg-surface-hover active:bg-surface-active mt-1 rounded-sm px-1.5 py-0.5 text-meta transition-colors duration-fast"
			@click.stop="toggle(note.id)"
		>
			{{ expanded ? 'Show less' : 'Show more' }}
		</button>
	</div>
</template>

<style scoped>
/* The fade belongs to the clamped state only — never to a scroll container, and
   never as a `max-height` transition on a variable-height Markdown body, which
   is a layout-tier animation on every frame. */
.note-clamped {
	overflow: hidden;
	mask-image: linear-gradient(to bottom, black 80%, transparent 100%);
}
</style>
