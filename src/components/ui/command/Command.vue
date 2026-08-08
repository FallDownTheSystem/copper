<script setup lang="ts">
/**
 * The command palette's list root, over reka's `Listbox`.
 *
 * **Reimplemented rather than vendored.** shadcn-vue's `Command` is ten files
 * that declare `dialog` and `input-group` as registry dependencies — two
 * component sets this project does not have — and the half of it that is not a
 * thin reka wrapper is a filtering engine built on reka's substring `useFilter`,
 * scoring every item 0 or 1 by scraping its rendered `textContent` at mount.
 * Task-019 requires `fuzzyMatch` and requires the results *ranked*, so that
 * engine would have been rewritten on arrival. Filtering three plain arrays in a
 * `computed` and rendering them with `v-for` is both smaller and correct by
 * construction — and it makes "hide an empty group" a `v-if` rather than a
 * registration protocol.
 *
 * `Listbox` supplies the parts that are genuinely worth not hand-writing: the
 * roving highlight across groups, Up/Down/Home/End, Enter on the highlighted
 * row, and a filter field that is IME-safe and drives `aria-activedescendant`.
 * It is also the ARIA-correct surface for "filter a list", which is what lets
 * the palette avoid the knowing `aria-required-children` violation the section
 * switcher documents — a textbox may not live inside a `role="menu"`, and it may
 * live beside a `role="listbox"`.
 */
import type { ListboxRootEmits, ListboxRootProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { ListboxRoot, useForwardPropsEmits } from 'reka-ui'
import { useTemplateRef } from 'vue'
import { cn } from '@/lib/utils'

const props = defineProps<ListboxRootProps & { class?: HTMLAttributes['class'] }>()
const emits = defineEmits<ListboxRootEmits>()

const delegatedProps = reactiveOmit(props, 'class')
const forwarded = useForwardPropsEmits(delegatedProps, emits)

/**
 * The one thing reka does not do on its own that a palette needs.
 *
 * `ListboxFilter` highlights the first row on every keystroke, so the highlight
 * follows a narrowing query — but nothing highlights anything before the first
 * one, and a palette whose `Enter` does nothing until you have typed a character
 * is a palette that looks broken on opening. The caller asks for it once, on
 * open, rather than this component reaching for a lifecycle it does not own: it
 * is rendered by a `v-if` on the overlay, so "mounted" and "opened" are the same
 * moment for the caller and not for anything in here.
 *
 * Structurally typed rather than `InstanceType<typeof ListboxRoot>`: the root is
 * a generic component, and naming its instance type would pin the item value
 * type here for no reader.
 */
const root = useTemplateRef<{ highlightFirstItem: () => void }>('root')

defineExpose({
	highlightFirstItem: () => root.value?.highlightFirstItem(),
})
</script>

<template>
	<ListboxRoot
		ref="root"
		data-slot="command"
		v-bind="forwarded"
		:class="cn('flex min-h-0 w-full flex-col overflow-hidden', props.class)"
	>
		<slot />
	</ListboxRoot>
</template>
