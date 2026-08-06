import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test'

import { noteRow, sectionRow, useSelection } from './useSelection'
import type { Note, Space } from './useSpace'

function note(id: string, section: string, order: number): Note {
	return {
		id,
		section,
		order,
		done: false,
		body: id,
		created: '2026-08-05T00:00:00Z',
		updated: '2026-08-05T00:00:00Z',
	}
}

/** Two sections, so range extension has a boundary to cross. */
function document2(
	noteIds: [string[], string[]] = [
		['n1', 'n2'],
		['n3', 'n4'],
	],
): Space {
	return {
		id: 'spc_1',
		name: 'development',
		activeSection: 'sec_a',
		sections: [
			{ id: 'sec_a', name: 'Research', order: 0 },
			{ id: 'sec_b', name: 'Inbox', order: 1 },
		],
		notes: [
			...noteIds[0].map((id, index) => note(id, 'sec_a', index)),
			...noteIds[1].map((id, index) => note(id, 'sec_b', index)),
		],
	}
}

const selection = useSelection()

beforeEach(() => {
	selection.resetForNewSpace()
	selection.syncDocument(document2())
})

describe('the two orders', () => {
	it('puts header rows in rowIds and keeps them out of visibleNoteIds', () => {
		expect(selection.rowIds.value).toEqual([
			sectionRow('sec_a'),
			noteRow('n1'),
			noteRow('n2'),
			sectionRow('sec_b'),
			noteRow('n3'),
			noteRow('n4'),
		])
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n2', 'n3', 'n4'])
	})
})

describe('select', () => {
	it('replaces the selection and sets both focus and the anchor', () => {
		selection.select('n1')
		selection.select('n3')

		expect(selection.selectedIds.value).toEqual(['n3'])
		expect(selection.focusedId.value).toBe(noteRow('n3'))
		expect(selection.anchorId.value).toBe('n3')
	})
})

describe('toggle', () => {
	it('adds then removes without clearing the rest', () => {
		selection.select('n1')
		selection.toggle('n3')
		expect(selection.selectedIds.value).toEqual(['n1', 'n3'])

		selection.toggle('n3')
		expect(selection.selectedIds.value).toEqual(['n1'])
	})
})

describe('extendTo', () => {
	it('produces a contiguous range across a section boundary', () => {
		selection.select('n2')
		selection.extendTo('n4')

		expect(selection.selectedIds.value).toEqual(['n2', 'n3', 'n4'])
		// The anchor stays put, so extending again grows from the same origin.
		expect(selection.anchorId.value).toBe('n2')
		expect(selection.focusedId.value).toBe(noteRow('n4'))
	})

	it('shrinks back towards the anchor rather than accumulating', () => {
		selection.select('n1')
		selection.extendTo('n4')
		selection.extendTo('n2')

		expect(selection.selectedIds.value).toEqual(['n1', 'n2'])
	})
})

describe('moveFocus', () => {
	it('traverses header rows and selects only when it lands on a note', () => {
		selection.focusRow(sectionRow('sec_a'))
		selection.moveFocus(1)
		expect(selection.focusedId.value).toBe(noteRow('n1'))
		expect(selection.selectedIds.value).toEqual(['n1'])

		selection.moveFocus(1)
		selection.moveFocus(1)
		// Landed on the second section's header: selection is left alone.
		expect(selection.focusedId.value).toBe(sectionRow('sec_b'))
		expect(selection.selectedIds.value).toEqual(['n2'])
	})

	it('clamps at both ends rather than wrapping', () => {
		selection.focusRow(sectionRow('sec_a'))
		selection.moveFocus(-1)
		expect(selection.focusedId.value).toBe(sectionRow('sec_a'))

		selection.focusLast()
		selection.moveFocus(1)
		expect(selection.focusedId.value).toBe(noteRow('n4'))
	})
})

describe('extendFocus', () => {
	it('moves between notes only, skipping the header between them', () => {
		selection.select('n2')
		selection.extendFocus(1)

		expect(selection.focusedId.value).toBe(noteRow('n3'))
		expect(selection.selectedIds.value).toEqual(['n2', 'n3'])
	})

	it('reaches the adjacent note when focus is on a section header', () => {
		selection.focusRow(sectionRow('sec_b'))
		selection.extendFocus(1)

		// Falling back to index 0 would jump the selection to the top of the
		// document from anywhere in the list.
		expect(selection.focusedId.value).toBe(noteRow('n3'))
		expect(selection.selectedIds.value).toEqual(['n3'])
	})

	it('reaches backwards past a header to the preceding note', () => {
		selection.focusRow(sectionRow('sec_b'))
		selection.extendFocus(-1)

		expect(selection.focusedId.value).toBe(noteRow('n2'))
	})
})

describe('selectAll and clear', () => {
	it('selects every rendered note and then clears the anchor too', () => {
		selection.selectAll()
		expect(selection.selectedIds.value).toEqual(['n1', 'n2', 'n3', 'n4'])

		selection.clear()
		expect(selection.selectedIds.value).toEqual([])
		expect(selection.anchorId.value).toBeNull()
	})
})

describe('reconcile', () => {
	it('prunes selected ids that no longer exist and clears a deleted anchor', () => {
		selection.select('n2')
		selection.extendTo('n4')
		const snapshot = selection.snapshot()

		selection.syncDocument(document2([['n1'], ['n4']]))
		selection.reconcile(snapshot)

		expect(selection.selectedIds.value).toEqual(['n4'])
		expect(selection.anchorId.value).toBeNull()
	})

	it('relocates focus to the nearest survivor by the former flattened index', () => {
		selection.select('n2')
		const snapshot = selection.snapshot()

		// n2 is gone; n3 was the next in the former order and survives.
		selection.syncDocument(document2([['n1'], ['n3', 'n4']]))
		selection.reconcile(snapshot)

		expect(selection.focusedId.value).toBe(noteRow('n3'))
	})

	it('falls back to a preceding survivor when nothing after it remains', () => {
		selection.select('n3')
		const snapshot = selection.snapshot()

		selection.syncDocument(document2([['n1', 'n2'], []]))
		selection.reconcile(snapshot)

		expect(selection.focusedId.value).toBe(noteRow('n2'))
	})

	it('follows a note by id when it moved to another section', () => {
		selection.select('n3')
		const snapshot = selection.snapshot()

		selection.syncDocument(document2([['n1', 'n2', 'n3'], ['n4']]))
		selection.reconcile(snapshot)

		expect(selection.focusedId.value).toBe(noteRow('n3'))
		expect(selection.selectedIds.value).toEqual(['n3'])
	})

	it('refocuses when the focused row was recreated under another rowgroup', () => {
		// A row that moved between sections is a *new* element carrying the same
		// id, and focus did not move with it — matching by id alone reported
		// "still there" while document.activeElement had fallen back to the body.
		document.body.innerHTML = '<div data-row-id="n:n3" tabindex="0"></div>'
		const moved = document.body.firstElementChild as HTMLElement
		moved.focus()

		selection.select('n3')
		const snapshot = selection.snapshot()
		expect(snapshot.activeElement).toBe(moved)

		// Vue tears the old node out and builds a fresh one with the same id.
		document.body.innerHTML = '<div data-row-id="n:n3" tabindex="0"></div>'
		selection.syncDocument(document2([['n1', 'n2', 'n3'], ['n4']]))
		selection.reconcile(snapshot)
		selection.restoreDom(snapshot)

		expect(document.activeElement).toBe(document.body.firstElementChild)
		document.body.innerHTML = ''
	})

	it('gives the grid a roving target on first load without selecting anything', () => {
		selection.resetForNewSpace()
		const snapshot = selection.snapshot()
		selection.syncDocument(document2())
		selection.reconcile(snapshot)

		// Without this every row renders tabindex="-1" and the list cannot be
		// reached by Tab at all.
		expect(selection.focusedId.value).toBe(noteRow('n1'))
		expect(selection.selectedIds.value).toEqual([])
	})

	it('leaves nothing focused when the space has no rows at all', () => {
		selection.select('n1')
		const snapshot = selection.snapshot()

		selection.syncDocument(null)
		selection.reconcile(snapshot)

		expect(selection.focusedId.value).toBeNull()
		expect(selection.selectedIds.value).toEqual([])
	})
})

describe('the scroll anchor', () => {
	/**
	 * happy-dom lays nothing out, so the three scroll metrics are installed by
	 * hand — which is enough, because the bottom test reads exactly those three
	 * and nothing else. The object is handed back so a test can grow the region
	 * the way an added row does and then restore against it.
	 */
	function mountRegion(metrics: { scrollTop: number; scrollHeight: number; clientHeight: number }) {
		const region = document.createElement('div')
		region.setAttribute('data-scroll-region', '')
		Object.defineProperty(region, 'scrollHeight', {
			configurable: true,
			get: () => metrics.scrollHeight,
		})
		Object.defineProperty(region, 'clientHeight', {
			configurable: true,
			get: () => metrics.clientHeight,
		})
		Object.defineProperty(region, 'scrollTop', {
			configurable: true,
			get: () => metrics.scrollTop,
			set: (value: number) => {
				metrics.scrollTop = value
			},
		})
		document.body.append(region)
		return region
	}

	afterEach(() => {
		document.body.innerHTML = ''
	})

	it('takes the bottom edge itself as the anchor when the list is already there', () => {
		mountRegion({ scrollTop: 380, scrollHeight: 500, clientHeight: 120 })

		// Not a note plus an offset: holding the topmost visible note's offset is
		// precisely what left a freshly added note below the fold.
		expect(selection.snapshot().scroll).toEqual({ kind: 'bottom' })
	})

	it('counts a sub-pixel remainder as the bottom', () => {
		// At a fractional device pixel ratio the three metrics do not cancel
		// exactly, so an exact equality test never fires on a real display.
		mountRegion({ scrollTop: 378.5, scrollHeight: 500, clientHeight: 120 })

		expect(selection.snapshot().scroll).toEqual({ kind: 'bottom' })
	})

	it('keeps the pin when the composer grows under the region', () => {
		// The defect this exists for, measured in a real browser: typing into the
		// composer expands it, which shrinks the scroll region by the same amount
		// without moving `scrollTop`. The arithmetic then reports 70px from the
		// bottom for a reader who never scrolled, and a capture taken at that
		// instant — submit is exactly when the composer is tallest — took a note
		// anchor and left the new note below the fold.
		const metrics = { scrollTop: 380, scrollHeight: 500, clientHeight: 120 }
		mountRegion(metrics)
		expect(selection.snapshot().scroll).toEqual({ kind: 'bottom' })

		metrics.clientHeight = 50
		expect(selection.snapshot().scroll).toEqual({ kind: 'bottom' })
	})

	it('releases the pin when the reader actually scrolls up', () => {
		const metrics = { scrollTop: 380, scrollHeight: 500, clientHeight: 120 }
		const region = mountRegion(metrics)
		region.innerHTML = '<div data-row-id="n:n1"></div><div data-row-id="n:n2"></div>'
		expect(selection.snapshot().scroll).toEqual({ kind: 'bottom' })

		// A scroll event fires only when `scrollTop` genuinely moves, which is what
		// separates a reader from a composer growing underneath them.
		metrics.scrollTop = 40
		region.dispatchEvent(new Event('scroll'))

		expect(selection.snapshot().scroll).toEqual({ kind: 'note', noteId: 'n1', offset: 0 })
	})

	it('re-arms at the bottom without waiting for a scroll event', () => {
		const metrics = { scrollTop: 40, scrollHeight: 500, clientHeight: 120 }
		mountRegion(metrics)
		expect(selection.snapshot().scroll).not.toEqual({ kind: 'bottom' })

		// Sufficient but not necessary: re-arming must not depend on an event
		// having been delivered, or a missed one strands the list unstuck forever.
		metrics.scrollTop = 380
		expect(selection.snapshot().scroll).toEqual({ kind: 'bottom' })
	})

	it('ignores the scroll events the list’s own reflow produces', () => {
		// Measured in WebView2: clamping a freshly measured note shrinks and regrows
		// the list several times over ~180ms, and every step fires a scroll event.
		// Treating those as a reader gave up the pin halfway through the cascade and
		// left the list 7px short, which then classified the next capture as
		// scrolled-up and killed stickiness for good.
		const metrics = { scrollTop: 380, scrollHeight: 500, clientHeight: 120 }
		const region = mountRegion(metrics)
		const snapshot = selection.snapshot()
		selection.restoreDom(snapshot)

		metrics.scrollHeight = 700
		region.dispatchEvent(new Event('scroll'))

		expect(selection.snapshot().scroll).toEqual({ kind: 'bottom' })
	})

	it('hands the list back on a reader’s own gesture mid-settle', () => {
		const metrics = { scrollTop: 380, scrollHeight: 500, clientHeight: 120 }
		const region = mountRegion(metrics)
		region.innerHTML = '<div data-row-id="n:n1"></div>'
		selection.restoreDom(selection.snapshot())

		// A wheel, unlike a reflow, is the reader taking the list back.
		region.dispatchEvent(new Event('wheel'))
		metrics.scrollTop = 40
		region.dispatchEvent(new Event('scroll'))

		expect(selection.snapshot().scroll).toEqual({ kind: 'note', noteId: 'n1', offset: 0 })
	})

	it('keeps pinning across the plateau while the inserted row is still animating', async () => {
		const frames = async (count: number) => {
			for (let i = 0; i < count; i++) {
				await new Promise((resolve) => requestAnimationFrame(resolve))
			}
		}

		const metrics = { scrollTop: 380, scrollHeight: 500, clientHeight: 120 }
		const region = mountRegion(metrics)
		// auto-animate parks a new row at `scale(.98)` until its entry animation is
		// half over, so the list holds perfectly still for ~110ms and only then
		// grows. Measured in WebView2, a settle loop that exited on that stillness
		// stopped before the growth and left `scrollTop` 12.57px below the true
		// maximum — the note flush against the viewport with the list's bottom
		// padding stranded underneath. Reporting a running animation is what
		// carries the loop across the plateau.
		region.getAnimations = () => [{ playState: 'running' }] as unknown as Animation[]

		selection.restoreDom(selection.snapshot())
		await frames(8)

		metrics.scrollHeight = 700
		await frames(8)

		expect(metrics.scrollTop).toBe(700)
	})

	it('re-pins to the bottom after the list grew, so the new note is on screen', () => {
		const metrics = { scrollTop: 380, scrollHeight: 500, clientHeight: 120 }
		mountRegion(metrics)
		const snapshot = selection.snapshot()

		// The added row, measured after the patch.
		metrics.scrollHeight = 560
		selection.restoreDom(snapshot)

		expect(metrics.scrollTop).toBe(560)
	})

	it('leaves a reader who has scrolled up exactly where they were', () => {
		const metrics = { scrollTop: 40, scrollHeight: 500, clientHeight: 120 }
		const region = mountRegion(metrics)
		region.innerHTML = '<div data-row-id="n:n1"></div>'
		const snapshot = selection.snapshot()

		metrics.scrollHeight = 560
		selection.restoreDom(snapshot)

		// The note anchor resolves to a zero delta under happy-dom's null layout,
		// so what this pins down is that the bottom pin did not fire.
		expect(metrics.scrollTop).toBe(40)
	})
})
