/**
 * Arrow-key focus movement inside a confirm popover, shared by the three the
 * panel has: the section-delete confirm, the done-delete scope pick and the
 * list-paste question.
 *
 * Each of those opens with focus explicitly placed on its safe control, so the
 * other offer is one arrow away rather than a Tab — arrows are how every menu
 * in the panel moves, and a popover that ignored them read as stuck. All four
 * arrows step for the same reason both axes exist in the wild: one popover
 * lays its offers in a row, the others in a column, and the caller should not
 * have to say which.
 *
 * The walk cycles and skips disabled controls, exactly as reka's menus do.
 * Everything that is not an arrow is left alone — Escape stays reka's,
 * Enter/Space stay the focused button's.
 */
export function moveFocusOnArrow(event: KeyboardEvent) {
	const forward = event.key === 'ArrowRight' || event.key === 'ArrowDown'
	if (!forward && event.key !== 'ArrowLeft' && event.key !== 'ArrowUp') return

	const content = event.currentTarget
	if (!(content instanceof HTMLElement)) return
	const buttons = [...content.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')]
	if (buttons.length === 0) return

	event.preventDefault()
	const held = buttons.indexOf(document.activeElement as HTMLButtonElement)
	// Focus outside the buttons — reka's content wrapper, say — enters the walk
	// at whichever end the arrow points from.
	const next =
		held === -1
			? forward
				? 0
				: buttons.length - 1
			: (held + (forward ? 1 : -1) + buttons.length) % buttons.length
	buttons[next]?.focus()
}
