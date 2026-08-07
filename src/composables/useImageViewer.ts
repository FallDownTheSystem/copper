/**
 * The in-panel image viewer's state: which attachment is showing, whether its
 * bytes have arrived, and where focus goes when it closes.
 *
 * Module scope for the usual reason — the card that opens it, the overlay that
 * renders it and the shell's Escape ladder all have to be looking at the same
 * state, and a ref declared inside the exported function hands each of them a
 * private copy.
 *
 * One-directional in the way `useSections` is: it invokes nothing itself and
 * reaches Rust only through `useAttachments`, which stays the one adapter for the
 * attachment surface and the one place an object URL is created or revoked.
 *
 * **Nothing here is a window.** The overlay is `absolute inset-0` inside the
 * panel root, so it is clipped by the same rounded rectangle everything else is
 * and cannot make the document scroll.
 */

import type { Attachment } from './useAttachments'
import { useAttachments } from './useAttachments'

export type ViewerImage =
	| { state: 'loading' }
	| { state: 'ready'; url: string }
	| { state: 'failed'; reason: string }

const attachment = ref<Attachment | null>(null)
const image = ref<ViewerImage>({ state: 'loading' })

/**
 * The element to hand focus back to, held outside the reactive state because
 * nothing renders it.
 *
 * Recorded at open time rather than read at close time: by then the overlay's own
 * close button is what has focus, and `document.activeElement` would answer with
 * that instead of with the thumbnail the user pressed.
 */
let invoker: HTMLElement | null = null

/** The row that element was in, as a fallback for when the element itself is
 *  gone — see [`returnFocus`]. Recorded at open time for the same reason the
 *  element is: afterwards there is nothing left to read it off. */
let invokerRow: string | null = null

/** Discards a read whose viewer has since been closed or re-pointed. Two
 *  double-clicks a moment apart are two reads, and the loser must not paint over
 *  the winner. */
let session = 0

const isOpen = computed(() => attachment.value !== null)

/**
 * Hands the keyboard back to something that exists.
 *
 * **Resolved inside the tick, not before it.** The overlay is still up when
 * `close` runs and the list re-renders between the two, so an element that was
 * connected at close time can be detached by the time focus would move to it —
 * which is exactly what a project switch does, and it is not a race the check can
 * be moved earlier to avoid. The whole ladder is therefore evaluated late.
 *
 * The last rung is the panel root rather than nothing at all. `document.body` is
 * an *ancestor* of that root, so focus falling back to it puts every press
 * outside the shell's keydown handler — no Escape ladder, no chords, and no way
 * back in but the mouse. `useSelection`'s own relocation watcher ends the same
 * way and for the same reason.
 */
function returnFocus() {
	const element = invoker
	const row = invokerRow
	invoker = null
	invokerRow = null

	void nextTick(() => {
		if (element?.isConnected) {
			element.focus()
			return
		}
		// Compared rather than selected: a row key carries the note's own id, and a
		// hand-edited `.copper` can put a quote in one.
		const rows = document.querySelectorAll<HTMLElement>('[data-row-id]')
		for (const candidate of rows) {
			if (candidate.dataset.rowId !== row) continue
			candidate.focus()
			return
		}
		document.querySelector<HTMLElement>('[data-panel-root]')?.focus()
	})
}

/** The url currently held, so `close` revokes exactly what it created. Read from
 *  the ref rather than kept alongside it, since `image` is the only writer. */
function heldUrl(): string | null {
	return image.value.state === 'ready' ? image.value.url : null
}

async function open(target: Attachment, from: HTMLElement | null) {
	// A viewer already showing something is closed first, so its URL is revoked
	// rather than orphaned — and its `invoker` is replaced, which is right: the
	// element the user last pressed is where they expect to land.
	close()

	const token = ++session
	attachment.value = target
	image.value = { state: 'loading' }
	invoker = from
	invokerRow = from?.closest<HTMLElement>('[data-row-id]')?.dataset.rowId ?? null

	const result = await useAttachments().loadFullImage(target.file)
	if (token !== session) {
		// The viewer moved on. Revoke rather than leak: nothing will render this.
		if ('url' in result) useAttachments().revokeFullImage(result.url)
		return
	}

	image.value =
		'url' in result
			? { state: 'ready', url: result.url }
			: { state: 'failed', reason: result.reason }
}

/**
 * Closes and returns focus.
 *
 * Idempotent, because it is reached from four places — Escape, the close button,
 * a click on the backdrop, and a space switch — and two of them can happen in the
 * same tick.
 */
function close() {
	if (!attachment.value) return

	session++
	const url = heldUrl()
	if (url) useAttachments().revokeFullImage(url)

	attachment.value = null
	image.value = { state: 'loading' }

	returnFocus()
}

/**
 * The WebView could not decode what Rust sent.
 *
 * Rust gating on the sniffed type says the bytes *begin* like an image, not that
 * they are a whole one — a file truncated by a failed copy or an interrupted
 * write passes every check on both sides and then fails in the decoder. Without
 * this the overlay shows a broken-image glyph and no reason; with it the failure
 * reads the same way a refused read does. The URL is revoked here rather than at
 * close, because nothing is ever going to render it.
 */
function reportBrokenImage() {
	const url = heldUrl()
	if (!url) return
	useAttachments().revokeFullImage(url)
	image.value = {
		state: 'failed',
		reason: 'That image could not be displayed — the file may be incomplete.',
	}
}

/**
 * A revoked preview cache closes the viewer, **and this watcher lives here rather
 * than in the component**.
 *
 * `clearPreviews` revokes the blob the overlay is rendering, so the viewer has to
 * go with it. Watching from `ImageViewer.vue` covered every case but one: the
 * tray's `open-settings` and the menu's Settings item both unmount `PanelShell`
 * and this component with it, so a project opened from Explorer while the
 * settings view was up revoked the URL with nothing listening — and coming back
 * remounted the overlay over a blob that no longer resolves. At module scope the
 * reaction outlives the component, which is the only place it can be correct.
 *
 * `useAttachments()` is called lazily inside the getter rather than at module
 * evaluation, so this file's import does not have to be ordered against a
 * composable graph it is not otherwise part of.
 */
watch(
	() => useAttachments().previewEpoch.value,
	() => close(),
)

export function useImageViewer() {
	return {
		attachment: readonly(attachment),
		image: readonly(image),
		isOpen,
		open,
		close,
		reportBrokenImage,
	}
}
