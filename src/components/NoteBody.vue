<script setup lang="ts">
import { openUrl } from '@tauri-apps/plugin-opener'
import type { Note } from '@/composables/useSpace'

const props = defineProps<{ note: Note }>()

const { renderNote } = useMarkdown()
const { canExpand, isExpanded, measure, toggle, clampHeight } = useNoteDisclosure()

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

let observer: ResizeObserver | null = null

function remeasure() {
	const element = contentRef.value
	if (!element || clampHeight.value <= 0) return
	// Writes state only, never style, in the same synchronous block.
	measure(props.note.id, element.getBoundingClientRect().height, clampHeight.value)
}

onMounted(() => {
	if (typeof ResizeObserver === 'undefined') return
	observer = new ResizeObserver(remeasure)
	if (contentRef.value) observer.observe(contentRef.value)
})

onBeforeUnmount(() => {
	observer?.disconnect()
	observer = null
})

// The probe resolves after the first paint, and the rendered HTML changes when
// the highlighter swaps in — both change the answer.
watch([clampHeight, html], () => void nextTick(remeasure))

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
			<div ref="content" class="note-prose select-text" v-html="html" @click="onClick" />
		</div>

		<button
			v-if="expandable"
			type="button"
			tabindex="-1"
			:aria-expanded="expanded"
			:aria-controls="bodyId"
			class="text-text-secondary hover:bg-surface-hover active:bg-surface-active mt-1 rounded-md px-1.5 py-0.5 text-meta transition-colors duration-fast"
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
