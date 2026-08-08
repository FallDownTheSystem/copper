/**
 * How the stored sided modifier tokens read on a key cap.
 *
 * `settings.json` holds `LCtrl` because that is one token the Rust parser can
 * read back, and nobody says "L-Ctrl" out loud. The expansion belongs at display
 * time rather than in the stored spelling: changing what is written to disk to
 * suit a chip would produce a file the app itself could not parse.
 *
 * In `lib/` because it has two readers with no import path between them —
 * `ShortcutRecorder`'s chips and `EmptyState`'s onboarding rows — and a table
 * copied into each would let one teach a spelling the other no longer uses.
 */
const SIDED_LABELS: Record<string, string> = {
	LShift: 'Left Shift',
	RShift: 'Right Shift',
	LCtrl: 'Left Ctrl',
	RCtrl: 'Right Ctrl',
	LAlt: 'Left Alt',
	RAlt: 'Right Alt',
}

/** The cap for one stored token. Safe over a chord's keys too: a chord can never
 *  carry a side, so no token in one is ever in the table. */
export function capLabel(key: string): string {
	return SIDED_LABELS[key] ?? key
}
