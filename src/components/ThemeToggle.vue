<script setup lang="ts">
/**
 * The theme choice as a segmented control.
 *
 * Three mutually exclusive, rarely changed options should be visible rather than
 * hidden behind a click, and three stacked radios would eat more of the 358px
 * content width while reading as a form field instead of a preference.
 *
 * **Built on `RadioGroup`, not `ToggleGroup`, and the difference is not
 * cosmetic.** `ToggleGroupRoot` renders `role="group"` unconditionally — there is
 * no branching on `type` — and each item delegates to the `Toggle` primitive,
 * which renders `<button aria-pressed>`. So `type="single"` changes only which
 * value JS tracks, never the exposed ARIA: a screen-reader user hears three
 * independent toggle buttons, one of which happens to be pressed, rather than one
 * choice with three options. `RadioGroupRoot` gives `role="radiogroup"` /
 * `role="radio"` / `aria-checked` with the same roving tabindex, the same single
 * Tab stop and the same arrow-key selection, so nothing visual or keyboard-facing
 * changes.
 */
import { RadioGroupItem, RadioGroupRoot } from 'reka-ui'

import type { ThemePreference } from '@/composables/useSettings'

defineProps<{ modelValue: ThemePreference }>()
defineEmits<{ 'update:modelValue': [value: ThemePreference] }>()

const OPTIONS = [
	{ value: 'system', label: 'System theme' },
	{ value: 'light', label: 'Light theme' },
	{ value: 'dark', label: 'Dark theme' },
] as const satisfies readonly { value: ThemePreference; label: string }[]
</script>

<template>
	<RadioGroupRoot
		:model-value="modelValue"
		aria-label="Theme"
		class="border-separator bg-surface-hover inline-flex items-center gap-0.5 rounded-md border p-0.5"
		@update:model-value="(value) => $emit('update:modelValue', value as ThemePreference)"
	>
		<!-- 32×32 is a recorded exception to the 44px hit-area baseline, not an
		     oversight: three 44px segments plus the row label do not fit 358px of
		     content width, and the usual remedy — a pseudo-element expander — is
		     unavailable because expanded hit areas must never overlap and these
		     segments sit flush. 32px clears WCAG 2.5.8's 24px AA floor with room to
		     spare and misses only the AAA/HIG 44px target. Every other control in
		     this view reaches 44px through padding. -->
		<RadioGroupItem
			v-for="option in OPTIONS"
			:key="option.value"
			:value="option.value"
			:aria-label="option.label"
			class="text-text-secondary hover:bg-surface-hover data-[state=checked]:bg-surface data-[state=checked]:text-text-primary grid size-8 place-items-center rounded-sm transition-colors duration-fast focus-visible:outline-2 focus-visible:outline-offset-2"
		>
			<!-- The focus ring above sets no colour on purpose. A colourless outline
			     resolves to `currentColor` and, in Windows High Contrast, maps onto the
			     system `Highlight` palette automatically; a hardcoded one overrides the
			     ring colour the user configured in their OS and fights that palette. -->
			<IconLucideMonitor
				v-if="option.value === 'system'"
				class="size-4"
				aria-hidden="true"
				focusable="false"
			/>
			<IconLucideSun
				v-else-if="option.value === 'light'"
				class="size-4"
				aria-hidden="true"
				focusable="false"
			/>
			<IconLucideMoon v-else class="size-4" aria-hidden="true" focusable="false" />
		</RadioGroupItem>
	</RadioGroupRoot>
</template>
