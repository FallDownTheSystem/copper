<script setup lang="ts">
/**
 * One labelled group of results, over `ListboxGroup` — which supplies the
 * `role="group"` and the `aria-labelledby` pointing at the label below it.
 *
 * Grouping is presentational only. Reka's collection spans groups, so the
 * highlight walks from the last row of one straight into the first of the next
 * and Enter resolves whichever row it landed on, with no per-group state.
 */
import type { ListboxGroupProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { ListboxGroup, useForwardProps } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = defineProps<ListboxGroupProps & { class?: HTMLAttributes['class'] }>()

const delegatedProps = reactiveOmit(props, 'class')
const forwardedProps = useForwardProps(delegatedProps)
</script>

<template>
	<ListboxGroup data-slot="command-group" v-bind="forwardedProps" :class="cn(props.class)">
		<slot />
	</ListboxGroup>
</template>

<style scoped>
/**
 * The rule between two groups, and adjacency is the whole reason it is CSS.
 *
 * Every group in the palette is `v-if`-gated on having results, so which of them
 * renders — and therefore which one is *first* — changes with every keystroke. A
 * divider drawn from the template would have to answer "is there a visible group
 * above me", which no group can know about its siblings; `+` is exactly that
 * question, asked by the one thing that can see both.
 *
 * The rule spans the group rather than bleeding to the popover's edges, which
 * lines it up with the highlight behind a focused row — the only other full-width
 * mark in the list. Decorative and nothing more: the groups already carry
 * `role="group"` and their labels, so a separator with a role would be announcing
 * a boundary the reader has just been told about.
 */
[data-slot='command-group'] + [data-slot='command-group'] {
	margin-top: 4px;
	border-top: 1px solid var(--separator);
	padding-top: 4px;
}
</style>
