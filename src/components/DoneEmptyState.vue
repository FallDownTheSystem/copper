<script setup lang="ts">
/**
 * The done view with nothing in it — the filter's counterpart to
 * `SearchEmptyState`, and mounted for the same reason: the list renders no
 * section at all when nothing survives the filter, so without this the panel
 * would simply go blank and look broken rather than answered.
 */
const { toggleDoneFilter } = useNoteList()
const { hasQuery } = useNoteSearch()
</script>

<template>
	<div class="px-3 pt-4">
		<!-- Two sentences, because two different things can be true: nothing is done
		     at all, or nothing that matches the query is. Saying only the first would
		     be wrong in the second case and would send the user looking for notes the
		     search is hiding. -->
		<p class="text-text-primary text-body font-semibold">
			{{ hasQuery ? 'No done notes match your search.' : 'Nothing is done yet.' }}
		</p>
		<button type="button" class="panel-button mt-2 min-h-6" @click="toggleDoneFilter">
			Show all notes
		</button>
	</div>
</template>
