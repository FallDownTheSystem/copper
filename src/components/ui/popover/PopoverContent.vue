<script setup lang="ts">
import type { PopoverContentEmits, PopoverContentProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { PopoverContent, PopoverPortal, useForwardPropsEmits } from 'reka-ui'
import { cn } from '@/lib/utils'

defineOptions({
	inheritAttrs: false,
})

// `to` is the one addition to the generated component, for the reason
// `DropdownMenuContent` records: reka teleports to document.body by default,
// which lands outside the panel root's `overflow: hidden`, outside its rounded
// rect and outside its contextmenu policy. The panel passes its own in-clip
// portal host.
const props = withDefaults(
	defineProps<
		PopoverContentProps & { class?: HTMLAttributes['class']; to?: string | HTMLElement }
	>(),
	{
		align: 'center',
		sideOffset: 4,
	},
)
const emits = defineEmits<PopoverContentEmits>()

const delegatedProps = reactiveOmit(props, 'class', 'to')

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
	<PopoverPortal :to="to">
		<PopoverContent
			data-slot="popover-content"
			v-bind="{ ...$attrs, ...forwarded }"
			:class="
				cn(
					'squircle data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 bg-popover text-popover-foreground rounded-lg p-3 shadow-md ring-1 dark:shadow-black/40 duration-base ease-out-quint z-50 w-72 origin-(--reka-popover-content-transform-origin) outline-none',
					props.class,
				)
			"
		>
			<slot />
		</PopoverContent>
	</PopoverPortal>
</template>
