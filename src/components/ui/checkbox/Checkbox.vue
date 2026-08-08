<script setup lang="ts">
/**
 * Ported from the reference app at `dashboard-template`, with one bug fixed —
 * see `scalesOnPress`. Motion recipes adapted from cuelume (MIT, Daniel Belyi).
 *
 * Two structural properties are the reason this works and must survive any edit:
 * the root is a `motion.button` merged through `as-child`, so the press dip is on
 * the real control rather than a wrapper; and the indicator is `force-mount`ed,
 * so unchecking retracts the stroke instead of unmounting the element mid-draw.
 */
import type { CheckboxRootEmits, CheckboxRootProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { motion } from 'motion-v'
import { CheckboxIndicator, CheckboxRoot, useForwardPropsEmits } from 'reka-ui'
import { EASE_OUT_QUINT } from '@/lib/motion'
import { cn } from '@/lib/utils'
import CheckboxIcon from './CheckboxIcon.vue'

const props = defineProps<CheckboxRootProps & { class?: HTMLAttributes['class'] }>()
const emits = defineEmits<CheckboxRootEmits>()

const delegatedProps = reactiveOmit(props, 'class')

const forwarded = useForwardPropsEmits(delegatedProps, emits)

const reducedMotion = useReducedMotion()

/**
 * The reference app gates only its *radio* on the preference and lets the
 * checkbox animate regardless. That is a bug rather than a style, and this is
 * half of the fix (`CheckboxIcon` is the other half): `motion-v` drives the Web
 * Animations API, so `main.css`'s `prefers-reduced-motion` block — which can only
 * zero CSS transitions — never reaches either of them.
 */
const scalesOnPress = computed(() => !props.disabled && !reducedMotion.value)

// Dipped further than a full-size button: a 16px box needs a bigger ratio before
// the scale reads at all. `undefined` rather than `{}` when it is not going to
// dip: motion-v decides whether to register its press gesture on
// `Boolean(whilePress)`, so an empty object still puts a pointer listener on
// every row's box and still resolves a variant on every press to arrive at no
// change at all.
const pressState = computed(() => (scalesOnPress.value ? { scale: 0.9 } : undefined))

// Only the boxes that actually dip earn a permanent compositor layer.
const pressStyle = computed(() => (scalesOnPress.value ? { willChange: 'transform' } : undefined))

const pressTransition = { duration: 0.15, ease: EASE_OUT_QUINT }

/**
 * Resolved once per box rather than once per render: this control is mounted on
 * every row of the list, and `cn` is a clsx join plus a tailwind-merge parse.
 *
 * The radius sits at 8px — half of the 16px box, the capsule bound — and the
 * curve, not the radius, is what decides how round this reads: `corner-shape`
 * bends the arc within the radius it is given, so at the bound a circular arc
 * is a full circle and CSS's `squircle` (superellipse(2)) is the visibly
 * flat-sided classic. The user's ruling (2026-08-08) is that the flat-sided
 * version reads as a square here and the box should be an *almost*-circle —
 * the iOS icon curve — so this control wears `squircle-round`
 * (superellipse(1.4)) instead of the panel's stock `squircle`.
 *
 * The fallback split stays deliberate: `rounded-[7px]` is what a runtime with
 * no `corner-shape` renders, held one pixel short of the bound so it stays a
 * rounded square there instead of collapsing into the true circle that only
 * the superellipse is allowed to approach.
 */
const rootClass = computed(() =>
	cn(
		'squircle-round border-text-disabled focus-ring data-[state=checked]:bg-accent-ring data-[state=checked]:border-accent-ring text-accent-contrast size-4 shrink-0 rounded-[7px] supports-[corner-shape:squircle]:rounded-[8px] border transition-colors duration-base disabled:cursor-not-allowed disabled:opacity-50',
		props.class,
	),
)
</script>

<template>
	<!-- The mark carries its checked colour in every state, rather than switching
	     on `data-[state=checked]`, so a retracting stroke never swaps colour
	     mid-draw. Only the box's own fill transitions. -->
	<CheckboxRoot v-slot="slotProps" as-child v-bind="forwarded">
		<motion.button
			data-slot="checkbox"
			:style="pressStyle"
			:whilePress="pressState"
			:transition="pressTransition"
			:class="rootClass"
		>
			<!-- Force-mounted so unchecking can retract the stroke instead of
			     unmounting mid-draw. `transition-none` kills the inherited CSS
			     transition, which would otherwise fight the WAAPI one. -->
			<CheckboxIndicator
				force-mount
				data-slot="checkbox-indicator"
				class="grid place-content-center text-current transition-none"
			>
				<slot v-bind="slotProps">
					<CheckboxIcon :state="slotProps.state" />
				</slot>
			</CheckboxIndicator>
		</motion.button>
	</CheckboxRoot>
</template>
