<script setup lang="ts">
import type { ContextMenuItemEmits, ContextMenuItemProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { ContextMenuItem, useForwardPropsEmits } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = withDefaults(
	defineProps<
		ContextMenuItemProps & {
			class?: HTMLAttributes['class']
			inset?: boolean
			variant?: 'default' | 'destructive'
		}
	>(),
	{
		variant: 'default',
	},
)
const emits = defineEmits<ContextMenuItemEmits>()

const delegatedProps = reactiveOmit(props, 'class')

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
	<ContextMenuItem
		data-slot="context-menu-item"
		:data-inset="inset ? '' : undefined"
		:data-variant="variant"
		v-bind="forwarded"
		:class="
			cn(
				// `min-w-0` so a label that cannot fit the 50vw ceiling wraps inside the
				// row instead of widening it past the menu it lives in. The label is a
				// bare text node at every call site — an anonymous flex item, which is
				// why this is the only handle there is.
				'focus:bg-accent focus:text-accent-foreground data-[variant=destructive]:text-destructive data-[variant=destructive]:focus:bg-destructive/10 dark:data-[variant=destructive]:focus:bg-destructive/20 data-[variant=destructive]:focus:text-destructive data-[variant=destructive]:*:[svg]:text-destructive focus:*:[svg]:text-accent-foreground gap-2 rounded-md px-2 py-1.5 text-meta data-inset:pl-8 [&_svg:not([class*=size-])]:size-4 group/context-menu-item relative flex min-w-0 cursor-default items-center outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0',
				props.class,
			)
		"
	>
		<slot />
	</ContextMenuItem>
</template>
