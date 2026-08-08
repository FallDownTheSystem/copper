<script setup lang="ts" generic="T extends string">
/**
 * A colour family chosen from a grid of swatches.
 *
 * **`RadioGroup`, not `ToggleGroup`**, for the reason `ThemeToggle` records at
 * length: `ToggleGroupRoot` renders `role="group"` unconditionally and each item
 * renders `<button aria-pressed>`, so a screen-reader user hears eighteen
 * independent toggles rather than one choice. `RadioGroupRoot` gives
 * `role="radiogroup"` / `role="radio"` / `aria-checked` with the same roving
 * tabindex, the same single Tab stop and the same arrow-key selection.
 *
 * **A grid rather than a wrapping row**, and six columns rather than as many as
 * fit. The accents are one set of eighteen and the tones are one set of six, so a
 * fixed six-wide grid gives the two rows the same width and the same rhythm; left
 * to wrap, the tone row would break after five and leave one swatch stranded
 * underneath. Six is also what the panel's ~376px of content width can hold while
 * still leaving the label column room for a description.
 *
 * Colour is the only thing that distinguishes one option from another here, which
 * would normally fail "never rely on colour alone" — so the name is not left to
 * the eye: every swatch carries it as its accessible name, and the row's palette
 * action and the settings row both say the current one in words.
 */
import { RadioGroupItem, RadioGroupRoot } from 'reka-ui'

defineProps<{
	modelValue: T
	options: readonly { value: T; label: string; swatch: string }[]
	/** The group's accessible name. The row's own label is a sibling, not an
	 *  ancestor, so it does not name this on its own. */
	label: string
	/** The row's error region, when the row has one. Present only while the error
	 *  is, so the group is not described by an empty node the rest of the time. */
	errorId?: string
}>()

const emit = defineEmits<{ 'update:modelValue': [value: T] }>()
</script>

<template>
	<RadioGroupRoot
		:model-value="modelValue"
		:aria-label="label"
		:aria-invalid="errorId ? 'true' : undefined"
		:aria-describedby="errorId"
		class="grid grid-cols-6 gap-0.5"
		@update:model-value="(value) => emit('update:modelValue', value as T)"
	>
		<!-- 28×28 is the same recorded exception `ThemeToggle` takes, arrived at the
		     same way: six swatches plus the row's label do not fit 376px at 44px each,
		     and the usual remedy — `hit-44`'s pseudo-element expander — is unavailable
		     because expanded hit areas must never overlap and these sit flush. 28px
		     clears WCAG 2.5.8's 24px AA floor and misses only the AAA/HIG target. -->
		<RadioGroupItem
			v-for="option in options"
			:key="option.value"
			v-slot="{ checked }"
			:value="option.value"
			:aria-label="option.label"
			class="focus-ring relative grid size-7 place-items-center rounded-full"
		>
			<!-- The selected ring is its own element rather than a border or a shadow on
			     the button, and that is what keeps it compatible with `focus-ring`: the
			     focus treatment owns `outline` and `box-shadow` on this element, so a
			     selected swatch that also had focus would otherwise have to give one of
			     the two up. Decorative — `aria-checked` is what a screen reader hears. -->
			<span
				v-if="checked"
				aria-hidden="true"
				class="border-accent-ring absolute inset-0 rounded-full border"
			/>
			<!-- Inline, because the value is a family's own colour rather than one of the
			     panel's roles: a token per family would be eighteen tokens that no other
			     rule reads. The ring is drawn on the fill for the same reason the
			     `--control-border` token exists — a pale swatch on the light panel is
			     otherwise a shape with no edge. -->
			<span
				class="ring-control-border/40 size-4 rounded-full ring-1"
				:style="{ backgroundColor: option.swatch }"
			/>
		</RadioGroupItem>
	</RadioGroupRoot>
</template>
