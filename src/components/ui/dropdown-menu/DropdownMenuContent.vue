<script setup lang="ts">
import type { DropdownMenuContentEmits, DropdownMenuContentProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { DropdownMenuContent, DropdownMenuPortal, useForwardPropsEmits } from 'reka-ui'
import { cn } from '@/lib/utils'

defineOptions({
	inheritAttrs: false,
})

// `to` is the one addition to the generated component. Reka teleports to
// document.body by default, which lands outside the panel root's
// `overflow: hidden`, outside its rounded rect and outside its contextmenu
// policy — over a transparent region with no surface behind it. The panel
// passes its own in-clip portal host.
const props = withDefaults(
	defineProps<
		DropdownMenuContentProps & { class?: HTMLAttributes['class']; to?: string | HTMLElement }
	>(),
	{
		align: 'start',
		sideOffset: 4,
	},
)
const emits = defineEmits<DropdownMenuContentEmits>()

const delegatedProps = reactiveOmit(props, 'class', 'to')

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
	<DropdownMenuPortal :to="to">
		<DropdownMenuContent
			data-slot="dropdown-menu-content"
			v-bind="{ ...$attrs, ...forwarded }"
			:class="
				cn(
					'squircle data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 bg-popover text-popover-foreground min-w-32 rounded-lg p-1 shadow-md ring-1 dark:shadow-black/40 duration-100 z-50 max-h-(--reka-dropdown-menu-content-available-height) w-(--reka-dropdown-menu-trigger-width) origin-(--reka-dropdown-menu-content-transform-origin) overflow-x-hidden overflow-y-auto data-[state=closed]:overflow-hidden',
					props.class,
				)
			"
		>
			<slot />
		</DropdownMenuContent>
	</DropdownMenuPortal>
</template>
