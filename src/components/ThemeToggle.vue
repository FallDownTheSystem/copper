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
import { motion } from 'motion-v'
import { RadioGroupItem, RadioGroupRoot } from 'reka-ui'

import type { ThemePreference } from '@/composables/useSettings'
import { EASE_OUT_QUINT } from '@/lib/motion'

defineProps<{
	modelValue: ThemePreference
	/** The row's error region, when the row has one. Present only while the error
	 *  is, so the group is not described by an empty node the rest of the time. */
	errorId?: string
}>()
defineEmits<{ 'update:modelValue': [value: ThemePreference] }>()

const reducedMotion = useReducedMotion()

/** motion-v drives the Web Animations API, which `main.css`'s
 *  `prefers-reduced-motion` block cannot reach — the preference has to be read
 *  here, exactly as `CheckboxIcon` reads it. */
const pillTransition = computed(() =>
	reducedMotion.value ? { duration: 0 } : { duration: 0.15, ease: EASE_OUT_QUINT },
)

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
		:aria-invalid="errorId ? 'true' : undefined"
		:aria-describedby="errorId"
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
			v-slot="{ checked }"
			:value="option.value"
			:aria-label="option.label"
			class="text-text-secondary hover:bg-surface-hover data-[state=checked]:text-text-primary focus-ring relative grid size-8 place-items-center rounded-sm transition-colors duration-fast"
		>
			<!-- One pill that moves rather than a fill each segment paints on itself:
			     motion-v matches the leaving and arriving spans by `layout-id` and
			     animates the box between them, so the selection slides instead of
			     teleporting. Decorative only — `aria-checked` on the button is what a
			     screen reader hears, and the pill must not add a second cue. The icons
			     below carry `relative` so they paint above it: both are `z-index:
			     auto`, and document order is what decides between them. -->
			<motion.span
				v-if="checked"
				layout-id="theme-toggle-pill"
				:transition="pillTransition"
				aria-hidden="true"
				class="bg-surface absolute inset-0 rounded-sm"
			/>

			<!-- The focus ring above names a colour, which is safe in High Contrast for
			     a reason worth stating: under `forced-colors: active` the UA
			     force-adjusts `outline-color` to the system palette whatever the author
			     asked for, so the token cannot override the ring colour the user
			     configured in their OS. Leaving it colourless would only have meant
			     `currentColor` in the ordinary themes — a ring that changes hue with the
			     segment's own state, and does not match the panel's other rings. -->
			<IconLucideMonitor
				v-if="option.value === 'system'"
				class="relative size-4"
				aria-hidden="true"
				focusable="false"
			/>
			<IconLucideSun
				v-else-if="option.value === 'light'"
				class="relative size-4"
				aria-hidden="true"
				focusable="false"
			/>
			<IconLucideMoon v-else class="relative size-4" aria-hidden="true" focusable="false" />
		</RadioGroupItem>
	</RadioGroupRoot>
</template>
