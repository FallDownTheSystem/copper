<script setup lang="ts">
const { notice, initialize } = useCaptureNotice()

// Registering here rather than in `PanelShell` keeps the readiness signal next
// to the listeners it is a promise about: capture is armed on the strength of
// this resolving.
onMounted(() => {
	void initialize()
})
</script>

<template>
	<!-- Placed in the shell's middle grid cell rather than added as a fourth row,
	     and `self-end` so it sits at the bottom of the scroll region — directly
	     above the pinned composer. Two grid items in one cell overlap, so the
	     notice cannot displace the composer of a fixed-size panel, and it needs
	     no measurement and no positioning context to find where the composer
	     starts.

	     `pointer-events-none` because the panel is revealed *without* focus and
	     the user is still typing into another application — a band that could
	     take a click would be a trap. It holds no interactive elements for the
	     same reason.

	     `role="status"` announces it without stealing focus. Pre-rendered and
	     empty, because injecting a live region and its text together does not
	     announce; only a text change inside a region already in the tree does.

	     No enter or exit transition. The notice lives for 1500 ms and the user's
	     attention is by definition elsewhere; a fade would spend a meaningful
	     fraction of that lifetime arriving. -->
	<div class="pointer-events-none col-start-1 row-start-2 z-20 self-end px-3 pb-2" role="status">
		<p
			v-if="notice"
			:data-cause="notice.cause"
			class="border-separator bg-surface text-text-primary rounded-md border px-2 py-1.5 text-meta shadow-sm"
		>
			{{ notice.message }}
		</p>
	</div>
</template>
