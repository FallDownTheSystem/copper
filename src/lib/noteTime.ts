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

/** How the list is ordered inside each section. `manual` is the document's own
 *  order — what a drag and Alt+Arrow write — and the only mode under which
 *  either is permitted. */
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

/** The absolute instant, spelled out. The card carries it as a `title` now that
 *  the line itself is relative — the exact day is the thing "3h ago" gives up,
 *  and it is one hover away rather than gone. */
export function formatCreated(created: string | null | undefined): string | null {
	const at = parseCreated(created)
	return at === null ? null : CREATED_FORMAT.format(at)
}

/**
 * Built once, beside `CREATED_FORMAT` and for the same reason.
 *
 * `narrow` is what produces "2m ago" rather than "2 minutes ago" — a note footer
 * is a `text-meta` line under a body, and the long form is wider than most of the
 * notes it would sit under. `numeric: 'always'` rather than `'auto'`: the
 * idiomatic forms `'auto'` reaches for are only defined at ±1 and 0, so a list
 * would read "5m ago, 2h ago, yesterday, 3d ago, last week" — one row in six
 * written in a different register. One shape for every row is worth more here
 * than "yesterday" is.
 */
const RELATIVE_FORMAT = new Intl.RelativeTimeFormat(undefined, {
	numeric: 'always',
	style: 'narrow',
})

const SECOND = 1000
const MINUTE = 60 * SECOND
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

/**
 * The ladder, largest first. Each step's threshold is its own length, so the unit
 * chosen is the largest one that has completed at least once.
 *
 * Months and years are the calendar's, approximated — 30 and 365 days. Nothing
 * downstream measures anything with these; they pick a word and a numeral for a
 * footer, and a note two years old reading "2y ago" a day early is not a claim
 * anybody can act on. `sortByCreated` is where real instants matter, and it never
 * comes through here.
 */
const STEPS: { unit: Intl.RelativeTimeFormatUnit; ms: number }[] = [
	{ unit: 'year', ms: 365 * DAY },
	{ unit: 'month', ms: 30 * DAY },
	{ unit: 'week', ms: 7 * DAY },
	{ unit: 'day', ms: DAY },
	{ unit: 'hour', ms: HOUR },
	{ unit: 'minute', ms: MINUTE },
	{ unit: 'second', ms: SECOND },
]

/**
 * How long ago the note was written, in the largest unit that has run at least
 * once — "2m ago", "3h ago", "2w ago". Null on the same terms as `formatCreated`:
 * a `created` that cannot be read produces nothing rather than a guess.
 *
 * **`now` is a parameter rather than a call to `Date.now()` inside**, which is
 * what makes this a pure function of two instants: every card on screen formats
 * against the same tick (see `useRelativeTime`), so two notes captured in the same
 * second cannot render as "0s ago" and "1s ago" because they were formatted a
 * millisecond apart — and the tests need no clock control to be exact.
 *
 * **Truncated toward zero, never rounded.** Rounding would call a note written
 * 1m50s ago "2m ago", which claims more time has passed than has; truncating
 * says "1m ago" until the second minute is genuinely complete. Below one second
 * that leaves "0s ago", which is the honest reading of a note that was written
 * now and is replaced by the next tick.
 */
export function formatRelative(created: string | null | undefined, now: number): string | null {
	const at = parseCreated(created)
	if (at === null) return null

	// Negative into the past, which is the sign `RelativeTimeFormat` reads as "ago".
	const elapsed = at - now
	const magnitude = Math.abs(elapsed)
	const step = STEPS.find((candidate) => magnitude >= candidate.ms) ?? STEPS[STEPS.length - 1]!

	return RELATIVE_FORMAT.format(Math.trunc(elapsed / step.ms), step.unit)
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
