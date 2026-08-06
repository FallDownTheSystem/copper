<script setup lang="ts">
/**
 * The "this is the active one" cue, in both the forms it has to take.
 *
 * A coloured dot before the label, and the same distinction in words after it.
 * The words are not decoration: colour alone would carry the whole difference
 * between the active row and the rest, so a reader who does not see the accent
 * would get nothing at all.
 *
 * The dot occupies its slot whether or not the row is active — hidden, not
 * absent — so marking a row shifts no text beside it.
 *
 * The label goes in the slot rather than in a prop because each of the three
 * surfaces styles it differently, and it sits *between* the two halves — a
 * component that rendered only the dot would leave the sr-only text to be
 * hand-written a third time, which is the duplication this exists to remove.
 * Attributes are forwarded to the dot: spacing and transitions are the row's
 * business, not this component's.
 */
defineOptions({ inheritAttrs: false })

defineProps<{
	active: boolean
	/** What the dot means here — `active section`, `active space`. Rendered in
	 *  parentheses, so it reads as an aside rather than as part of the name. */
	label: string
}>()
</script>

<template>
	<span
		v-bind="$attrs"
		aria-hidden="true"
		class="bg-accent-ring size-1.5 shrink-0 rounded-full"
		:class="active ? 'opacity-100' : 'opacity-0'"
	/>
	<slot />
	<span v-if="active" class="sr-only">({{ label }})</span>
</template>
