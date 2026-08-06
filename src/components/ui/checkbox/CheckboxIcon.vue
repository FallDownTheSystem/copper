<script setup lang="ts">
/**
 * The animated mark: an SVG `pathLength` draw rather than an opacity or scale
 * fade, so the check is *written* on. Ported from the reference app; motion
 * recipes adapted from cuelume (MIT, Daniel Belyi).
 */
import type { MotionValue } from 'motion-v'
import type { HTMLAttributes } from 'vue'
import { motion, useMotionValue } from 'motion-v'
import { cn } from '@/lib/utils'

const props = defineProps<{
	state: boolean | 'indeterminate'
	class?: HTMLAttributes['class']
}>()

const reducedMotion = useReducedMotion()

/**
 * A round cap still paints a dot when its stroke has zero length, which would
 * leave a speck floating in an empty box. Wear the cap only while there is a
 * stroke to cap.
 */
function useLinecap(length: MotionValue<number>) {
	const cap = shallowRef(length.get() === 0 ? 'butt' : 'round')
	onScopeDispose(
		length.on('change', (value) => {
			cap.value = value === 0 ? 'butt' : 'round'
		}),
	)
	return cap
}

// Seeded from the state at mount so a note that is already done simply shows its
// mark. Only a state the user changes gets drawn.
const checkLength = useMotionValue(props.state === true ? 1 : 0)
const dashLength = useMotionValue(props.state === 'indeterminate' ? 1 : 0)

const checkCap = useLinecap(checkLength)
const dashCap = useLinecap(dashLength)

/**
 * Drawing a mark on is the confirmation, so it gets room to read. Wiping one off
 * is just cleanup and should be out of the way before the next click lands.
 *
 * The reduced-motion branch is the fix for the reference app's bug: there, only
 * the radio consulted the preference and the stroke drew regardless.
 */
function draw(visible: boolean) {
	if (reducedMotion.value) return { duration: 0 } as const
	return { type: 'spring', bounce: 0, duration: visible ? 0.3 : 0.15 } as const
}
</script>

<template>
	<svg
		:class="cn('size-3.5', props.class)"
		viewBox="0 0 24 24"
		fill="none"
		stroke="currentColor"
		stroke-width="3"
		stroke-linejoin="round"
		aria-hidden="true"
		focusable="false"
	>
		<motion.path
			d="M4 12L10 18L20 6"
			:initial="false"
			:style="{ pathLength: checkLength }"
			:animate="{ pathLength: state === true ? 1 : 0 }"
			:transition="draw(state === true)"
			:stroke-linecap="checkCap"
		/>
		<motion.path
			d="M5 12H19"
			:initial="false"
			:style="{ pathLength: dashLength }"
			:animate="{ pathLength: state === 'indeterminate' ? 1 : 0 }"
			:transition="draw(state === 'indeterminate')"
			:stroke-linecap="dashCap"
		/>
	</svg>
</template>
