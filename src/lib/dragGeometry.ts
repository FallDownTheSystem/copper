/**
 * Where a pointer drag would drop the note it is carrying.
 *
 * Pure over measured geometry, and deliberately so: the drag itself is a pile of
 * pointer capture, auto-scroll and DOM writes, and none of that is the part that
 * can be wrong in a way a user notices. The part that can is this — which section
 * a Y coordinate falls in, and how many notes sit above it.
 *
 * **Every coordinate is relative to the list root**, the element the sections are
 * rendered inside. That is the one frame in which a number stays true while the
 * region scrolls underneath the pointer: the root scrolls with the content, so a
 * row measured against it at drag start still describes where that row is a
 * second later.
 */

/** A note row that could be dropped next to, measured at drag start. */
export type DragRow = {
	noteId: string
	sectionId: string
	top: number
	bottom: number
}

/** A section's own extent, header included. It is what makes an *empty* section
 *  a possible destination — there is no row to compare against, only the band the
 *  section occupies. */
export type DragSection = {
	sectionId: string
	top: number
	bottom: number
	/** Where the section's *notes* start, below its header row. An empty section
	 *  is the only case that needs it, and it needs it because `bottom` is below
	 *  the "No notes in this section yet." placeholder — which is not where the
	 *  note lands. It lands first, at the top. */
	contentTop: number
}

export type DragLayout = {
	/** In visual order, top to bottom. */
	sections: DragSection[]
	/** In visual order, top to bottom, across every section. */
	rows: DragRow[]
}

export type DropTarget = {
	sectionId: string
	/**
	 * The insertion index within the target section, counted over the rows that
	 * are **not** the dragged note.
	 *
	 * That exclusion is what makes this the number `reorder_note` takes: it
	 * interprets `index` against the destination with the note already removed.
	 */
	index: number
	/** Where to paint the insertion line, in list-root coordinates. */
	indicatorY: number
}

/** How far the pointer travels before a press becomes a drag. Small enough not to
 *  feel sticky, large enough that a click on a 20px-wide grip is never one. */
export const DRAG_ACTIVATION_PX = 5

export function passedThreshold(dx: number, dy: number, threshold = DRAG_ACTIVATION_PX) {
	return dx * dx + dy * dy >= threshold * threshold
}

/**
 * The section a Y coordinate belongs to: the last one that starts at or above it.
 *
 * The gaps *between* sections therefore belong to the section above, and anything
 * past the end of the list belongs to the last section — which is what makes
 * dragging to the very bottom mean "append here" rather than resolving to
 * nothing. Above the first section there is no section above, so the first one
 * takes it.
 */
function sectionAt(y: number, sections: readonly DragSection[]): DragSection | null {
	let found: DragSection | null = null
	for (const section of sections) {
		if (section.top <= y) found = section
	}
	return found ?? sections[0] ?? null
}

/**
 * A line in the gap between two rows rather than on either row's edge, so it
 * reads as "between these two" and not as "on this one".
 */
function indicatorFor(rows: readonly DragRow[], index: number, section: DragSection): number {
	const before = rows[index - 1]
	const after = rows[index]
	if (before && after) return (before.bottom + after.top) / 2
	if (after) return after.top - 2
	if (before) return before.bottom + 2
	// An empty section has no row to sit beside, so the line goes just under its
	// header — where the note will actually land. Not `bottom`: an empty section
	// renders a "No notes in this section yet." row, and a line drawn under that
	// promises the note will arrive below a placeholder it is about to replace.
	return section.contentTop + 2
}

/**
 * Resolves a pointer position to the drop it would perform.
 *
 * The dragged note is filtered out rather than skipped over, because the index
 * this returns is counted in a list it is not part of. Dropping a note back where
 * it started therefore returns its own current index, and the commit path
 * recognises that as the no-op it is.
 */
export function resolveDrop(
	y: number,
	layout: DragLayout,
	draggedNoteId: string,
): DropTarget | null {
	const section = sectionAt(y, layout.sections)
	if (!section) return null

	const rows = layout.rows.filter(
		(row) => row.sectionId === section.sectionId && row.noteId !== draggedNoteId,
	)

	// Midpoints, not edges: the note goes above every row whose centre the pointer
	// has not yet passed. Counting them *is* the insertion index, because the rows
	// arrive in visual order.
	const index = rows.filter((row) => (row.top + row.bottom) / 2 < y).length

	return { sectionId: section.sectionId, index, indicatorY: indicatorFor(rows, index, section) }
}
