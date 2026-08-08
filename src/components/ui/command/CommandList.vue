<script setup lang="ts">
/**
 * The scrolling result region, over `ListboxContent` — which is the element that
 * carries `role="listbox"` and owns Up/Down/Home/End, Enter and typeahead.
 *
 * Capped and scrolled internally for the reason every list in this panel is: the
 * window is a fixed 440 × 760 and nothing outside `main` may make the document
 * scroll. `thin-scrollbar` keeps its reserved gutter here, unlike `main`, since
 * the rows are left-aligned labels rather than a text column whose centring the
 * reader would notice shifting.
 */
import type { ListboxContentProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { ListboxContent, useForwardProps } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = defineProps<ListboxContentProps & { class?: HTMLAttributes['class'] }>()

const delegatedProps = reactiveOmit(props, 'class')
const forwardedProps = useForwardProps(delegatedProps)
</script>

<template>
	<ListboxContent
		data-slot="command-list"
		v-bind="forwardedProps"
		:class="
			cn('thin-scrollbar max-h-72 min-h-0 overflow-y-auto overflow-x-hidden p-1', props.class)
		"
	>
		<slot />
	</ListboxContent>
</template>
