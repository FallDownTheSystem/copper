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
 */
export function inOverlay(target: EventTarget | null): boolean {
	return (
		target instanceof HTMLElement &&
		target.closest('[data-slot="context-menu-content"], [data-slot="dropdown-menu-content"]') !==
			null
	)
}
