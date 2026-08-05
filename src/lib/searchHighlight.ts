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

/**
 * Case-insensitive occurrences of `term` in `text`, as offsets **into `text`**.
 *
 * Searching a wholesale-lowercased copy is wrong for the same reason it is
 * tempting: `String.prototype.toLowerCase` is not length-preserving. `İ` folds to
 * two code units and `ẞ` to `ss`, so every offset after one of those in the same
 * text node is shifted, and the range lands on the wrong characters — or throws
 * for running off the end. Each candidate window is folded on its own instead, so
 * an offset is always an index into the original string.
 */
function occurrences(text: string, term: string): number[] {
	const found: number[] = []
	if (term.length === 0 || term.length > text.length) return found

	const needle = term.toLowerCase()
	const haystack = text.toLowerCase()

	// The fast path, and the one essentially every body takes: folding left the
	// length alone, so an offset into the folded string is an offset into the
	// original and the engine's own substring search can be used. Advancing by one
	// rather than by the term length keeps overlapping matches of a repeated term.
	if (haystack.length === text.length) {
		for (let at = haystack.indexOf(needle); at !== -1; at = haystack.indexOf(needle, at + 1)) {
			found.push(at)
		}
		return found
	}

	// Otherwise fold each window on its own. Slower, and always right. A match
	// that itself spans a length-changing fold is not found by a fixed-width
	// window and simply goes unpainted — which is the safe direction, since the
	// alternative is a range over the wrong characters.
	for (let at = 0; at + term.length <= text.length; at++) {
		if (text.slice(at, at + term.length).toLowerCase() === needle) found.push(at)
	}
	return found
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

/**
 * Rebuilds the registry entry from every element's ranges.
 *
 * `new Highlight(...all)` is not an option: the ranges of a few hundred rendered
 * bodies reach the engine's argument-count limit, and a spread that large throws
 * rather than degrading. Built with `add` in a loop, the cost is linear in the
 * ranges and bounded by nothing but memory.
 */
function publish(highlights: HighlightRegistry) {
	for (const element of ranges.keys()) {
		if (!element.isConnected) ranges.delete(element)
	}

	const Ctor = (globalThis as unknown as { Highlight: new () => { add: (range: Range) => void } })
		.Highlight
	const highlight = new Ctor()
	let count = 0
	for (const list of ranges.values()) {
		for (const range of list) {
			highlight.add(range)
			count++
		}
	}

	if (count === 0) highlights.delete(HIGHLIGHT_NAME)
	else highlights.set(HIGHLIGHT_NAME, highlight)
}

/**
 * One publish per flush, however many bodies wrote this tick.
 *
 * Every rendered body has its own watcher, so a keystroke calls
 * `applyHighlight` once per body — and a publish that walked the whole map each
 * time would make a keystroke quadratic in the number of notes. Collecting is
 * per-body and cheap; publishing is shared.
 */
let publishScheduled = false

function schedulePublish() {
	if (publishScheduled) return
	publishScheduled = true
	queueMicrotask(() => {
		publishScheduled = false
		const highlights = registry()
		if (highlights) publish(highlights)
	})
}

/**
 * Records one rendered body's matches. Call after render and whenever the terms
 * change; an empty `terms` clears this element's contribution.
 *
 * Writes to the map only — the registry is updated once for the whole flush.
 */
export function applyHighlight(root: HTMLElement | null, terms: readonly string[]) {
	if (!registry()) return

	if (!root) {
		schedulePublish()
		return
	}
	if (terms.length === 0) ranges.delete(root)
	else ranges.set(root, collect(root, [...terms]))

	schedulePublish()
}

/** Forgets one element's ranges, for a row that is unmounting. */
export function releaseHighlight(root: HTMLElement | null) {
	if (!registry() || !root) return
	if (ranges.delete(root)) schedulePublish()
}
