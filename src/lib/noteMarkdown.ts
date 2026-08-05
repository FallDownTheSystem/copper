/**
 * The two clipboard payloads. Pure, no Vue imports, and no knowledge of the
 * store — everything here is a function of the note bodies it is handed.
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
 * notes' own `done` state or grouping. Continuation lines of a multi-line body
 * are indented two spaces so they stay inside their item; a blank line stays
 * genuinely blank rather than becoming two spaces of trailing whitespace.
 */
export function buildListMarkdown(bodies: readonly string[]): string {
	return bodies
		.map((body) => {
			const [first = '', ...rest] = body.split('\n')
			return [`- ${first}`, ...rest.map((line) => (line.length > 0 ? `  ${line}` : ''))].join('\n')
		})
		.join('\n')
}
