<script setup lang="ts">
/**
 * One label-and-control line.
 *
 * `align` exists because the two control shapes want different baselines: a
 * switch or a segmented control is one line tall and centres against the label,
 * while a shortcut control is two or three and has to align to the label's cap
 * height instead.
 */
withDefaults(
	defineProps<{
		label: string
		description?: string
		/** For the accessible name of a control that has no visible text of its own. */
		labelFor?: string
		align?: 'center' | 'start'
	}>(),
	{ description: undefined, labelFor: undefined, align: 'center' },
)
</script>

<template>
	<div class="flex items-start justify-between gap-3 py-3">
		<!-- `min-w-0` is load-bearing: a flex child defaults to `min-width: auto`,
		     so without it a long label widens the row instead of wrapping. -->
		<div class="min-w-0 flex-1">
			<label v-if="labelFor" :for="labelFor" class="text-text-primary block text-body font-medium">
				{{ label }}
			</label>
			<p v-else class="text-text-primary text-body font-medium">{{ label }}</p>

			<p v-if="description" class="text-text-secondary mt-0.5 text-meta text-pretty">
				{{ description }}
			</p>

			<slot name="below" />
		</div>

		<!-- Absent rather than empty when there is no trailing control: a shortcut
		     row puts its control in `below`, and an empty box here would still take
		     the row's `gap-3`. -->
		<div
			v-if="$slots.default"
			class="shrink-0"
			:class="align === 'center' ? 'self-center' : 'self-start pt-0.5'"
		>
			<slot />
		</div>
	</div>
</template>
