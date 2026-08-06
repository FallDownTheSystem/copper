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
</script>

<template>
	<div v-if="pending.length > 0" class="mb-1.5 min-w-0">
		<p class="text-text-secondary mb-1 flex items-center gap-1.5 text-meta">
			<IconLucidePaperclip class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
			<span>{{ pendingLabel }}</span>
		</p>

		<!-- A list rather than a row of chips: filenames are long, the panel is 390px
		     wide, and side-by-side chips would each truncate to nothing. -->
		<ul class="flex min-w-0 flex-col gap-1">
			<li
				v-for="attachment in pending"
				:key="attachment.id"
				class="border-separator bg-surface-hover flex min-w-0 items-center gap-1.5 rounded-md border px-2 py-1"
			>
				<span class="text-text-primary min-w-0 flex-1 truncate text-meta">
					{{ attachment.name }}
				</span>
				<span class="text-text-secondary shrink-0 text-meta">
					{{ formatBytes(attachment.bytes) }}
				</span>
				<!-- Removing is keyboard-reachable as an ordinary tab stop: the tray sits
				     in the composer, which is not the grid, so task-004's roving-focus
				     rules do not apply here and a `-1` would strand it. -->
				<button
					type="button"
					class="text-text-disabled hover:text-text-primary outline-focus-ring hit-44 relative shrink-0 rounded p-0.5 transition-colors duration-fast focus-visible:outline-2"
					:aria-label="`Remove ${attachment.name}`"
					@click="removePending(attachment.id)"
				>
					<IconLucideX class="size-3.5" aria-hidden="true" focusable="false" />
				</button>
			</li>
		</ul>
	</div>
</template>
