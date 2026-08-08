<script setup lang="ts">
import type { DropdownMenuItemProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { DropdownMenuItem, useForwardProps } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = withDefaults(
	defineProps<
		DropdownMenuItemProps & {
			class?: HTMLAttributes['class']
			inset?: boolean
			variant?: 'default' | 'destructive'
		}
	>(),
	{
		variant: 'default',
	},
)

const delegatedProps = reactiveOmit(props, 'inset', 'variant', 'class')

const forwardedProps = useForwardProps(delegatedProps)
</script>

<template>
	<!-- `data-[highlighted]:` twins beside every `focus:` rule, because the two
	     states are not the same set: reka moves focus onto a hovered item only
	     when nothing else inside the content holds it, and the section switcher's
	     filter field holds it the whole time — there reka *highlights* the hovered
	     row without focusing it, and `focus:` alone left that hover invisible. A
	     focused item carries `data-highlighted` as well, so the twins repaint
	     nothing in menus where focus does move. -->
	<DropdownMenuItem
		data-slot="dropdown-menu-item"
		:data-inset="inset ? '' : undefined"
		:data-variant="variant"
		v-bind="forwardedProps"
		:class="
			cn(
				'focus:bg-accent focus:text-accent-foreground data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground data-[variant=destructive]:text-destructive data-[variant=destructive]:focus:bg-destructive/10 data-[variant=destructive]:data-[highlighted]:bg-destructive/10 dark:data-[variant=destructive]:focus:bg-destructive/20 dark:data-[variant=destructive]:data-[highlighted]:bg-destructive/20 data-[variant=destructive]:focus:text-destructive data-[variant=destructive]:data-[highlighted]:text-destructive data-[variant=destructive]:*:[svg]:text-destructive not-data-[variant=destructive]:focus:**:text-accent-foreground not-data-[variant=destructive]:data-[highlighted]:**:text-accent-foreground gap-2 rounded-md px-2 py-1.5 text-meta data-inset:pl-8 [&_svg:not([class*=size-])]:size-4 group/dropdown-menu-item relative flex cursor-default items-center outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0',
				props.class,
			)
		"
	>
		<slot />
	</DropdownMenuItem>
</template>
