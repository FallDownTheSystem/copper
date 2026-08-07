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

/** Discards a read whose viewer has since been closed or re-pointed. Two
 *  double-clicks a moment apart are two reads, and the loser must not paint over
 *  the winner. */
let session = 0

const isOpen = computed(() => attachment.value !== null)

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

	const target = invoker
	invoker = null
	// After the overlay has come down, or focus lands on an element that is about
	// to be unmounted and falls back to the body — which puts the keyboard outside
	// the panel root, where the Escape ladder never sees it.
	if (target?.isConnected) void nextTick(() => target.focus())
}

export function useImageViewer() {
	return {
		attachment: readonly(attachment),
		image: readonly(image),
		isOpen,
		open,
		close,
	}
}
