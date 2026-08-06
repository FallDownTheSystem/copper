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
	 * The one chord that fires **from the composer**, and the documented exception
	 * to the suppression rule below. "I am mid-thought, typing, and I want this and
	 * the next five captures to go somewhere else" is the entire use case, so a
	 * binding the composer swallowed would be a binding for nothing.
	 *
	 * `Ctrl+Shift+K` was considered and rejected: it is the browser devtools
	 * console chord.
	 */
	switchSection: {
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
	remove: {
		display: 'Delete',
		matches: (event) => event.key === 'Delete' && !ctrl(event) && !event.shiftKey,
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
 * The composer specifically, as opposed to the other two text surfaces.
 *
 * `Ctrl+K` is the only chord allowed through the guard above, and only from
 * here: inside the inline note editor and the search input it stays suppressed,
 * because neither is a place where "where does the next capture land" is the
 * question being asked.
 */
export function inComposer(target: EventTarget | null): boolean {
	return target instanceof HTMLElement && target.closest('[data-composer]') !== null
}

/**
 * An open menu owns the keyboard while it is up.
 *
 * Its content is portalled inside the panel root, so a keypress inside it still
 * bubbles to the shell — and without this a `Delete` typed at an open menu would
 * delete the notes *and* leave the menu standing.
 */
export function inOverlay(target: EventTarget | null): boolean {
	return (
		target instanceof HTMLElement &&
		target.closest('[data-slot="context-menu-content"], [data-slot="dropdown-menu-content"]') !==
			null
	)
}
