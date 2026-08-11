/**
 * Whether a pasted clipboard is a flat Markdown-style list, and nothing else —
 * the one shape the zero-focus paste offers to split into separate notes.
 *
 * The rule is deliberately all-or-nothing: **every** non-blank line must be a
 * top-level list item. A heading, a nested item, a paragraph between bullets —
 * any line that is not an item means the text has structure the split would
 * destroy, so the caller gets `null` and the paste stays one note, exactly as
 * it always was. Splitting is only obviously right when every line is a peer.
 *
 * Two items are the floor. A single bullet has nothing to split, and asking
 * would make the popover a toll on ordinary pastes.
 */

/** A top-level item: a bullet (`-`, `*`, `+`) or an ordered marker (`1.`,
 *  `1)`), at column zero, with the space Markdown requires after it. Indented
 *  items are nested and deliberately do not match — the caller refuses the
 *  whole text rather than flattening a hierarchy. */
const ITEM = /^(?:[-*+]|\d{1,9}[.)])\s+(\S.*)$/

/**
 * The item bodies of a flat list, in order, or `null` when the text is
 * anything else.
 *
 * The markers are stripped: the list structure moves into the notes list
 * itself, and a note whose whole body is `- one` would render as a one-item
 * list rather than the line it was. Ordered markers go the same way — the
 * store owns note order, and a body starting `3.` would pin a number that
 * stops being true the moment the notes are reordered.
 */
export function splitFlatList(text: string): string[] | null {
	const items: string[] = []
	for (const line of text.split(/\r?\n/)) {
		// Blank lines separate loose list items and end most clipboards; they say
		// nothing about structure either way.
		if (line.trim().length === 0) continue
		const match = ITEM.exec(line)
		if (!match) return null
		items.push(match[1]!.trimEnd())
	}
	return items.length >= 2 ? items : null
}
