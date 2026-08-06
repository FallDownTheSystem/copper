<script setup lang="ts">
/**
 * A settings group: heading, trailing rule, rows.
 *
 * The small-caps-with-trailing-rule idiom is the list view's, reused rather than
 * reinvented — one internal vocabulary per product, and at 390px a rule is the
 * cheapest way to make a heading register as a boundary without spending vertical
 * space the panel cannot spare.
 *
 * The layout takes a fourth or fifth group with no structural change, which is
 * what task-010's section switcher and task-008's own in-panel rebinding layer
 * will need.
 */
defineProps<{ title: string }>()
</script>

<template>
	<section>
		<!-- Stored sentence case and uppercased in CSS, never hard-typed caps: a
		     screen reader spells out an all-caps string letter by letter. -->
		<div class="flex items-center gap-2">
			<h2 class="text-text-secondary shrink-0 text-label font-semibold uppercase">{{ title }}</h2>
			<span class="bg-separator h-px min-w-0 flex-1" aria-hidden="true" />
		</div>

		<div class="mt-1">
			<slot />
		</div>
	</section>
</template>
