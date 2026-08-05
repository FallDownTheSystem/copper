<script setup lang="ts">
const { activeQuery, clearQuery } = useNoteSearch()

function clearAndFocus() {
	clearQuery()
	// Focus returns to the field the query was typed in, not to the list — the
	// user's next act after clearing a fruitless search is almost always another
	// search.
	void nextTick(() => document.querySelector<HTMLInputElement>('[data-search]')?.focus())
}
</script>

<template>
	<div class="px-3 pt-4">
		<p class="text-text-primary text-body font-semibold">No notes match “{{ activeQuery }}”.</p>
		<!-- min-h-6 plus the padding keeps this over the 24px hit-area floor for
		     dense UI; nothing sits close enough to it to overlap. -->
		<button type="button" class="panel-button mt-2 min-h-6" @click="clearAndFocus">
			Clear search
		</button>
	</div>
</template>
