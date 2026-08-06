/**
 * The section-name normalisation, mirrored from Rust for **display and
 * filtering only**.
 *
 * `src-tauri/src/entry.rs` is the authority: it is what actually decides the
 * name a section is stored under and which existing names a new one collides
 * with, and nothing here changes that. This copy exists because the switcher has
 * to show the user the name they are about to get *before* the store sees it —
 * offering `Create section "Deep  Research"` and then storing `Deep Research` is
 * a promise the UI cannot keep, and filtering the list on the raw text made
 * `Deep  Research` miss the existing `Deep Research`, offer to create it, and
 * then silently activate the existing one instead.
 *
 * Keep the two in step. If the Rust rule changes, this changes with it.
 */

/** `entry::SECTION_NAME_MAX`. */
export const SECTION_NAME_MAX = 80

export function normaliseSectionName(name: string): string {
	const collapsed = name.split(/\s+/).filter(Boolean).join(' ')
	// `Array.from`, not `String.slice`, so the cap counts **code points** exactly
	// as Rust's `chars()` does — `slice` counts UTF-16 units and would cut an emoji
	// in half. Not graphemes either, in either language: a combining mark at the
	// boundary can still be separated from its base, which is the limitation the
	// no-new-crates rule accepts on the Rust side and this mirrors deliberately.
	// Trimmed again because the cut can land immediately after a space.
	return Array.from(collapsed).slice(0, SECTION_NAME_MAX).join('').trimEnd()
}
