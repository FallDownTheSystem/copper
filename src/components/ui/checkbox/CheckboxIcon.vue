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
	/** Reka's `CheckedState`, forwarded verbatim. `'indeterminate'` is in the type
	 *  because reka can produce it, not because Copper does — nothing here passes a
	 *  tri-state model value, so the dash the reference app draws for it is not
	 *  ported. It would be a second `motion.path`, a second motion value and a
	 *  second change subscription on every row of the list, all inert. */
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

const checkCap = useLinecap(checkLength)

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

/** Merged once per mark rather than once per render — one of these exists per row
 *  of the list. `cn` and not a plain list, so a caller's own `size-*` still wins. */
const svgClass = computed(() => cn('size-3.5', props.class))
</script>

<template>
	<svg
		:class="svgClass"
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
	</svg>
</template>
