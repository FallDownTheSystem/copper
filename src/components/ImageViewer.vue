<script setup lang="ts">
/**
 * The full-size image overlay: an in-panel sheet, not a window.
 *
 * **Fit to the panel, with no zoom or pan.** The specification offers that as an
 * explicit first pass and it is the one taken: at 390 × 660 a zoom control is a
 * lot of surface for a viewer whose job is "let me see the screenshot I pasted",
 * and task-011's OS-viewer path — still one keystroke away on `Space` — is where
 * a genuine close look belongs.
 *
 * **Layered between the list and the menus.** `z-25` sits above the status band
 * at `z-20` and below the portal host at `z-30`, so a menu opened over it would
 * still win. `absolute inset-0` rather than `fixed`, and nothing here scrolls:
 * task-004's first acceptance criterion is that the document scrolls in neither
 * axis, and `main` is the only scroll region in the panel.
 *
 * The keyboard is the shell's. `PanelShell` gives the viewer the first rung of
 * the Escape ladder and declines every other in-panel chord while it is up, for
 * the same reason an open menu declines them — so there is no second handler here
 * to fall out of step with that one.
 */
const { attachment, image, isOpen, close, reportBrokenImage } = useImageViewer()

const closeButton = useTemplateRef<HTMLButtonElement>('closeButton')

// Focus has to move into the overlay, or Escape and the close button are both
// unreachable — and the press would fall through to whatever the thumbnail left
// focused underneath. `useImageViewer.close` hands it back.
//
// `immediate`, because this component is not only mounted while the viewer is
// shut: the tray's `open-settings` unmounts `PanelShell` and this with it, so a
// viewer left open across a settings visit comes back with the overlay rendered
// and nothing focused. A watcher that only fires on a *change* never runs on that
// path.
watch(
	isOpen,
	(open) => {
		if (open) void nextTick(() => closeButton.value?.focus())
	},
	{ immediate: true },
)

// The epoch reaction that closes the viewer over a revoked blob lives in
// `useImageViewer` at module scope, not here: it has to survive this component
// being unmounted by the settings view.
</script>

<template>
	<div
		v-if="isOpen"
		role="dialog"
		aria-modal="true"
		:aria-label="attachment ? `${attachment.name}, full size` : 'Image'"
		class="bg-surface animate-in fade-in duration-fast absolute inset-0 z-25 flex flex-col backdrop-blur-md"
		@click.self="close"
		@keydown.tab.prevent
	>
		<!-- The one focusable control in here, which is what makes the `Tab` above a
		     complete focus trap rather than a cycle that has to be written out. -->
		<div class="border-separator flex min-h-10 shrink-0 items-center gap-2 border-b px-2">
			<span class="text-text-primary min-w-0 flex-1 truncate text-meta">
				{{ attachment?.name }}
			</span>
			<button
				ref="closeButton"
				type="button"
				aria-label="Close image"
				class="icon-button shrink-0"
				@click="close"
			>
				<IconLucideX class="size-4" aria-hidden="true" focusable="false" />
			</button>
		</div>

		<!-- `min-h-0` so the image is bounded by the panel rather than growing the
		     column past it, which is what would make the document scroll. -->
		<div class="grid min-h-0 flex-1 place-items-center p-2" @click.self="close">
			<!-- No `alt` of its own beyond the filename: the dialog is already labelled
			     with it, and `object-contain` is what keeps a wide screenshot inside the
			     390px width instead of cropping it. -->
			<!-- `@error` because Rust gating the bytes as an image is not the same
			     claim as the WebView being able to decode them: a truncated or
			     malformed file passes the magic-number check and then renders as a
			     broken glyph with nothing saying why. It reports through the same
			     refusal state a failed read does, so there is one place that explains
			     an image that will not appear. -->
			<img
				v-if="image.state === 'ready'"
				:src="image.url"
				:alt="attachment?.name ?? ''"
				class="max-h-full max-w-full object-contain"
				draggable="false"
				@error="reportBrokenImage"
			/>

			<p
				v-else-if="image.state === 'failed'"
				class="text-text-secondary px-4 text-center text-meta"
			>
				<IconLucideTriangleAlert
					class="text-destructive mx-auto mb-2 size-5"
					aria-hidden="true"
					focusable="false"
				/>
				{{ image.reason }}
			</p>

			<!-- Deliberately wordless. A ten-megabyte read off a local disk is over
			     before a spinner would finish its first turn, and the one case that is
			     slow — a disconnected network drive — ends in the message above. -->
			<span v-else class="sr-only" role="status">Loading image</span>
		</div>
	</div>
</template>
