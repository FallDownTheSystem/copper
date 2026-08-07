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
 * One note as a Markdown list item: the first line after the marker, every
 * continuation line indented two spaces so it stays inside the item, and a blank
 * line left genuinely blank rather than becoming two spaces of trailing
 * whitespace.
 */
function item(body: string, marker = '- '): string {
	const [first = '', ...rest] = body.split('\n')
	return [`${marker}${first}`, ...rest.map((line) => (line.length > 0 ? `  ${line}` : ''))].join(
		'\n',
	)
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
 * state; a body is embedded as-is, because it is already Markdown and escaping it
 * would corrupt every fence and table in the space.
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
