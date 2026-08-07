<script setup lang="ts">
/**
 * The query lives in `useNoteSearch` at module scope, not here. A ref held
 * inside this component cannot be read by `NoteList` — the same private-copy
 * trap task-004 warns about, one level up.
 */
const { query, hasQuery, clearQuery } = useNoteSearch()

const input = useTemplateRef<HTMLInputElement>('input')

function focusSearch() {
	input.value?.focus()
	input.value?.select()
}

/** One rung of the Escape ladder, handled where the focus is. The press is
 *  consumed only when there is a query to clear, so Escape in an empty field
 *  still falls through to the levels below it. */
function onKeydown(event: KeyboardEvent) {
	if (event.key !== 'Escape' || !hasQuery.value) return
	event.preventDefault()
	event.stopPropagation()
	clearQuery()
}

defineExpose({ focusSearch, query })
</script>

<template>
	<!-- The drag region is the header's empty area and the mark, never the field
	     or the button: a drag region swallows the pointer events of anything under
	     it. -->
	<header
		data-tauri-drag-region
		class="border-separator flex min-h-12 items-center gap-2 border-b px-3 py-2"
	>
		<!-- Copper's mark, and the header's dependable grab handle: the field and
		     the menu button leave the header almost no bare area, so the drag region
		     on the header itself is a strip a few pixels wide in practice.

		     The glyph is this element's own text rather than a child span, because
		     a child element receives the mousedown and `data-tauri-drag-region` is
		     read off the element that does. Branding rather than a control — no
		     hover state, no tab stop, nothing to activate — so it is a `div` and
		     `aria-hidden`: a lone decorative `c` announced to a screen reader is
		     noise. -->
		<div
			data-tauri-drag-region
			aria-hidden="true"
			class="text-accent-text grid size-8 shrink-0 cursor-grab place-items-center text-body font-semibold select-none active:cursor-grabbing"
		>
			c
		</div>

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
				data-search
				type="search"
				name="search"
				autocomplete="off"
				placeholder="Search notes…"
				class="panel-field h-8 w-full min-w-0 pr-2 pl-8"
				@keydown="onKeydown"
			/>
		</div>

		<PanelMenu />
	</header>
</template>
