<script setup lang="ts">
import type { DropdownMenuSubContentEmits, DropdownMenuSubContentProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { ContextMenuSubContent, useForwardPropsEmits } from 'reka-ui'
import { cn } from '@/lib/utils'

// `max-w-[50vw]` for the reason `ContextMenuContent` records, and it needs saying
// twice: a submenu opens beside a parent that is already at the edge, so it is the
// panel of the pair most likely to be shifted or flipped, and `Move to ▸` carries
// section names — user text of any length.
const props = defineProps<DropdownMenuSubContentProps & { class?: HTMLAttributes['class'] }>()
const emits = defineEmits<DropdownMenuSubContentEmits>()

const delegatedProps = reactiveOmit(props, 'class')

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
	<ContextMenuSubContent
		data-slot="context-menu-sub-content"
		v-bind="forwarded"
		:class="
			cn(
				'squircle data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 bg-popover text-popover-foreground min-w-32 max-w-[50vw] rounded-lg p-1 shadow-md ring-1 dark:shadow-black/40 duration-base ease-out-quint z-50 origin-(--reka-context-menu-content-transform-origin) overflow-hidden',
				props.class,
			)
		"
	>
		<slot />
	</ContextMenuSubContent>
</template>
