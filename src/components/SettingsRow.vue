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

/** Named whether or not there is anything to say, because the region below is
 *  permanently mounted and the rows that carry no `for`-able control — the two
 *  segmented choices — still have to point their group at it. */
const errorId = useId()
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

			<!-- Two copies, and only one of them is in the accessibility tree.
			     Injecting an element and its text together does not announce — only a
			     text change inside a region already registered does — so the spoken
			     copy is permanently mounted and empty until there is something to say,
			     and the visible paragraph is hidden from assistive tech to stop the
			     message being read twice. `ShortcutRecorder` and the Updates row use
			     the same arrangement. The id is on the mounted copy rather than the
			     visible one so a control's `aria-describedby` always resolves.

			     Keyed on whether the row was *given* an error prop rather than on
			     whether it currently holds a message — the rows that can never fail
			     from here, and the shortcut rows that run their own pair of regions in
			     `below`, would otherwise each contribute a second empty alert. -->
			<p v-if="error" aria-hidden="true" class="text-text-primary mt-1.5 text-meta">{{ error }}</p>
			<span v-if="error !== undefined" :id="errorId" class="sr-only" role="alert">{{
				error ?? ''
			}}</span>
		</div>

		<!-- Absent rather than empty when there is no trailing control: a shortcut
		     row puts its control in `below`, and an empty box here would still take
		     the row's `gap-3`. -->
		<div v-if="$slots.default" class="shrink-0 self-center">
			<!-- Handed down rather than pushed in from the parent: the row owns the
			     message and the id, and the control only has to point at it. Withheld
			     while there is no error, so nothing is described by an empty region.

			     camelCase, unlike every other attribute here: a slot's props arrive
			     under the name as written rather than camelised the way a component's
			     are, so `:error-id` would be a key no destructuring reads. -->
			<slot :errorId="error ? errorId : undefined" />
		</div>
	</div>
</template>
