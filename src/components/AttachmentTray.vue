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
const { pending, pendingLabel, removePending } = useAttachments()

// The same `<TransitionGroup>` hooks as the note list, gated on the
// reduced-motion pair alone — the tray has no drag and no external reload to
// stand down for. `reduced` folds in Copper's "Animate controls" setting, which
// the hooks drive through the Web Animations API where main.css's root gate
// cannot reach.
//
// Accepted and by design: the `v-if` around the `<ul>` means the first
// attachment arriving and the last one leaving take the whole list with them
// and stay instant. Only the rows added and removed in between animate.
const reduced = useReducedMotion()

const { moveClass, onEnter, onLeave, onEnterCancelled, onLeaveCancelled } = useListTransition(
	() => !reduced.value,
)
</script>

<template>
	<div v-if="pending.length > 0" class="mb-1.5 min-w-0">
		<p class="text-text-secondary mb-1 flex items-center gap-1.5 text-meta">
			<IconLucidePaperclip class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
			<span>{{ pendingLabel }}</span>
		</p>

		<!-- A list rather than a row of chips: filenames are long, the panel is 440px
		     wide, and side-by-side chips would each truncate to nothing. -->
		<TransitionGroup
			tag="ul"
			class="flex min-w-0 flex-col gap-1"
			:css="false"
			:move-class="moveClass"
			@enter="onEnter"
			@leave="onLeave"
			@enter-cancelled="onEnterCancelled"
			@leave-cancelled="onLeaveCancelled"
		>
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
					:title="`Remove ${attachment.name}`"
					@click="removePending(attachment.id)"
				>
					<IconLucideX class="size-3.5" aria-hidden="true" focusable="false" />
				</button>
			</li>
		</TransitionGroup>
	</div>
</template>
