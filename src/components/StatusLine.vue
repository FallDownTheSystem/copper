<script setup lang="ts">
const { toast, clear } = useStatusMessage()
const { errorFor } = useSpace()

/**
 * A failed `list`-scope mutation is rendered here rather than only reaching the
 * assertive live region, where it would be invisible to everyone not using a
 * screen reader. It shares this pill rather than getting a fourth surface — the
 * panel cannot grow, and an error and a confirmation from the same action are
 * never both true.
 *
 * The error wins when both are present: a confirmation left standing next to a
 * failure would be the more misleading of the two. It also carries no action and
 * no timer — it is cleared by `clearActionError`, so it stays until the thing it
 * is about is retried.
 */
const listError = errorFor('list')
const text = computed(() => listError.value ?? toast.value?.text ?? null)
const action = computed(() => (listError.value ? null : (toast.value?.action ?? null)))

/**
 * Keyed on the message rather than on the toast, so the error branch does not
 * re-animate every time an unrelated toast is replaced behind it.
 */
const pillKey = computed(() => (listError.value ? 'error' : (toast.value?.generation ?? 0)))

/** The toast has been used, so it stops offering. Cleared *before* running,
 *  because the action may write a message of its own — `undo` says so when there
 *  is nothing left to undo — and clearing afterwards would wipe it. */
function run() {
	const pressed = action.value
	clear()
	pressed?.run()
}
</script>

<template>
	<!-- **The live region is the wrapper, and it is always in the DOM.** Injecting
	     a region and its text together does not announce; only a text change
	     inside a region already in the accessibility tree does. The pill itself
	     comes and goes inside it.

	     Click-through, and the pill is too. This band overlays the last rows of
	     the note list, so anything here that took a click would swallow presses
	     aimed at a note underneath it for five seconds after every action. The one
	     exception is the action button, which re-enables pointer events for itself
	     — the only part of the pill that does anything. -->
	<div class="pointer-events-none" role="status">
		<p
			v-if="text"
			:key="pillKey"
			data-status-toast
			class="toast-pill animate-in fade-in slide-in-from-bottom-1 flex items-center gap-2 rounded-md border px-2 py-1.5 text-meta duration-100"
			:class="
				listError
					? 'text-text-primary bg-surface-danger border-destructive/40'
					: 'text-text-primary bg-toast-surface border-separator'
			"
		>
			<!-- `min-w-0` so a long message wraps inside the pill rather than pushing
			     the button off the panel's edge. -->
			<span class="min-w-0">{{ text }}</span>

			<!-- No `hit-44`: the expander is a pseudo-element reaching 44px in every
			     direction, and on a pointer-events-auto control inside a click-through
			     overlay that would blank a strip of the list wider than the pill. -->
			<button
				v-if="action"
				type="button"
				data-toast-action
				class="focus-ring text-accent-text hover:bg-surface-hover pointer-events-auto -my-0.5 ml-auto shrink-0 rounded-md px-1.5 py-0.5 font-semibold transition-colors duration-fast"
				@click="run"
			>
				{{ action.label }}
			</button>
		</p>
	</div>
</template>
