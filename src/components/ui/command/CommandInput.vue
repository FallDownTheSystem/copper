<script setup lang="ts">
/**
 * The filter field, over `ListboxFilter`.
 *
 * It carries `panel-field` — and so the one `focus-ring` — because it is the
 * only focusable control in the palette: `ListboxFilter` sets the root's
 * `focusable` to false while it is mounted, which is what takes the list and
 * every row out of the tab order and off the focus ring entirely. The rows wear
 * a highlighted background instead, which is the idiom commit c94e646 settled
 * for menus: a ring on every arrow-key step through a list would be noise.
 *
 * No IME guard of its own, unlike `SectionSwitcher`'s hand-written one: reka
 * routes this through `useComposing` already and declines navigation and Enter
 * while a candidate window is open.
 */
import type { ListboxFilterEmits, ListboxFilterProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { ListboxFilter, useForwardPropsEmits } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = defineProps<ListboxFilterProps & { class?: HTMLAttributes['class'] }>()
const emits = defineEmits<ListboxFilterEmits>()

const delegatedProps = reactiveOmit(props, 'class')
const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
	<ListboxFilter
		data-slot="command-input"
		autocomplete="off"
		v-bind="forwarded"
		:class="cn('panel-field h-8 w-full min-w-0 px-2', props.class)"
	/>
</template>
