<script setup lang="ts">
/**
 * The composer's pending list: what the next submission will carry.
 *
 * Rendered above the field in a row it never shares, like the active-section
 * chip, so appearing and disappearing shifts the field rather than reflowing
 * something beside it. The panel cannot grow, so this row is the only place the
 * space comes from — which is why each item is one line and the whole list is
 * capped at ten by `useAttachments`.
 */
import autoAnimate, { type AnimationController } from '@formkit/auto-animate'

import { listMotion } from '@/lib/listMotion'

const { pending, pendingLabel, removePending } = useAttachments()

// The imperative controller rather than `v-auto-animate`, and gated explicitly,
// for the same reason as `NoteSection`: the directive gives no handle to
// disable, and auto-animate drives the Web Animations API — so neither the
// `prefers-reduced-motion` block nor the `.reduce-motion` root class can reach
// it, and the library's own media check misses Copper's "Animate controls".
const list = useTemplateRef<HTMLElement>('list')
let controller: AnimationController | null = null

const reduced = useReducedMotion()

function syncAnimation() {
	if (!controller) return
	if (reduced.value) controller.disable()
	else controller.enable()
}

// Watched rather than set up in `onMounted`, because the `<ul>` is inside
// `v-if="pending.length > 0"` and is therefore absent at mount whenever the
// composer opens with nothing attached — the common case. The element is also
// destroyed and rebuilt each time the tray empties and refills, so the
// controller has to be rebound to whatever element is current.
//
// Accepted and by design: that same `v-if` means the first attachment arriving
// and the last one leaving take the whole list with them and stay instant. Only
// the rows added and removed in between animate.
watch([list, reduced], ([element]) => {
	if (!element) {
		controller = null
		return
	}
	controller ??= autoAnimate(element, listMotion)
	syncAnimation()
})
</script>

<template>
	<div v-if="pending.length > 0" class="mb-1.5 min-w-0">
		<p class="text-text-secondary mb-1 flex items-center gap-1.5 text-meta">
			<IconLucidePaperclip class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
			<span>{{ pendingLabel }}</span>
		</p>

		<!-- A list rather than a row of chips: filenames are long, the panel is 440px
		     wide, and side-by-side chips would each truncate to nothing. -->
		<ul ref="list" class="flex min-w-0 flex-col gap-1">
			<li
				v-for="attachment in pending"
				:key="attachment.id"
				class="squircle border-separator bg-surface-hover flex min-w-0 items-center gap-1.5 rounded-md border px-2 py-1"
			>
				<span class="text-text-primary min-w-0 flex-1 truncate text-meta">
					{{ attachment.name }}
				</span>
				<span class="text-text-secondary shrink-0 text-meta">
					{{ formatBytes(attachment.bytes) }}
				</span>
				<!-- Removing is keyboard-reachable as an ordinary tab stop: the tray sits
				     in the composer, which is not the grid, so task-004's roving-focus
				     rules do not apply here and a `-1` would strand it.

				     Deliberately not `hit-44`. These rows stack about 27px apart, so two
				     44px pseudo-element expanders would overlap and make part of each
				     other's edge unhittable — the exact case main.css's own rule forbids.
				     The `-m-1 p-1.5` pair grows the real target to roughly 26px instead,
				     which the negative margin keeps out of the row's layout height. -->
				<button
					type="button"
					class="text-text-disabled hover:text-text-primary focus-ring -m-1 shrink-0 rounded-sm p-1.5 transition-colors duration-fast"
					:aria-label="`Remove ${attachment.name}`"
					@click="removePending(attachment.id)"
				>
					<IconLucideX class="size-3.5" aria-hidden="true" focusable="false" />
				</button>
			</li>
		</ul>
	</div>
</template>
