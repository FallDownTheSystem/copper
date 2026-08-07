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

const pressTransition = { duration: 0.15, ease: [0.22, 1, 0.36, 1] as const }

/**
 * Resolved once per box rather than once per render: this control is mounted on
 * every row of the list, and `cn` is a clsx join plus a tailwind-merge parse.
 *
 * The radius is what makes the superellipse visible at all. `corner-shape` bends
 * the corner arc within the radius it is given, so on a 16px box a 4px radius —
 * the reference app's value, carried over unexamined — leaves the squircle and
 * the circular arc within a pixel of each other, and the box reads as a plain
 * rounded square however well the property is supported. 6px is most of the way
 * to the 8px that would make the whole edge one curve, which is where the
 * difference is legible while the shape is still a square with soft corners —
 * and it is a sane plain radius on a runtime that ignores `corner-shape`, since
 * that `rounded-[6px]` is the fallback.
 */
const rootClass = computed(() =>
	cn(
		'squircle border-text-disabled outline-focus-ring data-[state=checked]:bg-accent-ring data-[state=checked]:border-accent-ring size-4 shrink-0 rounded-[6px] border text-white transition-colors duration-base focus-visible:outline-2 focus-visible:outline-offset-1 disabled:cursor-not-allowed disabled:opacity-50',
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
