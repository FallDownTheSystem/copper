import { describe, expect, it } from 'vite-plus/test'

import { passedThreshold, resolveDrop, type DragLayout } from './dragGeometry'

/**
 * Two sections with a gap between them, which is the shape every interesting
 * question here has: `sec_a` holds two notes, `sec_b` holds one, and 24px of
 * margin sits between the groups belonging to neither. Each section's header
 * occupies its first 16px, so `contentTop` is where its notes begin.
 */
const LAYOUT: DragLayout = {
	sections: [
		{ sectionId: 'sec_a', top: 0, bottom: 100, contentTop: 16 },
		{ sectionId: 'sec_b', top: 124, bottom: 200, contentTop: 140 },
	],
	rows: [
		{ noteId: 'n1', sectionId: 'sec_a', top: 20, bottom: 60 },
		{ noteId: 'n2', sectionId: 'sec_a', top: 64, bottom: 100 },
		{ noteId: 'n3', sectionId: 'sec_b', top: 144, bottom: 180 },
	],
}

describe('resolveDrop', () => {
	it('counts the index over the section without the dragged note in it', () => {
		// `reorder_note` interprets `index` against the destination with the note
		// already removed, so the number counted here has to exclude it too. Holding
		// n1 over the bottom of its own section means "after n2", which is index 1 —
		// not 2, which is what counting n1 itself would give.
		expect(resolveDrop(90, LAYOUT, 'n1')).toMatchObject({ sectionId: 'sec_a', index: 1 })
	})

	it('resolves a note held over its own position to the index it already has', () => {
		// This is what lets the commit path recognise a no-op drag and refuse to push
		// an undo entry for it.
		expect(resolveDrop(40, LAYOUT, 'n1')).toMatchObject({ sectionId: 'sec_a', index: 0 })
		expect(resolveDrop(82, LAYOUT, 'n2')).toMatchObject({ sectionId: 'sec_a', index: 1 })
	})

	it('switches at a row midpoint, not at its edge', () => {
		// n2 spans 64–100, so its centre is 82. Anywhere above that the note goes
		// before it; anywhere below, after.
		expect(resolveDrop(81, LAYOUT, 'n3')).toMatchObject({ index: 1 })
		expect(resolveDrop(83, LAYOUT, 'n3')).toMatchObject({ index: 2 })
	})

	it('crosses into another section, which is what makes a drag a move', () => {
		expect(resolveDrop(150, LAYOUT, 'n1')).toMatchObject({ sectionId: 'sec_b', index: 0 })
		expect(resolveDrop(190, LAYOUT, 'n1')).toMatchObject({ sectionId: 'sec_b', index: 1 })
	})

	it('gives the gap between two sections to the one above it', () => {
		// Nothing is rendered at y=110, so the alternative is resolving to no target
		// at all — which would make the indicator flicker out every time a drag
		// crossed a section boundary.
		expect(resolveDrop(110, LAYOUT, 'n1')).toMatchObject({ sectionId: 'sec_a', index: 1 })
	})

	it('clamps past both ends rather than resolving to nothing', () => {
		// Above the first section: there is no section above it, so it takes the drop.
		expect(resolveDrop(-40, LAYOUT, 'n3')).toMatchObject({ sectionId: 'sec_a', index: 0 })
		// Below everything means "append to the last section", which is how a note is
		// dragged to the very bottom of the list.
		expect(resolveDrop(400, LAYOUT, 'n1')).toMatchObject({ sectionId: 'sec_b', index: 1 })
	})

	it('accepts a drop into a section with no notes', () => {
		const empty: DragLayout = {
			sections: [
				{ sectionId: 'sec_a', top: 0, bottom: 100, contentTop: 16 },
				// Taller than its header, because an empty section renders a
				// "No notes in this section yet." row underneath it.
				{ sectionId: 'sec_b', top: 124, bottom: 170, contentTop: 140 },
			],
			rows: LAYOUT.rows.filter((row) => row.sectionId === 'sec_a'),
		}

		// There is no row to compare against — only the band the section occupies —
		// which is the whole reason sections are measured as well as rows. The line
		// goes just under the header, where the note lands, rather than at the
		// section's bottom edge: that is below the placeholder row the note is about
		// to replace, and a line there promises the wrong destination.
		expect(resolveDrop(150, empty, 'n1')).toEqual({
			sectionId: 'sec_b',
			index: 0,
			indicatorY: 142,
		})
	})

	it('has nowhere to drop when nothing is rendered', () => {
		expect(resolveDrop(10, { sections: [], rows: [] }, 'n1')).toBeNull()
	})

	describe('the insertion line', () => {
		it('sits in the gap between the two rows it goes between', () => {
			// n1 ends at 60 and n2 starts at 64, so the line lands on 62 — reading as
			// "between these" rather than as "on this one".
			expect(resolveDrop(62, LAYOUT, 'n3')?.indicatorY).toBe(62)
		})

		it('sits just clear of the first and last rows at either end', () => {
			expect(resolveDrop(0, LAYOUT, 'n3')?.indicatorY).toBe(18)
			expect(resolveDrop(99, LAYOUT, 'n3')?.indicatorY).toBe(102)
		})

		it('ignores the dragged row when placing itself', () => {
			// Dropping n1 at the end of its own section puts the line under n2 — not
			// in the gap n1 currently occupies, which is about to close up.
			expect(resolveDrop(95, LAYOUT, 'n1')?.indicatorY).toBe(102)
		})
	})
})

describe('passedThreshold', () => {
	it('holds a press that has barely moved', () => {
		expect(passedThreshold(0, 0)).toBe(false)
		expect(passedThreshold(3, 3)).toBe(false)
		expect(passedThreshold(0, -4)).toBe(false)
	})

	it('releases once the travel reaches the threshold, in any direction', () => {
		// Radial, not per-axis: a diagonal 3/4 is a 5px move and has to count.
		expect(passedThreshold(3, 4)).toBe(true)
		expect(passedThreshold(-5, 0)).toBe(true)
		expect(passedThreshold(0, 12)).toBe(true)
	})
})
