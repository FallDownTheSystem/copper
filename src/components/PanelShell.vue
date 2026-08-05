<script setup lang="ts">
const {
	loadState,
	refreshing,
	actionError,
	noteCount,
	spaceName,
	activeSectionObject,
	initialize,
} = useSpace()
const { setClampHeight } = useNoteDisclosure()
const { ensureHighlighter } = useMarkdown()

const root = useTemplateRef<HTMLElement>('root')
const portalHost = useTemplateRef<HTMLElement>('portalHost')
const clampProbe = useTemplateRef<HTMLElement>('clampProbe')
const header = useTemplateRef<{ focusSearch: () => void }>('header')
const composer = useTemplateRef<{ focus: () => void }>('composer')

// Passed down as plain elements rather than refs: reka wants the node.
const boundary = ref<HTMLElement | null>(null)
const portalTo = ref<HTMLElement | null>(null)

const empty = computed(() => loadState.value === 'ready' && noteCount.value === 0)

let probeObserver: ResizeObserver | null = null

onMounted(() => {
	boundary.value = root.value
	portalTo.value = portalHost.value

	// `--note-clamp` is a calc() over other custom properties, and
	// getComputedStyle returns it unevaluated — so it is measured off one real
	// box, once, rather than per card.
	const probe = clampProbe.value
	if (probe) {
		setClampHeight(probe.getBoundingClientRect().height)
		if (typeof ResizeObserver !== 'undefined') {
			probeObserver = new ResizeObserver(() => setClampHeight(probe.getBoundingClientRect().height))
			probeObserver.observe(probe)
		}
	}

	void initialize()
	// Fire and forget: until it resolves, fences render unhighlighted and the
	// panel is fully usable.
	void ensureHighlighter()
})

onBeforeUnmount(() => {
	probeObserver?.disconnect()
	probeObserver = null
})

// Focus the composer only when the empty state actually renders — never during
// loading, which would let the panel steal focus before the space arrives.
watch(empty, (isEmpty) => {
	if (isEmpty) void nextTick(() => composer.value?.focus())
})

function onShellKeydown(event: KeyboardEvent) {
	if (event.key === 'f' && (event.ctrlKey || event.metaKey)) {
		event.preventDefault()
		header.value?.focusSearch()
	}
}

/**
 * The default WebView context menu is suppressed everywhere except the two text
 * fields and rendered note bodies, where Copy/Paste is genuinely useful.
 *
 * Task-006 narrows the `.note-prose` exemption when it adds its own context
 * menu; that is its change to make.
 */
function onContextMenu(event: MouseEvent) {
	const target = event.target as HTMLElement | null
	if (target?.closest('textarea, input, .note-prose')) return
	event.preventDefault()
}
</script>

<template>
	<div
		ref="root"
		class="panel-surface grid h-full min-h-0 w-full grid-rows-[auto_1fr_auto] select-none font-sans text-body"
		@keydown="onShellKeydown"
		@contextmenu="onContextMenu"
	>
		<PanelHeader ref="header" :boundary="boundary" :portal-to="portalTo" />

		<!-- The only scrollable region. `min-h-0` is load-bearing: a grid item
		     defaults to `min-height: auto`, so without it this grows to its content
		     height and the whole document scrolls despite `overflow: hidden` on
		     html/body/#app. `min-w-0` is the horizontal equivalent and a separate
		     failure mode — a wide code fence or Markdown table widens the document
		     without it. -->
		<main
			data-scroll-region
			class="thin-scrollbar min-h-0 min-w-0 overflow-y-auto overscroll-contain"
			:aria-busy="refreshing"
		>
			<h1 class="sr-only">{{ spaceName || 'Copper' }}</h1>

			<PanelStates>
				<div class="pt-2 pb-3">
					<NoteList />

					<!-- Additive, not a replacement: a zero-note space still renders its
					     section headers and the active section's own empty line, because
					     hiding where a capture will land is worst exactly when the list
					     is empty. -->
					<div v-if="empty" class="px-3 pt-4">
						<p class="text-text-primary text-body font-semibold">No notes yet</p>
						<p class="text-text-secondary mt-1 text-meta">
							Add one below. It lands in {{ activeSectionObject?.name ?? 'this space' }}.
						</p>
					</div>

					<EditorRecoveryRow />
				</div>
			</PanelStates>
		</main>

		<Composer ref="composer" />

		<!-- Inside the panel root, so teleported dropdown content stays inside the
		     clip, the rounded rect and the contextmenu policy above. -->
		<div ref="portalHost" class="pointer-events-none absolute inset-0 z-30 empty:hidden">
			<div class="pointer-events-auto contents" />
		</div>

		<!-- Measured, never shown. -->
		<div
			ref="clampProbe"
			aria-hidden="true"
			class="pointer-events-none absolute h-(--note-clamp) w-0"
		/>

		<!-- Pre-rendered and empty. Injecting the element and its text together
		     does not announce; only a text change inside a live region already in
		     the accessibility tree does. -->
		<div class="sr-only" role="alert" aria-live="assertive">{{ actionError }}</div>
		<div class="sr-only" role="status" aria-live="polite">
			{{ refreshing ? 'Refreshing notes' : '' }}
		</div>
	</div>
</template>
