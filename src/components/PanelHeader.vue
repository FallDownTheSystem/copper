<script setup lang="ts">
defineProps<{
	boundary: HTMLElement | null
	portalTo: HTMLElement | null
}>()

/**
 * The field is real, labelled and focusable, and nothing consumes `query` yet —
 * filtering is Phase 5. That is the intended interim state: the layout needs the
 * region, and a fake field would have to be replaced rather than wired up. The
 * clear button belongs with filtering.
 */
const query = ref('')

const input = useTemplateRef<HTMLInputElement>('input')

function focusSearch() {
	input.value?.focus()
	input.value?.select()
}

defineExpose({ focusSearch, query })
</script>

<template>
	<!-- The drag region is the header's empty area only, never the field or the
	     button: a drag region swallows the pointer events of anything under it. -->
	<header
		data-tauri-drag-region
		class="border-separator flex min-h-12 items-center gap-2 border-b px-3 py-2"
	>
		<label for="panel-search" class="sr-only">Search notes</label>
		<div class="relative min-w-0 flex-1">
			<IconLucideSearch
				class="text-text-disabled pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2"
				aria-hidden="true"
				focusable="false"
			/>
			<input
				id="panel-search"
				ref="input"
				v-model="query"
				type="search"
				name="search"
				autocomplete="off"
				placeholder="Search notes…"
				class="border-separator bg-surface-hover text-text-primary placeholder:text-text-disabled outline-focus-ring h-8 w-full min-w-0 select-text rounded-md border pr-2 pl-8 text-body focus-visible:outline-2 focus-visible:-outline-offset-1"
			/>
		</div>

		<PanelMenu :boundary="boundary" :portal-to="portalTo" />
	</header>
</template>
