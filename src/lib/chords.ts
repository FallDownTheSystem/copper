/**
 * Every in-panel chord, as one table of display string plus matcher.
 *
 * One entry per action so a menu's hint cannot drift from the handler that
 * actually fires — they are the same object. It is also the shape a Phase 7
 * settings layer would read from, if in-panel actions ever become rebindable.
 *
 * All of these are WebView-scoped and unrelated to the two global hotkeys
 * task-005 owns.
 */

export type Chord = {
	/** What the menu shows, right-aligned. */
	display: string
	/** A second chord that fires the same action. The menus keep teaching
	 *  `display` — one hint per action — so an alias surfaces only where every
	 *  chord is listed, in `ShortcutReference`. */
	alias?: string
	matches: (event: KeyboardEvent) => boolean
}

function ctrl(event: KeyboardEvent) {
	return event.ctrlKey || event.metaKey
}

/** `event.key` for a letter is uppercase while Shift is held, so the comparison
 *  has to be case-folded rather than testing `'c'`. */
function letter(event: KeyboardEvent, value: string) {
	return event.key.toLowerCase() === value
}

export const CHORDS = {
	copy: {
		display: 'Ctrl+C',
		matches: (event) => ctrl(event) && !event.shiftKey && letter(event, 'c'),
	},
	copyAsList: {
		display: 'Ctrl+Shift+C',
		matches: (event) => ctrl(event) && event.shiftKey && letter(event, 'c'),
	},
	markDone: {
		display: 'Space',
		matches: (event) => event.key === ' ' && !ctrl(event) && !event.shiftKey,
	},
	edit: {
		display: 'Enter',
		matches: (event) => event.key === 'Enter' && !ctrl(event) && !event.shiftKey,
	},
	merge: {
		display: 'Ctrl+Shift+M',
		matches: (event) => ctrl(event) && event.shiftKey && letter(event, 'm'),
	},
	/**
	 * The one chord that fires **from every surface**, text ones included, and the
	 * only exception to the suppression rule below.
	 *
	 * It was the section switcher's, where the exception was narrower: the composer
	 * only, because "I am mid-thought, typing, and I want this and the next five
	 * captures to go somewhere else" was the whole use case and the search field
	 * and the inline editor are not places that question is asked. Task-019 gave
	 * the binding to the command palette, and that argument does not survive the
	 * change of meaning — "open the command palette" is asked from anywhere, and a
	 * palette the search field swallowed would be a palette with a hole in it. So
	 * the exception widened to every surface and `inComposer()` went with it.
	 *
	 * `Ctrl+Shift+K` was considered and rejected: it is the browser devtools
	 * console chord.
	 */
	commandPalette: {
		display: 'Ctrl+K',
		matches: (event) => ctrl(event) && !event.shiftKey && letter(event, 'k'),
	},
	/**
	 * Deliberately two things by context: on a focused card it starts the
	 * `$EDITOR` handoff, inside the inline editor it commits the edit. The
	 * editor's textarea is a text surface, so the shell's `inTextSurface` guard
	 * resolves the ambiguity before this is ever consulted.
	 */
	openInEditor: {
		display: 'Ctrl+Enter',
		matches: (event) => ctrl(event) && !event.shiftKey && event.key === 'Enter',
	},
	/**
	 * Two chords, one action (user ruling 2026-08-12). Delete stays the primary —
	 * it is what the menus teach — and Ctrl+D is the alias for a hand already on
	 * the letter block, where the Delete key is a reach. `redo` set the precedent
	 * for a two-chord entry; this one carries its second chord as `alias` because
	 * the reference lists both while the menus show one.
	 */
	remove: {
		display: 'Delete',
		alias: 'Ctrl+D',
		matches: (event) =>
			(event.key === 'Delete' && !ctrl(event) && !event.shiftKey) ||
			(ctrl(event) && !event.shiftKey && letter(event, 'd')),
	},
	/**
	 * The keyboard equivalent of a drag: a note travels through its list, a
	 * section header carries its whole section. In the table rather than
	 * hand-matched in the shell so the context menus' Move up / Move down hints
	 * and the handler share one definition.
	 */
	reorderUp: {
		display: 'Alt+↑',
		matches: (event) => event.altKey && !ctrl(event) && !event.shiftKey && event.key === 'ArrowUp',
	},
	reorderDown: {
		display: 'Alt+↓',
		matches: (event) =>
			event.altKey && !ctrl(event) && !event.shiftKey && event.key === 'ArrowDown',
	},
	undo: {
		display: 'Ctrl+Z',
		matches: (event) => ctrl(event) && !event.shiftKey && letter(event, 'z'),
	},
	redo: {
		display: 'Ctrl+Y',
		// Ctrl+Shift+Z is the alias, and it is why this is not simply `letter('y')`.
		matches: (event) =>
			ctrl(event) &&
			((!event.shiftKey && letter(event, 'y')) || (event.shiftKey && letter(event, 'z'))),
	},
} as const satisfies Record<string, Chord>

/**
 * A keypress that belongs to an IME composition, and therefore to the text
 * surface rather than to any chord.
 *
 * Both halves are needed. `isComposing` is the standard signal; WebView2 also
 * reports the legacy `keyCode` 229 while a candidate window is open, and a
 * Japanese, Chinese or Korean user accepting a candidate with Enter would
 * otherwise submit the note, commit the edit or choose a section instead.
 *
 * Every text surface in the panel asks this, which is why it lives beside the
 * chord table rather than in one of them: five hand-written copies of a
 * two-term predicate is five places for the second term to go missing.
 *
 * It takes a `KeyboardEvent` specifically. A `FocusEvent` carries neither
 * field, so a blur handler that needs the same answer has to track the flag on
 * its own session — see `useNoteEditor`.
 */
export function isComposing(event: KeyboardEvent): boolean {
	return event.isComposing || event.keyCode === 229
}

/**
 * The three text-editing surfaces. No in-panel chord fires while one of them has
 * focus, so `Ctrl+Z` undoes typing rather than a note operation and `Ctrl+C`
 * copies the selected query text rather than the notes.
 *
 * The search input is on the list deliberately: leaving it off is the omission
 * that would make editing a query dangerous.
 */
export function inTextSurface(target: EventTarget | null): boolean {
	return target instanceof HTMLElement && target.closest('input, textarea') !== null
}

/**
 * An open menu owns the keyboard while it is up.
 *
 * Its content is portalled inside the panel root, so a keypress inside it still
 * bubbles to the shell — and without this a `Delete` typed at an open menu would
 * delete the notes *and* leave the menu standing.
 *
 * **The command palette is on the list even though it is not a reka menu.** It
 * is rendered inside the panel root rather than portalled, so its presses bubble
 * the same way; the entry is what makes `Delete` at an open palette filter the
 * palette instead of deleting the selection underneath it, and what keeps
 * `Escape` off the shell's ladder so the palette closes without also clearing
 * the query. The selector matches its outermost element, so everything the
 * overlay contains resolves here — including a press that landed on the dialog
 * container itself rather than on the field.
 *
 * **The popover is a reka layer like the menus**, portalled to the same in-clip
 * host, and it owns the keyboard for the same reason: reka moves focus into the
 * open content, so an `Escape` there must close the popover rather than take a
 * rung of the shell's ladder with it.
 */
export function inOverlay(target: EventTarget | null): boolean {
	return (
		target instanceof HTMLElement &&
		target.closest(
			'[data-slot="context-menu-content"], [data-slot="dropdown-menu-content"], [data-slot="popover-content"], [data-slot="command-overlay"]',
		) !== null
	)
}
