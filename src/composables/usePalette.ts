/**
 * Whether the command palette is up, and what had focus before it was.
 *
 * Module scope, like every other shared piece of view state here: the shell's
 * chord layer opens it, the component renders it and an action inside it closes
 * it, and a ref declared inside the exported function would hand each of those
 * three a private copy.
 *
 * **The focus promise is this module's, not a relay's.** The section switcher
 * returns focus through three components — chip to header to composer — because
 * only the composer knows whether it held the caret. The palette has no such
 * question to ask: it takes `Ctrl+K` from *every* surface, so there is no one
 * component that could know, and the only honest answer is "whatever had focus a
 * moment ago". Recording the element here keeps that answer in one place.
 *
 * What it deliberately does **not** promise is a text caret. Focusing a
 * `<textarea>` does not restore its selection range, and the composer records
 * `{ start, end }` by hand for exactly that reason — a promise the palette does
 * not make, because the surfaces it opens from now include two it knows nothing
 * about.
 */

const isOpen = ref(false)

/** Not a `ref`: nothing renders it, and holding a live DOM node in the
 *  reactivity graph would proxy an element for no reader. */
let restoreFocusTo: HTMLElement | null = null

function open() {
	if (isOpen.value) return
	const active = document.activeElement
	restoreFocusTo = active instanceof HTMLElement ? active : null
	isOpen.value = true
}

/**
 * Closes, and hands focus back on the tick *after* the overlay is gone.
 *
 * The delay is not incidental. The palette's contents sit inside a trapped
 * `FocusScope`, which pulls focus back the moment it lands outside the scope —
 * so focusing the composer while the overlay is still mounted moves focus and
 * then has it snatched straight back. Waiting until the DOM update has removed
 * the scope is what makes the restore stick.
 *
 * `isConnected` because the element may not have survived the palette: an action
 * that switches space replaces the whole document, and the note row that had
 * focus is gone by the time this runs.
 */
function close() {
	if (!isOpen.value) return
	isOpen.value = false
	const target = restoreFocusTo
	restoreFocusTo = null
	if (!target) return
	void nextTick(() => {
		if (target.isConnected) target.focus()
	})
}

export function usePalette() {
	return {
		isOpen: readonly(isOpen),
		open,
		close,
	}
}
