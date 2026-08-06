<script setup lang="ts">
/** One label-and-control line. */
defineProps<{
	label: string
	description?: string
	/** For the accessible name of a control that has no visible text of its own. */
	labelFor?: string
	/** A failed action for this row. Rows whose message is richer than a line of
	 *  text — a shortcut row's icon and its live regions — use `below` instead. */
	error?: string | null
}>()
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

			<p v-if="error" class="text-text-primary mt-1.5 text-meta" role="alert">{{ error }}</p>
		</div>

		<!-- Absent rather than empty when there is no trailing control: a shortcut
		     row puts its control in `below`, and an empty box here would still take
		     the row's `gap-3`. -->
		<div v-if="$slots.default" class="shrink-0 self-center">
			<slot />
		</div>
	</div>
</template>
