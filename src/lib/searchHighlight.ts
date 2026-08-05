/**
 * Search-match highlighting over rendered note bodies, without touching the
 * markup.
 *
 * Bodies are `markdown-it` + `shiki` output cached on the **body string**
 * (`useMarkdown`). Injecting `<mark>` would mean string surgery on highlighted
 * code and would change the very string the cache is keyed on, so the cached
 * HTML would no longer be byte-identical before and after a search. The CSS
 * Custom Highlight API styles arbitrary ranges over the live DOM instead: no
 * markup change, nothing for the cache to notice.
 *
 * `::highlight()` accepts only a small property set — colour, background,
 * text-decoration, text-shadow, -webkit-text-stroke — so the treatment in
 * `main.css` is a background plus a text colour and cannot be a border or a
 * radius.
 *
 * Feature-detected rather than branched into the renderer: where
 * `CSS.highlights` is absent (happy-dom, an older WebView) every call is a
 * no-op and the results are simply not painted.
 */

export const HIGHLIGHT_NAME = 'copper-search-match'

/**
 * One `Highlight` holds ranges from every note at once, so a per-note update has
 * to rebuild the registry entry from all of them. Keyed by element rather than
 * by note id: a row Vue recreated is a different element, and the entry for the
 * old one has to go with it.
 */
const ranges = new Map<HTMLElement, Range[]>()

type HighlightRegistry = {
	set: (name: string, highlight: unknown) => void
	delete: (name: string) => void
}

function registry(): HighlightRegistry | null {
	const css = globalThis.CSS as unknown as { highlights?: HighlightRegistry } | undefined
	// `Highlight` and `CSS.highlights` shipped together, but both are checked:
	// a partial polyfill is worse than an absent one.
	if (!css?.highlights || typeof (globalThis as { Highlight?: unknown }).Highlight !== 'function') {
		return null
	}
	return css.highlights
}

/** Case-insensitive occurrences of `term` in `text`, as offsets. */
function occurrences(text: string, term: string): number[] {
	const found: number[] = []
	if (term.length === 0) return found

	const haystack = text.toLowerCase()
	const needle = term.toLowerCase()
	let from = 0
	for (;;) {
		const at = haystack.indexOf(needle, from)
		if (at === -1) return found
		found.push(at)
		// Advance by one rather than by the term length, so overlapping matches of
		// a repeated term are all painted.
		from = at + 1
	}
}

function collect(root: HTMLElement, terms: string[]): Range[] {
	const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
	const collected: Range[] = []

	for (let node = walker.nextNode(); node; node = walker.nextNode()) {
		const text = node.nodeValue
		if (!text) continue

		for (const term of terms) {
			for (const at of occurrences(text, term)) {
				const range = document.createRange()
				range.setStart(node, at)
				range.setEnd(node, at + term.length)
				collected.push(range)
			}
		}
	}

	return collected
}

function publish(highlights: HighlightRegistry) {
	const all = [...ranges.values()].flat()
	if (all.length === 0) {
		highlights.delete(HIGHLIGHT_NAME)
		return
	}
	const Ctor = (globalThis as unknown as { Highlight: new (...items: Range[]) => unknown })
		.Highlight
	highlights.set(HIGHLIGHT_NAME, new Ctor(...all))
}

/**
 * Repaints one rendered body. Call after render and whenever the terms change;
 * an empty `terms` clears this element's contribution.
 */
export function applyHighlight(root: HTMLElement | null, terms: readonly string[]) {
	const highlights = registry()
	if (!highlights) return

	if (!root || terms.length === 0) {
		if (root) ranges.delete(root)
		else clearDetached()
		publish(highlights)
		return
	}

	clearDetached()
	ranges.set(root, collect(root, [...terms]))
	publish(highlights)
}

/** Drops entries whose element has left the document — an unmounted row would
 *  otherwise keep contributing ranges to the registry forever. */
function clearDetached() {
	for (const element of ranges.keys()) {
		if (!element.isConnected) ranges.delete(element)
	}
}

/** Forgets one element's ranges, for a row that is unmounting. */
export function releaseHighlight(root: HTMLElement | null) {
	const highlights = registry()
	if (!highlights || !root) return
	if (ranges.delete(root)) publish(highlights)
}
