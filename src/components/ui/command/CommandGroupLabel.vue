<script setup lang="ts">
/**
 * A group's heading, over `ListboxGroupLabel` — which carries the id the group's
 * `aria-labelledby` names, so this is what makes the group announce itself.
 *
 * Same appearance as `DropdownMenuLabel`: the palette and the `...` menu are two
 * lists of the same kind of thing and a second heading treatment would only make
 * them look unrelated.
 */
import type { ListboxGroupLabelProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { ListboxGroupLabel, useForwardProps } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = defineProps<ListboxGroupLabelProps & { class?: HTMLAttributes['class'] }>()

const delegatedProps = reactiveOmit(props, 'class')
const forwardedProps = useForwardProps(delegatedProps)
</script>

<template>
	<ListboxGroupLabel
		data-slot="command-group-label"
		v-bind="forwardedProps"
		:class="cn('text-text-secondary px-2 py-1.5 text-label font-medium', props.class)"
	>
		<slot />
	</ListboxGroupLabel>
</template>
