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
