<script setup lang="ts">
/**
 * The panel-wide drop treatment, and the only listener for Tauri's drag events.
 *
 * **Why an OS-level drop rather than HTML5 drag-and-drop.** `dragDropEnabled`
 * is on — it is Tauri's default — and while it is on, the WebView receives no
 * HTML5 drag events at all. That would have been a blocking hazard for
 * task-006's note reordering, except that `@formkit/drag-and-drop` is
 * configured with `nativeDrag: false` and so runs entirely on synthetic pointer
 * events. The two are independent, which is what cleared this task's gate.
 *
 * A drop on a **hidden** panel does nothing, and needs no guard: a window that
 * is not on screen is not a drop target as far as the OS is concerned, so no
 * event arrives.
 */
import { getCurrentWebview } from '@tauri-apps/api/webview'
import type { UnlistenFn } from '@tauri-apps/api/event'

const { attachPaths } = useAttachments()
const { reportActionError } = useSpace()

const over = ref(false)
let unlisten: UnlistenFn | null = null

/**
 * `onDragDropEvent` delivers enter, over, drop and leave through one
 * subscription. `over` fires continuously while the pointer moves, so it is
 * folded into the same "show the treatment" branch rather than given one of its
 * own — the flag is idempotent and re-setting it costs nothing.
 */
onMounted(async () => {
	try {
		unlisten = await getCurrentWebview().onDragDropEvent(async (event) => {
			if (event.payload.type === 'enter' || event.payload.type === 'over') {
				over.value = true
				return
			}

			// Both remaining types dismiss the treatment, and dismissing it *before*
			// the ingest round trip matters: leaving it up while files are read would
			// make a large drop look like the panel had hung.
			over.value = false
			if (event.payload.type !== 'drop') return

			const message = await attachPaths(event.payload.paths)
			// The composer's surface, because the tray it concerns lives there — a
			// drop is a way of filling the composer, however far from it the pointer
			// was.
			if (message) reportActionError('composer', message)
		})
	} catch (error) {
		// Dropping files is one of three ways to attach one, and the other two are
		// unaffected — so a failed registration costs a convenience rather than the
		// feature. Logged rather than rethrown: this runs in `onMounted`, where a
		// rejection is unhandled and would take down nothing but the console.
		console.error('[copper] could not listen for file drops', error)
	}
})

onUnmounted(() => {
	unlisten?.()
	unlisten = null
})
</script>

<template>
	<!-- Above the portal host so it is not painted over by an open menu, and
	     `pointer-events-none` throughout: the drag is the OS's, not the
	     document's, so this element must never become a hit-test target that
	     could swallow a click after the drag ends. -->
	<div
		v-if="over"
		class="border-accent-ring bg-surface/80 pointer-events-none absolute inset-0 z-40 m-2 grid place-items-center rounded-lg border-2 border-dashed"
		aria-hidden="true"
	>
		<p class="text-text-primary flex items-center gap-2 text-body font-semibold">
			<IconLucidePaperclip class="size-4" aria-hidden="true" focusable="false" />
			<span>Drop to attach</span>
		</p>
	</div>
</template>
