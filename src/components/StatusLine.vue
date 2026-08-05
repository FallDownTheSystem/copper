<script setup lang="ts">
const { message, clear } = useStatusMessage()
const { errorFor } = useSpace()

/**
 * Every action in this task reports a failed mutation on the `list` scope, and
 * until now nothing rendered it: the failure reached the assertive live region
 * for screen readers and was invisible to everyone else. It shares this band
 * rather than getting a fourth surface — the panel cannot grow, and an error and
 * a confirmation from the same action are never both true.
 *
 * The error wins when both are present: a confirmation left standing next to a
 * failure would be the more misleading of the two.
 */
const listError = errorFor('list')
const text = computed(() => listError.value ?? message.value)

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
		class="rounded-md px-2 py-1.5 text-meta"
		:class="[
			text ? 'bg-surface border shadow-sm' : 'sr-only',
			listError
				? 'text-text-primary border-destructive/40 bg-destructive/10'
				: 'text-text-primary border-separator',
		]"
	>
		{{ text ?? '' }}
	</p>
</template>
