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

/** The module coalesces onto a microtask, so assertions have to wait for it. */
function flush() {
	return Promise.resolve()
}

describe('applyHighlight', () => {
	it('paints every case-insensitive occurrence', async () => {
		applyHighlight(body('Inherited and inherited again'), ['inherited'])
		await flush()

		expect(published?.ranges.map((range) => range.toString())).toEqual(['Inherited', 'inherited'])
	})

	it('lands on the right characters when case folding changes length', async () => {
		// The regression: searching a wholesale-lowercased copy and reusing its
		// offsets. `İ` folds to two code units, so every offset after it is shifted
		// by one and the range covers the wrong characters — here `tanb` instead of
		// `stan`, and for a match near the end, an offset past the end of the node.
		applyHighlight(body('İstanbul'), ['stan'])
		await flush()

		expect(published?.ranges.map((range) => range.toString())).toEqual(['stan'])
	})

	it('collects every term of a multi-term query', async () => {
		// The terms share one folded copy of each text node now, so more than one
		// term is the case that would break if that sharing were wrong.
		applyHighlight(body('alpha beta'), ['alpha', 'beta'])
		await flush()

		expect(published?.ranges.map((range) => range.toString())).toEqual(['alpha', 'beta'])
	})

	it('publishes once per flush however many bodies wrote', async () => {
		// One watcher per rendered body means a keystroke calls this once per note;
		// a publish that walked the whole map each time made the keystroke
		// quadratic in the number of notes.
		applyHighlight(body('alpha'), ['a'])
		applyHighlight(body('alpha'), ['a'])
		applyHighlight(body('alpha'), ['a'])
		await flush()

		expect(publishCount).toBe(1)
	})

	it('collects ranges from every body into one highlight', async () => {
		applyHighlight(body('first note'), ['note'])
		applyHighlight(body('second note'), ['note'])
		await flush()

		expect(published?.ranges).toHaveLength(2)
	})

	it('clears an element by applying no terms, and drops the entry when empty', async () => {
		const element = body('alpha')
		applyHighlight(element, ['alpha'])
		await flush()
		expect(published?.ranges).toHaveLength(1)

		applyHighlight(element, [])
		await flush()
		expect(published).toBeNull()
	})

	it('forgets an unmounted element rather than holding its ranges forever', async () => {
		const element = body('alpha')
		applyHighlight(element, ['alpha'])
		await flush()

		element.remove()
		releaseHighlight(element)
		await flush()

		expect(published).toBeNull()
	})

	it('no-ops where the API is absent rather than throwing', async () => {
		globals.CSS = {}
		expect(() => applyHighlight(body('alpha'), ['alpha'])).not.toThrow()
	})
})
