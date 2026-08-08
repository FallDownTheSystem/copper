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
import { motion } from 'motion-v'
import { RadioGroupItem, RadioGroupRoot } from 'reka-ui'

import { EASE_OUT_QUINT } from '@/lib/motion'

const props = defineProps<{
	modelValue: T
	options: readonly { value: T; label: string }[]
	/** The group's accessible name. The row's own label is a sibling, not an
	 *  ancestor, so it does not name this on its own. */
	label: string
	/** The row's error region, when the row has one. Present only while the error
	 *  is, so the group is not described by an empty node the rest of the time. */
	errorId?: string
}>()

const emit = defineEmits<{ 'update:modelValue': [value: T] }>()

const reducedMotion = useReducedMotion()

/** Per group, not per component: motion-v matches shared-layout elements by this
 *  id alone, so two segmented controls on the same screen sharing one id would
 *  hand the pill back and forth across the view. */
const pillId = computed(() => `settings-choice-pill:${props.label}`)

/** motion-v drives the Web Animations API, which `main.css`'s
 *  `prefers-reduced-motion` block cannot reach — the preference has to be read
 *  here, exactly as `CheckboxIcon` reads it. */
const pillTransition = computed(() =>
	reducedMotion.value ? { duration: 0 } : { duration: 0.15, ease: EASE_OUT_QUINT },
)
</script>

<template>
	<RadioGroupRoot
		:model-value="modelValue"
		:aria-label="label"
		:aria-invalid="errorId ? 'true' : undefined"
		:aria-describedby="errorId"
		class="border-separator bg-surface-hover inline-flex items-center gap-0.5 rounded-md border p-0.5"
		@update:model-value="(value) => emit('update:modelValue', value as T)"
	>
		<RadioGroupItem
			v-for="option in options"
			:key="option.value"
			v-slot="{ checked }"
			:value="option.value"
			class="text-text-secondary hover:bg-surface-hover data-[state=checked]:text-text-primary focus-ring relative grid min-h-11 min-w-14 place-items-center rounded-sm px-3 text-meta transition-colors duration-fast"
		>
			<!-- One pill that moves rather than a fill each segment paints on itself:
			     motion-v matches the leaving and arriving spans by `layout-id` and
			     animates the box between them, so the selection slides instead of
			     teleporting. Decorative only — `aria-checked` on the button is what a
			     screen reader hears, and the pill must not add a second cue. -->
			<motion.span
				v-if="checked"
				:layout-id="pillId"
				:transition="pillTransition"
				aria-hidden="true"
				class="bg-surface absolute inset-0 rounded-sm"
			/>
			<!-- Positioned, so it paints above the pill: both are `z-index: auto`, and
			     document order is what decides between them. -->
			<span class="relative">{{ option.label }}</span>
		</RadioGroupItem>
	</RadioGroupRoot>
</template>
