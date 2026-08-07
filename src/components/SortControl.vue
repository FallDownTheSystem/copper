<script setup lang="ts">
/**
 * The list's order, as one control for the whole document.
 *
 * **It sits in the header rather than in the section menu**, which is the other
 * half of the sort becoming document-wide. A per-section setting belonged in the
 * menu you open on a section; a document-wide one has no section to be opened
 * from, and a menu you have to open to read the state is a state you cannot see —
 * the same objection the pin answers by being a control rather than an entry.
 * That visibility is what retired `SectionHeader`'s sort marker: the mode is on
 * screen already, so a per-section badge would be the same fact repeated once per
 * section.
 *
 * **A cycle rather than a menu**, matching the toggle beside it. Three states in
 * a fixed order is a press to reach any of them and no overlay to open, and the
 * button reads out the one in effect rather than describing what a press will do
 * — which is what makes `title` load-bearing here and worth keeping in step.
 */
const { sortMode, isSorted, setSort } = useNoteList()

/** Manual leads: it is the document's own order, and the one every drag and
 *  Alt+Arrow writes. Going round the three ends where it started. */
const NEXT = {
	manual: 'oldest',
	oldest: 'newest',
	newest: 'manual',
} as const satisfies Record<SortMode, SortMode>

/**
 * The label is the *state*, not the action, and it is blank on Manual.
 *
 * Manual is the document's own order and the one most lists are in, so naming it
 * would spend width on every panel forever to say "nothing is happening". A
 * computed order is the exceptional one and is worth a word; the icon alone is
 * what remains when there is nothing exceptional to report.
 */
const LABELS = { manual: '', oldest: 'Oldest', newest: 'Newest' } as const satisfies Record<
	SortMode,
	string
>

/** Named as the list will read rather than as the field it sorts by — "Oldest
 *  first" says what the top of the list will be, which is the thing being
 *  chosen. */
const DESCRIPTIONS = {
	manual: 'Manual order',
	oldest: 'Oldest first',
	newest: 'Newest first',
} as const satisfies Record<SortMode, string>

const label = computed(() => LABELS[sortMode.value])

/** The state, then what the press does with it. The accessible name is the same
 *  sentence: on Manual there is no visible text at all, and even sorted a bare
 *  "Newest" would not say it is a sort. */
const title = computed(
	() =>
		`${DESCRIPTIONS[sortMode.value]} · press for ${DESCRIPTIONS[NEXT[sortMode.value]].toLowerCase()}`,
)
</script>

<template>
	<button
		type="button"
		data-sort-mode
		class="panel-button inline-flex min-h-6 shrink-0 items-center gap-1 px-1.5"
		:class="isSorted ? 'text-accent-text' : 'text-text-secondary'"
		:title="title"
		:aria-label="title"
		@click="setSort(NEXT[sortMode])"
	>
		<IconLucideArrowDownUp class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
		<span v-if="label" class="text-label uppercase">{{ label }}</span>
	</button>
</template>
