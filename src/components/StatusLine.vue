<script setup lang="ts">
const { message, clear } = useStatusMessage()

/**
 * Cleared on the next user action, not on a timer — which sits inside "prefer
 * explicit dismissal over timers" instead of having to defend a five-second
 * floor.
 *
 * Registered in the **capture** phase, and that is the whole trick: it therefore
 * runs before the handler the action itself is bound to, so the press that
 * clears is always the press *before* the one that writes. A bubble-phase
 * listener would wipe the message its own keystroke had just set.
 */
useEventListener(window, 'keydown', clear, { capture: true })
useEventListener(window, 'pointerdown', clear, { capture: true })
</script>

<template>
	<!-- Always in the DOM, empty until something writes: injecting a live region
	     and its text together does not announce, only a text change inside a
	     region already in the accessibility tree does. `sr-only` while empty
	     keeps it in that tree while taking it out of flow, so the band occupies
	     no space until it has something to say. -->
	<p
		role="status"
		class="text-text-primary rounded-md px-2 py-1.5 text-meta"
		:class="message ? 'border-separator bg-surface border shadow-sm' : 'sr-only'"
	>
		{{ message ?? '' }}
	</p>
</template>
