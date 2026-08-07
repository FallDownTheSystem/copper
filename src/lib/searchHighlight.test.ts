import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test'

import { applyHighlight, HIGHLIGHT_NAME, releaseHighlight } from './searchHighlight'

/** happy-dom implements neither `CSS.highlights` nor `Highlight`, and the module
 *  feature-detects both — so without a stub every call is a no-op and the suite
 *  would assert nothing at all. */
class HighlightStub {
	ranges: Range[] = []
	add(range: Range) {
		this.ranges.push(range)
	}
}

let published: HighlightStub | null
let publishCount: number

const registry = {
	set(name: string, highlight: unknown) {
		if (name !== HIGHLIGHT_NAME) return
		published = highlight as HighlightStub
		publishCount++
	},
	delete(name: string) {
		if (name !== HIGHLIGHT_NAME) return
		published = null
		publishCount++
	},
}

const globals = globalThis as unknown as Record<string, unknown>

beforeEach(() => {
	published = null
	publishCount = 0
	globals.Highlight = HighlightStub
	globals.CSS = { ...(globals.CSS as object), highlights: registry }
})

afterEach(() => {
	document.body.innerHTML = ''
})

function body(text: string) {
	const element = document.createElement('div')
	element.textContent = text
	document.body.append(element)
	return element
}

/** A body whose text is split across elements, as a rendered Markdown body's
 *  always is. */
function markup(html: string) {
	const element = document.createElement('div')
	element.innerHTML = html
	document.body.append(element)
	return element
}

function painted() {
	return published?.ranges.map((range) => range.toString()) ?? null
}

/** The module coalesces onto a microtask, so assertions have to wait for it. */
function flush() {
	return Promise.resolve()
}

describe('applyHighlight', () => {
	it('paints a contiguous match case-insensitively', async () => {
		applyHighlight(body('Inherited configuration'), 'inherited')
		await flush()

		expect(painted()).toEqual(['Inherited'])
	})

	it('lands on the right characters when case folding changes length', async () => {
		// The regression: searching a wholesale-lowercased copy and reusing its
		// offsets. `İ` folds to two code units, so every offset after it is shifted
		// by one and the range covers the wrong characters — here `tanb` instead of
		// `stan`, and for a match near the end, an offset past the end of the node.
		applyHighlight(body('İstanbul'), 'stan')
		await flush()

		expect(painted()).toEqual(['stan'])
	})

	it('paints one range per run of a scattered match', async () => {
		// A subsequence match is mostly gaps, so the ranges are the runs rather than
		// one range per matched character — and one range spanning the gaps would
		// paint the text in between.
		applyHighlight(body('albert brown carrot'), 'abc')
		await flush()

		expect(painted()).toEqual(['a', 'b', 'c'])
	})

	it('matches across element boundaries, which is why the text is concatenated', async () => {
		// The change task-014 forced. `http req` has no occurrence inside either text
		// node; task-006 searched each node on its own and would have painted
		// nothing.
		applyHighlight(markup('<strong>HTTP</strong> requests'), 'httpreq')
		await flush()

		expect(painted()).toEqual(['HTTP', 'req'])
	})

	it('never paints a range that spans two nodes', async () => {
		// `Range` would happily cover the gap, and everything between the two text
		// nodes with it — which in a rendered body is markup rather than text.
		applyHighlight(markup('<em>ab</em><em>cd</em>'), 'abcd')
		await flush()

		expect(painted()).toEqual(['ab', 'cd'])
	})

	it('paints nothing when the characters are not in order', async () => {
		applyHighlight(body('alpha beta'), 'zeta')
		await flush()

		expect(published).toBeNull()
	})

	it('publishes once per flush however many bodies wrote', async () => {
		// One watcher per rendered body means a keystroke calls this once per note;
		// a publish that walked the whole map each time made the keystroke
		// quadratic in the number of notes.
		applyHighlight(body('alpha'), 'a')
		applyHighlight(body('alpha'), 'a')
		applyHighlight(body('alpha'), 'a')
		await flush()

		expect(publishCount).toBe(1)
	})

	it('collects ranges from every body into one highlight', async () => {
		applyHighlight(body('first note'), 'note')
		applyHighlight(body('second note'), 'note')
		await flush()

		expect(published?.ranges).toHaveLength(2)
	})

	it('clears an element by applying an empty needle, and drops the entry when empty', async () => {
		const element = body('alpha')
		applyHighlight(element, 'alpha')
		await flush()
		expect(published?.ranges).toHaveLength(1)

		applyHighlight(element, '')
		await flush()
		expect(published).toBeNull()
	})

	it('forgets an unmounted element rather than holding its ranges forever', async () => {
		const element = body('alpha')
		applyHighlight(element, 'alpha')
		await flush()

		element.remove()
		releaseHighlight(element)
		await flush()

		expect(published).toBeNull()
	})

	it('no-ops where the API is absent rather than throwing', async () => {
		globals.CSS = {}
		expect(() => applyHighlight(body('alpha'), 'alpha')).not.toThrow()
	})
})
