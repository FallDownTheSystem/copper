/**
 * The auto-growing textarea, shared by the composer and the inline editor.
 *
 * `field-sizing: content` is the clean answer but landed in Chromium 123, while
 * the build targets chrome105 and Evergreen WebView2 cannot be pinned to one
 * runtime version. The fallback must not reset height and read `scrollHeight`
 * per keystroke — that forces synchronous layout on every character — so it
 * measures inside a `requestAnimationFrame` instead.
 */
export function useAutoSize(
	textarea: Readonly<ShallowRef<HTMLTextAreaElement | null>>,
	options: { maxLines?: number } = {},
) {
	const supportsFieldSizing =
		typeof CSS !== 'undefined' && CSS.supports?.('field-sizing', 'content') === true

	let sizingFrame = 0

	function scheduleAutoSize() {
		if (supportsFieldSizing) return
		cancelAnimationFrame(sizingFrame)
		sizingFrame = requestAnimationFrame(() => {
			const element = textarea.value
			if (!element) return
			element.style.height = 'auto'
			element.style.height = `${Math.min(element.scrollHeight, maxHeight(element, options.maxLines))}px`
		})
	}

	onBeforeUnmount(() => cancelAnimationFrame(sizingFrame))

	return { supportsFieldSizing, scheduleAutoSize }
}

/** Measured, not assumed: the CSS cap is `5lh`, and a hardcoded pixel equivalent
 *  drifts from it at 200% browser zoom and at any user font size. */
function maxHeight(element: HTMLTextAreaElement, maxLines: number | undefined) {
	if (maxLines === undefined) return Number.POSITIVE_INFINITY

	const style = getComputedStyle(element)
	const lineHeight = Number.parseFloat(style.lineHeight)
	if (!Number.isFinite(lineHeight)) return Number.POSITIVE_INFINITY

	const vertical =
		Number.parseFloat(style.paddingTop) +
		Number.parseFloat(style.paddingBottom) +
		Number.parseFloat(style.borderTopWidth) +
		Number.parseFloat(style.borderBottomWidth)
	return lineHeight * maxLines + vertical
}
