import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test'
import { nextTick } from 'vue'

import { useSections } from './useSections'
import { flushReveal, noteRow, revealRow, sectionRow, useSelection } from './useSelection'
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
const sections = useSections()

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
	it('traverses header rows, selecting on a note and clearing on a header', () => {
		selection.focusRow(sectionRow('sec_a'))
		selection.moveFocus(1)
		expect(selection.focusedId.value).toBe(noteRow('n1'))
		expect(selection.selectedIds.value).toEqual(['n1'])

		selection.moveFocus(1)
		selection.moveFocus(1)
		// Landed on the second section's header: the selection is cleared, so the
		// note the arrow just left does not keep its ring while the heading wears
		// the focus outline (the 2026-08-10 ruling recorded on `landOn`).
		expect(selection.focusedId.value).toBe(sectionRow('sec_b'))
		expect(selection.selectedIds.value).toEqual([])
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

describe('moveFocusOnly', () => {
	it('moves the roving target and leaves both the selection and the anchor alone', () => {
		selection.select('n1')
		selection.moveFocusOnly(1)

		expect(selection.focusedId.value).toBe(noteRow('n2'))
		expect(selection.selectedIds.value).toEqual(['n1'])
		// The anchor stays where the last deliberate act put it, so a `Shift+Arrow`
		// after this still grows from there rather than from wherever focus wandered.
		expect(selection.anchorId.value).toBe('n1')
	})

	it('traverses header rows exactly as the plain move does', () => {
		selection.select('n2')
		selection.moveFocusOnly(1)

		expect(selection.focusedId.value).toBe(sectionRow('sec_b'))
		expect(selection.selectedIds.value).toEqual(['n2'])
	})

	it('clamps at both ends rather than wrapping', () => {
		selection.focusRow(sectionRow('sec_a'))
		selection.moveFocusOnly(-1)
		expect(selection.focusedId.value).toBe(sectionRow('sec_a'))

		selection.focusLast()
		selection.moveFocusOnly(1)
		expect(selection.focusedId.value).toBe(noteRow('n4'))
	})

	/** The gesture the whole traversal exists for. `toggle` was already the one
	 *  path to a discontiguous selection; until this there was no way to *reach*
	 *  the second note without the trip there replacing the first. */
	it('composes with toggle into a discontiguous selection', () => {
		selection.select('n1')
		selection.moveFocusOnly(1)
		selection.moveFocusOnly(1)
		selection.moveFocusOnly(1)
		expect(selection.focusedId.value).toBe(noteRow('n3'))

		selection.toggle('n3')
		expect(selection.selectedIds.value).toEqual(['n1', 'n3'])
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
	afterEach(() => sections.reset())

	it('selects every note and then clears the anchor too', () => {
		selection.selectAll()
		expect(selection.selectedIds.value).toEqual(['n1', 'n2', 'n3', 'n4'])

		selection.clear()
		expect(selection.selectedIds.value).toEqual([])
		expect(selection.anchorId.value).toBeNull()
	})

	/** Task-013 AC16. Collapsing folds rows away; it never narrows what an action
	 *  targets, and taking the *visible* order made Ctrl+A silently skip a folded
	 *  section while every other action still reached into it. */
	it('reaches notes inside a collapsed section', () => {
		sections.setCollapsed('sec_b', true)
		expect(selection.visibleNoteIds.value).toEqual(['n1', 'n2'])

		selection.selectAll()
		expect(selection.selectedIds.value).toEqual(['n1', 'n2', 'n3', 'n4'])
	})
})

describe('selectSection', () => {
	afterEach(() => sections.reset())

	it('takes only that section and lands focus on its header', () => {
		selection.selectSection('sec_b')

		expect(selection.selectedIds.value).toEqual(['n3', 'n4'])
		// The header rather than the first note: the target rule reads a focused
		// header as "take the selection", and the section may have no note rows.
		expect(selection.focusedId.value).toBe(sectionRow('sec_b'))
		expect(selection.anchorId.value).toBe('n3')
	})

	it('still takes a collapsed section, whose notes have no rows at all', () => {
		sections.setCollapsed('sec_a', true)

		selection.selectSection('sec_a')

		expect(selection.selectedIds.value).toEqual(['n1', 'n2'])
		expect(selection.focusedId.value).toBe(sectionRow('sec_a'))
	})

	it('empties the selection for a section that is not there', () => {
		selection.select('n1')
		selection.selectSection('sec_missing')

		expect(selection.selectedIds.value).toEqual([])
		// Focus is left where it was: there is no row to move it to.
		expect(selection.focusedId.value).toBe(noteRow('n1'))
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

	/**
	 * Activating a section folds the one being left while the chosen one unfolds,
	 * and the fold keeps the leaving rows in flow while their height animates to
	 * zero — so mid-fold the region is transiently taller than both its settled
	 * selves. A `bottom` captured from a region with no overflow is a phantom:
	 * the reader is at 0 because there is nowhere else to be, and restoring that
	 * anchor pinned the region to the transient bottom every frame, shoving the
	 * top sections off screen and easing back as the fold drained (the
	 * section-activation flicker, 2026-08-14).
	 */
	it('records no anchor for a region with no overflow, so nothing pins through a fold', () => {
		const metrics = { scrollTop: 0, scrollHeight: 120, clientHeight: 120 }
		mountRegion(metrics)

		const snapshot = selection.snapshot()
		expect(snapshot.scroll).toBeNull()

		// The fold's transient extra height. With no anchor there is nothing to
		// restore, and the region stays where the reader was.
		metrics.scrollHeight = 260
		selection.restoreDom(snapshot)
		expect(metrics.scrollTop).toBe(0)
	})

	/**
	 * The fitting-region guard above runs at capture time — but a section
	 * activation's snapshot runs when the store answers, milliseconds into the
	 * fold, when the leaving rows' transient height gives the region phantom
	 * overflow. The capture then cannot tell a phantom from a real bottom and
	 * falls back to the latch, which a region that has never scrolled holds
	 * vacuously true. The restore is where the lie is provable: a reader
	 * genuinely at the end of an overflowing region has `scrollTop > 0`, so a
	 * bottom anchor arriving at `scrollTop 0` with overflow on screen must be
	 * refused — pinning it chased the phantom bottom down frame by frame as the
	 * fold drained (the flicker's second head, 2026-08-14).
	 */
	it('refuses a bottom restore that the scroll position disproves', () => {
		// The settled pre-click state: the list fits, which arms the latch — a
		// region with no overflow reads as "at the bottom".
		const metrics = { scrollTop: 0, scrollHeight: 629, clientHeight: 629 }
		mountRegion(metrics)
		expect(selection.snapshot().scroll).toBeNull()

		// The fold's transient height, present by the time the store answers and
		// the activation's own snapshot runs.
		metrics.scrollHeight = 860
		const snapshot = selection.snapshot()
		expect(snapshot.scroll).toEqual({ kind: 'bottom' })

		selection.restoreDom(snapshot)
		expect(metrics.scrollTop).toBe(0)
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
		// A row entering near `scale(1)` grows the scrollable overflow so slowly
		// that the list can read as still while the entry animation is running.
		// Measured in WebView2 (under the earlier animation library), a settle
		// loop that exited on stillness alone stopped before the growth and left
		// `scrollTop` 12.57px below the true maximum — the note flush against the
		// viewport with the list's bottom padding stranded underneath. Reporting a
		// running animation is what carries the loop across the plateau.
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

	/**
	 * Task-012's control animations are why this matters now. The settle loop asks
	 * `getAnimations({ subtree: true })` whether anything is still running and
	 * holds the pin while something is — and the completion control now animates
	 * on every toggle, inside the very region the loop watches.
	 *
	 * The question is not whether a 0.3s draw delays the release. It may, and
	 * harmlessly: the loop's action is `scrollTop = scrollHeight`, so a longer
	 * hold is a later release rather than a wrong position. The question is
	 * whether an animation that never reports itself finished can hold the loop
	 * open indefinitely, burning a callback every frame for the life of the panel.
	 *
	 * It cannot, and `SETTLE_CAP_MS` is the only thing that guarantees it — the
	 * stability counter is satisfied here from the first frame and the loop keeps
	 * going anyway. Delete the cap clause and this test spins until its own bound
	 * fails it, which is the point of writing it against a hostile stub rather
	 * than against a real animation.
	 */
	it('stops settling on the cap even while an animation never finishes', () => {
		const metrics = { scrollTop: 380, scrollHeight: 500, clientHeight: 120 }
		const region = mountRegion(metrics)
		// Never finishes, and the height never moves — so stability is never the
		// reason the loop continues, and the cap is the only available exit.
		Object.defineProperty(region, 'getAnimations', {
			configurable: true,
			value: () => [{ playState: 'running' }],
		})

		const snapshot = selection.snapshot()
		expect(snapshot.scroll).toEqual({ kind: 'bottom' })

		// Driven rather than awaited: the cap is two seconds, and a test that
		// actually waited them out would be two seconds of the suite.
		const frames: FrameRequestCallback[] = []
		const realRequestAnimationFrame = globalThis.requestAnimationFrame
		const realNow = Date.now
		let clock = 1_000_000
		globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
			frames.push(callback)
			return frames.length
		}) as typeof globalThis.requestAnimationFrame
		Date.now = () => clock

		let pumped = 0
		try {
			selection.restoreDom(snapshot)

			// 16ms of fake clock per frame, so the 2000ms cap falls at ~125.
			while (frames.length > 0 && pumped < 1000) {
				const next = frames.shift()
				if (!next) break
				clock += 16
				pumped++
				next(clock)
			}
		} finally {
			globalThis.requestAnimationFrame = realRequestAnimationFrame
			Date.now = realNow
		}

		// Nothing left scheduled: the loop let go rather than queueing another.
		expect(frames).toHaveLength(0)
		expect(pumped).toBeLessThan(1000)
		// And it was the cap that stopped it, not an early exit on stability —
		// otherwise this would pass just as well with the animation check removed.
		expect(pumped).toBeGreaterThan(100)
		// It kept the list pinned for the whole settle, which is the behaviour the
		// cap is bounding rather than replacing.
		expect(metrics.scrollTop).toBe(500)
	})
})

describe('revealing a row', () => {
	/**
	 * The same hand-installed metrics the anchor tests use — happy-dom lays nothing
	 * out — plus the rows themselves, since a reveal has to find an element to
	 * scroll to. `scrollIntoView` is stubbed per element rather than on the
	 * prototype: what these tests are about is *which* row was asked for.
	 *
	 * The row keys are a parameter, and `add` puts one in later, because a list
	 * that does not render the wanted row is a state of its own — a collapsed
	 * section, a note the done filter hides — and not the same thing as a region
	 * with no height.
	 */
	function mountList(
		clientHeight: number,
		keys: string[] = [sectionRow('sec_a'), noteRow('n1'), noteRow('n2')],
	) {
		const region = document.createElement('div')
		region.setAttribute('data-scroll-region', '')
		Object.defineProperty(region, 'clientHeight', { configurable: true, get: () => clientHeight })
		document.body.append(region)

		const calls: (ScrollIntoViewOptions | undefined)[] = []
		function add(key: string) {
			const row = document.createElement('div')
			row.dataset.rowId = key
			row.scrollIntoView = (options?: boolean | ScrollIntoViewOptions) => {
				calls.push(options as ScrollIntoViewOptions | undefined)
			}
			region.append(row)
			return row
		}
		for (const key of keys) add(key)

		return {
			region,
			calls,
			add,
			row: (key: string) => region.querySelector(`[data-row-id="${key}"]`),
		}
	}

	afterEach(() => {
		// Any request the test left behind would fire inside the next one.
		selection.resetForNewSpace()
		sections.reset()
		document.body.innerHTML = ''
	})

	it('scrolls the asked-for row into view', () => {
		const list = mountList(120)
		const target = list.row(noteRow('n2'))
		const seen: ScrollIntoViewOptions[] = []
		target!.scrollIntoView = (options?: boolean | ScrollIntoViewOptions) => {
			seen.push(options as ScrollIntoViewOptions)
		}

		revealRow(noteRow('n2'))

		expect(seen).toEqual([{ block: 'nearest' }])
	})

	/** A section is a place to be rather than a row to glance at, so it lands at
	 *  the top of the region rather than just inside its edge. */
	it('takes the alignment it is given', () => {
		const list = mountList(120)
		revealRow(sectionRow('sec_a'), 'start')

		expect(list.calls).toEqual([{ block: 'start' }])
	})

	/**
	 * A pinned heading is the one row for which its own landing means nothing.
	 *
	 * `position: sticky` moves what is painted and not what is laid out, so a
	 * heading riding the top of the region reports itself already there and
	 * `scrollIntoView` finds nothing to do — which would make "go to this section"
	 * a no-op for the section the reader is already inside, the exact case they
	 * would use it in. The rowgroup is the heading's layout position, so scrolling
	 * that is what un-pins it.
	 */
	it('lands the section itself when its heading is pinned', () => {
		const list = mountList(120, [])

		const group = document.createElement('div')
		group.dataset.sectionId = 'sec_a'
		// Scrolled 80px into the section, which is how far its heading has been
		// pushed down inside its own group to stay on screen.
		group.getBoundingClientRect = (() => ({ top: -80, bottom: 200, height: 280 })) as () => DOMRect
		const groupCalls: (ScrollIntoViewOptions | undefined)[] = []
		group.scrollIntoView = (options?: boolean | ScrollIntoViewOptions) => {
			groupCalls.push(options as ScrollIntoViewOptions | undefined)
		}
		list.region.append(group)

		const heading = document.createElement('div')
		heading.dataset.rowId = sectionRow('sec_a')
		heading.setAttribute('data-section-row', '')
		heading.getBoundingClientRect = (() => ({ top: 0, bottom: 24, height: 24 })) as () => DOMRect
		const headingCalls: (ScrollIntoViewOptions | undefined)[] = []
		heading.scrollIntoView = (options?: boolean | ScrollIntoViewOptions) => {
			headingCalls.push(options as ScrollIntoViewOptions | undefined)
		}
		group.append(heading)

		revealRow(sectionRow('sec_a'), 'start')

		expect(groupCalls).toEqual([{ block: 'start' }])
		expect(headingCalls).toEqual([])
	})

	/**
	 * The case the whole mechanism exists for: a global capture lands while the
	 * panel is hidden, and a hidden panel's region can have no layout to scroll.
	 * Scrolling it then would report success and do nothing, so the request is kept
	 * and the list flushes it when it next has somewhere to put the row.
	 */
	it('holds the request until the list has a height, then flushes it', () => {
		const hidden = mountList(0)
		revealRow(noteRow('n1'))
		expect(hidden.calls).toEqual([])

		document.body.innerHTML = ''
		const shown = mountList(120)
		flushReveal()

		expect(shown.calls).toEqual([{ block: 'nearest' }])
	})

	/** The drag's own auto-scroll owns the region until the drop, and a row being
	 *  carried is a gesture nothing else may interrupt. */
	it('stands aside while a row is being dragged', () => {
		const list = mountList(120)
		list.row(noteRow('n1'))!.setAttribute('data-dragging', '')

		revealRow(noteRow('n2'))
		expect(list.calls).toEqual([])

		list.row(noteRow('n1'))!.removeAttribute('data-dragging')
		flushReveal()
		expect(list.calls).toEqual([{ block: 'nearest' }])
	})

	/** Flushed once and then gone: a second flush must not re-scroll a list the
	 *  reader has since moved. */
	it('answers a request once', () => {
		const list = mountList(120)
		revealRow(noteRow('n1'))
		flushReveal()

		expect(list.calls).toHaveLength(1)
	})

	/**
	 * A height is not the only thing a reveal can be missing. The note is in a
	 * section the reader has folded shut, so the list renders no row for it at all
	 * — and none of the panel's own triggers (mount, visibility, a drop) fires when
	 * a section is expanded. Without a retry on the rendered rows the request sat
	 * there until something unrelated jumped the list.
	 */
	it('flushes when a row that was not rendered finally is', async () => {
		sections.setCollapsed('sec_a', true)
		const list = mountList(120, [sectionRow('sec_a'), sectionRow('sec_b')])

		revealRow(noteRow('n1'))
		expect(list.calls).toEqual([])

		list.add(noteRow('n1'))
		sections.setCollapsed('sec_a', false)
		// One tick for the watcher on the row order, one for the flush it defers
		// until the DOM has caught up with it.
		await nextTick()
		await nextTick()

		expect(list.calls).toEqual([{ block: 'nearest' }])
	})

	/**
	 * The other half of the same problem: a request that cannot land must not be
	 * held indefinitely, because the reader can take the viewport in the meantime
	 * and a reveal arriving after that yanks it away from them.
	 */
	it('expires a pending request the moment the reader scrolls', () => {
		const hidden = mountList(0)
		revealRow(noteRow('n1'))

		// A wheel is a reader, unlike the scroll events the list's own reflow fires.
		hidden.region.dispatchEvent(new Event('wheel'))

		document.body.innerHTML = ''
		const shown = mountList(120)
		flushReveal()

		expect(shown.calls).toEqual([])
	})

	/**
	 * Activating a section folds the one being left while the chosen one unfolds,
	 * and the fold keeps the leaving rows in flow while their height animates to
	 * zero — so for those 150ms the list is taller than it will be. A reveal
	 * landing inside that window scrolls into height that is about to vanish, and
	 * the region clamps the overshoot back in one frame: the activation flicker
	 * (2026-08-14). So a reveal waits out the region's running motions and lands
	 * on the settled layout.
	 */
	it('waits out a running motion and lands once it settles', async () => {
		const list = mountList(120)
		let finish!: () => void
		const finished = new Promise<void>((resolve) => {
			finish = resolve
		})
		let running = true
		list.region.getAnimations = (() => [
			{ playState: running ? 'running' : 'finished', timeline: null, finished },
		]) as unknown as typeof list.region.getAnimations

		revealRow(noteRow('n2'))
		expect(list.calls).toEqual([])

		running = false
		finish()
		await new Promise((resolve) => setTimeout(resolve))

		expect(list.calls).toEqual([{ block: 'nearest' }])
	})

	/** The row key names a note in a document nobody is looking at any more, and
	 *  the id can even be reused by the space that replaced it. */
	it('drops an unflushed request when the space is replaced', () => {
		mountList(0)
		revealRow(noteRow('n1'))

		selection.resetForNewSpace()

		document.body.innerHTML = ''
		const next = mountList(120)
		flushReveal()
		expect(next.calls).toEqual([])
	})
})
