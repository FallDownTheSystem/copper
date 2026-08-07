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
 *
 * **A subsequence match spans text nodes, so this walks the rendered text as one
 * string.** Task-006 looked for literal substrings inside each text node
 * independently, which was enough while the query was a list of words. Task-014's
 * matcher takes the query as one character sequence, and `http req` against
 * `<strong>HTTP</strong> requests` has its match straddling the boundary between
 * two nodes — so the nodes are concatenated, matched once, and the resulting
 * positions mapped back. Ranges are still cut at every node boundary: a `Range`
 * that spanned two nodes would paint everything between them, including markup
 * the user cannot see.
 *
 * **The string matched here is not the string that was scored.** `useNoteSearch`
 * ranks notes by their `body`; this paints the *rendered* text, where markdown
 * syntax is gone and code has been tokenised. There is no mapping between the
 * two, so the match is simply re-run here. A note can therefore rank on one
 * arrangement of characters and be painted on another — which is visible only in
 * bodies whose markup changes the text, and is the honest alternative to painting
 * nothing at all.
 */

import { fuzzyMatch, type MatchSpan } from './fuzzyMatch'

export const HIGHLIGHT_NAME = 'copper-search-match'

/**
 * One `Highlight` holds ranges from every note at once, so the registry entry is
 * rebuilt from this whole map — in `publish`, once per flush, never once per
 * note. Keyed by element rather than by note id: a row Vue recreated is a
 * different element, and the entry for the old one has to go with it.
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

/** One text node and where its text begins in the concatenation. */
type Piece = { node: Node; text: string; start: number }

/** Every text node under `root`, in document order, with the string they spell
 *  between them. */
function pieces(root: HTMLElement): { pieces: Piece[]; text: string } {
	const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
	const found: Piece[] = []
	let text = ''

	for (let node = walker.nextNode(); node; node = walker.nextNode()) {
		const value = node.nodeValue
		if (!value) continue
		found.push({ node, text: value, start: text.length })
		text += value
	}

	return { pieces: found, text }
}

/**
 * Turns matched spans into as few ranges as will cover them.
 *
 * A run is broken by either of two things: a gap in the spans — a subsequence
 * match is mostly gaps — or a node boundary. The second is the one worth
 * stating: `Range` happily spans nodes, and a range from the end of one text
 * node to the start of the next would paint every element in between, which in a
 * rendered body is markup rather than text.
 */
function rangesFor(found: Piece[], spans: readonly MatchSpan[]): Range[] {
	const collected: Range[] = []
	// The spans are ascending, so one cursor walks the pieces alongside them.
	let index = 0
	let runStart = -1
	let runEnd = -1
	let runPiece: Piece | null = null

	const flush = () => {
		if (!runPiece) return
		const range = document.createRange()
		range.setStart(runPiece.node, runStart - runPiece.start)
		range.setEnd(runPiece.node, runEnd - runPiece.start)
		collected.push(range)
		runPiece = null
	}

	for (const span of spans) {
		let piece = found[index]
		while (piece && index < found.length - 1 && span.start >= piece.start + piece.text.length) {
			piece = found[++index]
		}
		if (!piece) break

		if (runPiece === piece && span.start === runEnd) {
			runEnd = span.end
			continue
		}
		flush()
		runPiece = piece
		runStart = span.start
		runEnd = span.end
	}
	flush()

	return collected
}

function collect(root: HTMLElement, needle: string): Range[] {
	const { pieces: found, text } = pieces(root)
	const match = fuzzyMatch(text, needle)
	if (!match) return []
	return rangesFor(found, match.spans)
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
 * Records one rendered body's matches. Call after render and whenever the needle
 * changes; an empty `needle` clears this element's contribution.
 *
 * `needle` is the query already through `fuzzyNeedle` — whitespace stripped and
 * folded. Taking it pre-normalised rather than raw is what keeps the characters
 * painted here and the score `useNoteSearch` ranked by derived from the same
 * sequence.
 *
 * Writes to the map only — the registry is updated once for the whole flush.
 */
export function applyHighlight(root: HTMLElement | null, needle: string) {
	if (!registry()) return

	if (!root) {
		schedulePublish()
		return
	}
	if (needle.length === 0) ranges.delete(root)
	else ranges.set(root, collect(root, needle))

	schedulePublish()
}

/** Forgets one element's ranges, for a row that is unmounting. */
export function releaseHighlight(root: HTMLElement | null) {
	if (!registry() || !root) return
	if (ranges.delete(root)) schedulePublish()
}
