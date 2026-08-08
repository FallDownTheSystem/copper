<script setup lang="ts">
import type { ContextMenuContentEmits, ContextMenuContentProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { ContextMenuContent, ContextMenuPortal, useForwardPropsEmits } from 'reka-ui'
import { cn } from '@/lib/utils'

defineOptions({
	inheritAttrs: false,
})

// `to` is the one addition to the generated component, matching the change
// task-004 made to `DropdownMenuContent`. Reka teleports to document.body by
// default, which lands outside the panel root's `overflow: hidden`, outside its
// rounded rect and outside its contextmenu policy.
//
// `max-w-[50vw]` is the second, and it belongs here rather than at the call
// sites: a context menu opens *at the pointer*, so unlike the `...` menu — which
// is anchored to a button in the header and flips against the same boundary every
// time — it can be asked to appear anywhere, including a few pixels from the
// right edge. Collision handling shifts it back inside, but only as far as its own
// width allows; a menu wider than the space left over is clipped by the panel's
// rounded overflow instead, which is what cut shortcut labels off mid-word. Half
// the window is the rule, and the webview *is* the window, so `vw` states it
// directly. The `w-*` at each call site stays the preferred width and this is only
// the ceiling it cannot pass.
const props = defineProps<
	ContextMenuContentProps & { class?: HTMLAttributes['class']; to?: string | HTMLElement }
>()
const emits = defineEmits<ContextMenuContentEmits>()

const delegatedProps = reactiveOmit(props, 'class', 'to')

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
	<ContextMenuPortal :to="to">
		<ContextMenuContent
			data-slot="context-menu-content"
			v-bind="{ ...$attrs, ...forwarded }"
			:class="
				cn(
					'squircle data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 bg-popover text-popover-foreground min-w-36 max-w-[50vw] rounded-lg p-1 shadow-md ring-1 dark:shadow-black/40 duration-base ease-out-quint z-50 max-h-(--reka-context-menu-content-available-height) origin-(--reka-context-menu-content-transform-origin) overflow-x-hidden overflow-y-auto',
					props.class,
				)
			"
		>
			<slot />
		</ContextMenuContent>
	</ContextMenuPortal>
</template>
