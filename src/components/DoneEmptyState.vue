<script setup lang="ts">
/**
 * The done filter with nothing left on screen — the filter's counterpart to
 * `SearchEmptyState`, and mounted for the same reason: the list renders no
 * section at all when nothing survives, so without this the panel would go blank
 * and look broken rather than answered.
 *
 * **Both narrowing states land here, and they are opposite emptinesses.** The
 * done view is empty when nothing has been finished; the default view is empty
 * when everything has. Naming the wrong one would tell the reader the precise
 * opposite of what is true, which is worse than saying nothing.
 */
const { doneFilter, nextDoneFilter, cycleDoneFilter } = useNoteList()
const { hasQuery } = useNoteSearch()

/** Two sentences per state, because two different things can be true: nothing is
 *  in this half of the document at all, or nothing that matches the query is.
 *  Saying only the first would be wrong in the second case and would send the
 *  user looking for notes the search is hiding. */
const HEADINGS = {
	done: {
		plain: 'Nothing is done yet.',
		searching: 'No done notes match your search.',
	},
	todo: {
		plain: 'Everything here is done.',
		searching: 'No unfinished notes match your search.',
	},
} as const

/** `all` never reaches this component — it hides nothing, so an empty list under
 *  it is an empty space and `PanelShell` owns that. Falling back to the done copy
 *  keeps the type total without inventing a third sentence nobody can see. */
const heading = computed(() => {
	const state = doneFilter.value === 'todo' ? HEADINGS.todo : HEADINGS.done
	return hasQuery.value ? state.searching : state.plain
})

/** The same cycle the header button walks, so the way out of an empty view is
 *  the press it would have taken anyway rather than a second, private path. */
const ACTIONS = {
	todo: 'Hide done notes',
	done: 'Show done notes',
	all: 'Show all notes',
} as const

const actionLabel = computed(() => ACTIONS[nextDoneFilter.value])
</script>

<template>
	<div class="px-3 pt-4">
		<p class="text-text-primary text-body font-semibold">{{ heading }}</p>
		<button type="button" class="panel-button mt-2 min-h-6" @click="cycleDoneFilter">
			{{ actionLabel }}
		</button>
	</div>
</template>
