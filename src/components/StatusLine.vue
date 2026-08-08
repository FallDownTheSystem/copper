<script setup lang="ts">
const { toast, clear, pause, resume } = useStatusMessage()
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

/** Both failure channels wear the same surface. They differ only in who cleared
 *  them — a retry for the store's, the `Dismiss` button for the toast's — and
 *  that is not a difference the reader should have to see a colour for. */
const failed = computed(() => listError.value !== null || toast.value?.severity === 'error')

/**
 * Keyed on the message rather than on the toast, so the error branch does not
 * re-animate every time an unrelated toast is replaced behind it.
 */
const pillKey = computed(() => (listError.value ? 'error' : (toast.value?.generation ?? 0)))

/**
 * What the screen reader is told, which is not everything the pill shows.
 *
 * The `list` failure already has an assertive region of its own in the shell, so
 * repeating it here would announce one failure twice, in two politenesses.
 */
const announcement = computed(() => toast.value?.text ?? '')

const region = useTemplateRef<HTMLElement>('region')

/** The toast has been used, so it stops offering. Cleared *before* running,
 *  because the action may write a message of its own — `undo` says so when there
 *  is nothing left to undo — and clearing afterwards would wipe it. */
function run() {
	const pressed = action.value
	clear()
	pressed?.run()
}

/**
 * Hands the keyboard back to something that exists, once the pill is actually
 * gone.
 *
 * **On `after-leave`, not on the press.** The button outlives the press by the
 * length of the leave transition, so a check run any earlier finds focus still
 * on an element that is still in the tree, concludes there is nothing to fix,
 * and is not there for the moment the element is removed underneath it.
 *
 * Only from `document.body`, so nothing that took focus in the meantime is
 * robbed. Body is not merely a poor place to stand: it is an *ancestor* of the
 * panel root, so every in-panel chord and the whole Escape ladder are gone until
 * a mouse puts focus back — which is why the last rung here is that root, the
 * same ending `useImageViewer.returnFocus` and `useSelection`'s relocation
 * watcher have.
 */
function restoreFocus() {
	void nextTick(() => {
		const active = document.activeElement
		if (active !== null && active !== document.body) return
		const root =
			region.value?.closest<HTMLElement>('[data-panel-root]') ??
			document.querySelector<HTMLElement>('[data-panel-root]')
		root?.focus()
	})
}
</script>

<template>
	<!-- Click-through, and the pill is too. This band overlays the last rows of
	     the note list, so anything here that took a click would swallow presses
	     aimed at a note underneath it for five seconds after every action. The one
	     exception is the action button, which re-enables pointer events for itself
	     — the only part of the pill that does anything. -->
	<div ref="region" class="pointer-events-none">
		<!-- **The live region is this element, and it holds no controls.** It is
		     always in the DOM and empty until there is something to say: injecting a
		     region and its text together does not announce, only a text change
		     inside a region already in the accessibility tree does.

		     It is separate from the pill rather than wrapped around it because a
		     region containing the button re-reads the whole thing — label included —
		     on every unrelated change, and because the visible pill is then free to
		     come and go on the transition the eye wants without that motion being an
		     announcement. -->
		<div class="sr-only" role="status">{{ announcement }}</div>

		<Transition name="toast" mode="out-in" @after-leave="restoreFocus">
			<p
				v-if="text"
				:key="pillKey"
				data-status-toast
				class="toast-pill flex items-center gap-2 rounded-md border px-2 py-1.5 text-meta"
				:class="
					failed
						? 'text-text-primary bg-surface-danger border-destructive/40'
						: 'text-text-primary bg-toast-surface border-separator'
				"
			>
				<!-- `min-w-0` so a long message wraps inside the pill rather than pushing
				     the button off the panel's edge, and `break-words` so a path or a URL
				     with no space in it wraps rather than setting that minimum itself. -->
				<span class="min-w-0 break-words">{{ text }}</span>

				<!-- No `hit-44`: the expander is a pseudo-element reaching 44px in every
				     direction, and on a pointer-events-auto control inside a click-through
				     overlay that would blank a strip of the list wider than the pill.

				     The four listeners are one behaviour: the clock stops while the reader
				     is at the button, by either input. Pointer and focus hold it
				     separately, so arriving with one and leaving with the other does not
				     start it early. -->
				<button
					v-if="action"
					type="button"
					data-toast-action
					class="focus-ring text-accent-text hover:bg-surface-hover pointer-events-auto -my-0.5 ml-auto shrink-0 rounded-md px-1.5 py-0.5 font-semibold transition-colors duration-fast"
					@click="run"
					@pointerenter="pause('pointer')"
					@pointerleave="resume('pointer')"
					@focusin="pause('focus')"
					@focusout="resume('focus')"
				>
					{{ action.label }}
				</button>
			</p>
		</Transition>
	</div>
</template>

<style scoped>
/* Symmetric and interruptible, where the entrance used to be a keyframe: a pill
   replaced mid-animation now reverses out of the position it reached instead of
   restarting from the bottom of its travel. The leave is the quicker of the two
   because it is the half nobody is waiting to read.

   Reduced motion is not handled here — `main.css` collapses every duration in
   the app, transitions included. */
.toast-enter-active {
	transition:
		opacity 150ms var(--ease-out-quint),
		translate 150ms var(--ease-out-quint);
}

.toast-leave-active {
	transition:
		opacity 100ms var(--ease-out-quint),
		translate 100ms var(--ease-out-quint);
}

/* `translate` rather than a `transform`, so the utility classes on the pill keep
   whatever transform they set. */
.toast-enter-from,
.toast-leave-to {
	opacity: 0;
	translate: 0 8px;
}
</style>
