<script setup lang="ts" generic="T extends string">
/**
 * A two-or-three-value preference as a segmented control with text labels.
 *
 * **`RadioGroup`, not `ToggleGroup`**, for the reason `ThemeToggle` records at
 * length: `ToggleGroupRoot` renders `role="group"` unconditionally and each item
 * renders `<button aria-pressed>`, so a screen-reader user hears N independent
 * toggles rather than one choice. `RadioGroupRoot` gives `role="radiogroup"` /
 * `role="radio"` / `aria-checked` with the same roving tabindex, the same single
 * Tab stop and the same arrow-key selection.
 *
 * Generic rather than one component per setting, and text rather than icons:
 * neither `top`/`bottom` nor `copy`/`edit` has a glyph anyone would read
 * correctly, and both are choices a user makes once. `ThemeToggle` stays separate
 * because it is icon-only and carries its own 32px hit-area exception — these
 * segments are text and reach the 44px baseline, so they do not inherit it.
 */
import { RadioGroupItem, RadioGroupRoot } from 'reka-ui'

defineProps<{
	modelValue: T
	options: readonly { value: T; label: string }[]
	/** The group's accessible name. The row's own label is a sibling, not an
	 *  ancestor, so it does not name this on its own. */
	label: string
}>()

const emit = defineEmits<{ 'update:modelValue': [value: T] }>()
</script>

<template>
	<RadioGroupRoot
		:model-value="modelValue"
		:aria-label="label"
		class="border-separator bg-surface-hover inline-flex items-center gap-0.5 rounded-md border p-0.5"
		@update:model-value="(value) => emit('update:modelValue', value as T)"
	>
		<RadioGroupItem
			v-for="option in options"
			:key="option.value"
			:value="option.value"
			class="text-text-secondary hover:bg-surface-hover data-[state=checked]:bg-surface data-[state=checked]:text-text-primary focus-ring grid min-h-11 min-w-14 place-items-center rounded-sm px-3 text-meta transition-colors duration-fast"
		>
			{{ option.label }}
		</RadioGroupItem>
	</RadioGroupRoot>
</template>
