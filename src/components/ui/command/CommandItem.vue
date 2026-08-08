<script setup lang="ts">
/**
 * One result row, over `ListboxItem`.
 *
 * **Keyed off `data-highlighted`, not `focus:`.** `DropdownMenuItem` styles the
 * focused state because reka moves real DOM focus onto a menu item; here the
 * filter field keeps focus for the whole life of the palette and the highlight
 * is an attribute reka writes instead. The treatment is the same one either way
 * — the highlighted-row background from commit c94e646, with no focus ring of
 * its own, because a ring on every arrow-key step through a list is noise.
 */
import type { ListboxItemEmits, ListboxItemProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { ListboxItem, useForwardPropsEmits } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = defineProps<ListboxItemProps & { class?: HTMLAttributes['class'] }>()
const emits = defineEmits<ListboxItemEmits>()

const delegatedProps = reactiveOmit(props, 'class')
const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
	<ListboxItem
		data-slot="command-item"
		v-bind="forwarded"
		:class="
			cn(
				'data-highlighted:bg-accent data-highlighted:text-accent-foreground relative flex cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-meta outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*=size-])]:size-4',
				props.class,
			)
		"
	>
		<slot />
	</ListboxItem>
</template>
