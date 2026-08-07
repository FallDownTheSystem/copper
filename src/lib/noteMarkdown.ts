/**
 * The clipboard payloads. Pure, no Vue imports, and no knowledge of the store —
 * everything here is a function of the notes it is handed.
 *
 * Three of them, not one, and the split is deliberate. `buildCopyMarkdown` and
 * `buildListMarkdown` are body-only and each has a recorded contract that the
 * section-aware renderer would break; `buildSectionMarkdown` is task-013's one
 * renderer, shared verbatim by all three of its scopes. Which notes reach it is
 * the caller's question — this file only formats what it is given, which is what
 * makes the three scopes byte-identical for the same input by construction.
 */

/**
 * `Copy`: the raw Markdown bodies, joined by a blank line.
 *
 * This is byte-for-byte the join task-003's `merge_notes` performs server-side,
 * which is why there is no second `mergeBodies` next to it: the frontend must
 * never reimplement merge, so the only thing that would have called one is this.
 */
export function buildCopyMarkdown(bodies: readonly string[]): string {
	return bodies.join('\n\n')
}

/**
 * `Copy as List`: a flat plain bulleted list, optimised for pasting into an LLM
 * prompt with zero cleanup.
 *
 * Never `[ ]`/`[x]` checkbox syntax and never `## Section` headings, whatever the
 * notes' own `done` state or grouping. That contract is why the section-aware
 * renderer below is a separate function rather than this one growing a mode.
 */
export function buildListMarkdown(bodies: readonly string[]): string {
	return bodies.map((body) => item(body)).join('\n')
}

/**
 * One note as a Markdown list item.
 *
 * The compact form puts the first line after the marker and indents every
 * continuation line two spaces so it stays inside the item, leaving a blank line
 * genuinely blank rather than two spaces of trailing whitespace.
 *
 * **A body that opens a block construct does not fit that form**, because
 * Markdown's block syntax is line-anchored: pushed behind `- [ ] ` a fence, a
 * heading, a blockquote, a nested list or an indented code block stops being one
 * and becomes paragraph text. Measured with the project's own markdown-it, the
 * compact form turns ` ```js ` into an empty `<pre>` with the code leaked out
 * beside it, and — worse, because it consumes the item itself — turns a body
 * whose second line is `===` into `<h1>[ ] Title</h1>` and a body whose second
 * line is a table delimiter into a table whose first header cell is `- [ ] Name`.
 *
 * So such a body goes on continuation lines under a bare marker, separated by a
 * blank line. The blank line is not decoration: without it the marker's own
 * `[ ]` joins the first body line into one paragraph, and the two *retroactive*
 * constructs — a setext underline and a table delimiter row, which redefine the
 * line above them — then swallow the checkbox along with it.
 *
 * The cost is that a task-list extension sees `[ ]` on a line of its own and
 * renders it as literal text rather than a checkbox, so the done state reads as
 * characters instead of a tick for these notes. That is the right way round:
 * losing a note's code fence is a loss of content, and losing its checkbox
 * styling is a loss of polish.
 */
function item(body: string, marker = '- '): string {
	// `\r?\n` rather than `\n`: a body can carry CRLF from a Windows paste or a
	// hand-edited file, and splitting on the newline alone leaves a `\r` at the end
	// of every line — invisible on the clipboard and a stray character in whatever
	// it is pasted into.
	const [first = '', ...rest] = body.split(/\r?\n/)
	const indented = [first, ...rest].map((line) => (line.length > 0 ? `  ${line}` : ''))

	if (opensABlock(first) || redefinesTheLineAbove(rest[0])) {
		return [marker.trimEnd(), '', ...indented].join('\n')
	}
	return [`${marker}${first}`, ...indented.slice(1)].join('\n')
}

/**
 * Constructs that have to begin a line to mean anything.
 *
 * Over-triggering costs three characters of compactness; under-triggering
 * silently corrupts a note. Written to err the first way — `- | -` is caught as a
 * table delimiter below without being one, and nothing is lost by that.
 */
function opensABlock(line: string): boolean {
	return (
		/^ {0,3}(?:```|~~~)/.test(line) || // fenced code
		/^ {0,3}#{1,6}(?:\s|$)/.test(line) || // ATX heading
		/^ {0,3}>/.test(line) || // blockquote
		/^ {0,3}(?:[-+*]|\d{1,9}[.)])(?:\s|$)/.test(line) || // list item
		/^ {0,3}(?:[-*_][ \t]*){3,}$/.test(line) || // thematic break
		/^ {0,3}</.test(line) || // HTML block
		/^ {4,}\S/.test(line) // indented code
	)
}

/**
 * A line that changes what the line *above* it means: a setext underline turns a
 * paragraph into a heading, and a GFM delimiter row turns it into a table header.
 *
 * These are why the first line alone is not enough to decide. A table also has
 * its own opener — a leading `|` — which needs no case here, because every GFM
 * table has a delimiter row on its second line and that is what this catches.
 */
function redefinesTheLineAbove(line: string | undefined): boolean {
	if (line === undefined) return false
	if (/^ {0,3}(?:=+|-+)[ \t]*$/.test(line)) return true
	return /^[\s|:-]+$/.test(line) && line.includes('|') && line.includes('-')
}

/** A section and the notes of it that are in scope, in document order. */
export type MarkdownSection = {
	name: string
	notes: readonly { done: boolean; body: string }[]
}

/**
 * Task-013's one renderer, shared by the whole-document, selection and
 * single-section copy scopes.
 *
 * Sections are ATX headings and notes are task-list items carrying their done
 * state. A body's *characters* are embedded verbatim — it is already Markdown and
 * escaping it would corrupt every fence and table in the space — but where they
 * are placed inside the item is not free: see `item` for the block constructs
 * that cannot follow a list marker on the same line, and what happens to them
 * when they do.
 *
 * **Attachments are omitted.** A note's `file` is a content-addressed name inside
 * a sidecar directory beside the `.copper`, so a link to it means nothing on any
 * other machine, and `name` is the user's original filename and not unique — so
 * neither renders as something a reader could act on. Naming them in prose would
 * put text into the document that was never in a note. The pasted Markdown is
 * therefore the notes, and the attachments stay where the files are.
 *
 * A section holding nothing is not filtered out here — it renders as its heading
 * alone, which is what keeps an empty section visible in a document-wide copy.
 * Which sections are in scope at all is the caller's question, and so is whether
 * a result with no notes in it is worth writing to the clipboard.
 */
export function buildSectionMarkdown(sections: readonly MarkdownSection[]): string {
	return sections
		.map((section) =>
			[
				`# ${section.name}`,
				...section.notes.map((note) => item(note.body, marker(note.done))),
			].join('\n'),
		)
		.join('\n\n')
}

function marker(done: boolean): string {
	return done ? '- [x] ' : '- [ ] '
}
