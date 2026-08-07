/**
 * Note creation times: reading them, showing them, and ordering by them.
 *
 * **`created` has been on every note since task-003** (`store/model.rs`), as an
 * RFC3339 UTC string with second precision — `format::now_rfc3339`. Nothing in
 * this task changes the document; this module is the frontend catching up with a
 * field that was always there.
 *
 * The model keeps it a `String` rather than a parsed timestamp on purpose: a
 * hand-edited or malformed value is preserved verbatim rather than making the
 * surrounding document unloadable. That decision reaches all the way here, and is
 * why every function below is total over a string that is not a timestamp at all.
 * `parseCreated` answers `null` rather than throwing or substituting a date, and
 * both callers treat that answer as *unknown* rather than as old or as new —
 * showing nothing, and sorting into a trailing tier. Inventing a plausible date
 * for a note whose own is unreadable would be worse than saying nothing.
 */

/** Per-section order. `manual` is the document's own order — what a drag and
 *  Alt+Arrow write — and the only mode under which either is permitted. */
export type SortMode = 'manual' | 'oldest' | 'newest'

/** Epoch milliseconds, or null when the value is absent or unparseable. */
export function parseCreated(created: string | null | undefined): number | null {
	if (!created) return null
	const at = Date.parse(created)
	return Number.isNaN(at) ? null : at
}

/**
 * Built once. Constructing an `Intl.DateTimeFormat` is the expensive part of
 * formatting, and a note list re-renders this per card.
 *
 * `undefined` locale and no `timeZone`, so both follow the machine — which turns
 * the stored UTC instant into the local wall clock the note was actually taken
 * at. That is the whole of this task's timezone handling: there is no preference
 * to choose and nothing to convert between.
 */
const CREATED_FORMAT = new Intl.DateTimeFormat(undefined, {
	dateStyle: 'medium',
	timeStyle: 'short',
})

/** The line a card shows, or null when there is nothing honest to show. */
export function formatCreated(created: string | null | undefined): string | null {
	const at = parseCreated(created)
	return at === null ? null : CREATED_FORMAT.format(at)
}

/**
 * `ids` reordered by creation time, with the unknown ones trailing in the order
 * they arrived.
 *
 * **The trailing tier is the deliberate part.** Sorting an unreadable `created`
 * to the front under "Oldest first" would assert that the note is old, and to the
 * front under "Newest first" that it is new; both are claims the document does
 * not support. Trailing in file order says only "these could not be placed", is
 * the same answer in both directions, and is where a note whose timestamp someone
 * broke by hand can still be found.
 *
 * Stable in both tiers: `Array.prototype.sort` is stable, so notes sharing a
 * second — which second precision makes ordinary for a burst of captures — keep
 * the document order they came in with rather than reshuffling per render.
 */
export function sortByCreated(
	ids: readonly string[],
	createdAt: (id: string) => number | null,
	mode: Exclude<SortMode, 'manual'>,
): string[] {
	const dated: { id: string; at: number }[] = []
	const undated: string[] = []

	for (const id of ids) {
		const at = createdAt(id)
		if (at === null) undated.push(id)
		else dated.push({ id, at })
	}

	dated.sort((a, b) => (mode === 'oldest' ? a.at - b.at : b.at - a.at))
	return [...dated.map((entry) => entry.id), ...undated]
}
