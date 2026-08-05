import { beforeAll, describe, expect, it, vi } from 'vite-plus/test'

import { useMarkdown } from './useMarkdown'

const { renderNote, pruneCache, clearCache, ensureHighlighter, md } = useMarkdown()

function render(body: string, id = 'nte_1') {
	clearCache()
	return renderNote({ id, body })
}

describe('markup injection', () => {
	it('renders a script tag as visible text rather than executing it', () => {
		const html = render('<script>alert(1)</script>')

		expect(html).not.toContain('<script>')
		expect(html).toContain('&lt;script&gt;')
	})
})

describe('the link scheme allowlist', () => {
	it('emits no href at all for a file: URL', () => {
		const html = render('[x](file:///C:/Windows/System32/calc.exe)')

		// Not href="#", not aria-disabled: nothing for middle-click or the anchor
		// context menu to act on. markdown-it's own validateLink refuses `file:`
		// before the render rule sees it, so no anchor is produced at all and the
		// source text stays visible — which is stricter than emitting a hrefless
		// anchor, and shows the user what the link pointed at.
		expect(html).not.toContain('href')
		expect(html).not.toContain('<a')
	})

	it('emits no href for a javascript: URL', () => {
		const html = render('[x](javascript:alert(1))')

		expect(html).not.toContain('href')
		expect(html).not.toContain('<a')
	})

	it('emits no href for a relative path, which markdown-it itself permits', () => {
		const html = render('[x](../../secrets/notes.copper)')

		expect(html).toContain('<a')
		expect(html).not.toContain('href')
	})

	it('emits no href for a registered custom protocol handler', () => {
		const html = render('[x](ms-settings:privacy)')

		expect(html).not.toContain('href')
	})

	it('keeps http, https and mailto, with rel and a removed tab stop', () => {
		for (const url of ['https://ok.test/a', 'http://ok.test/a', 'mailto:a@ok.test']) {
			const html = render(`[x](${url})`)

			expect(html).toContain(`href="${url}"`)
			expect(html).toContain('rel="noreferrer"')
			// Rendered anchors are natively tabbable; left alone they break the
			// grid's one-Tab-stop contract.
			expect(html).toContain('tabindex="-1"')
		}
	})

	it('applies the same allowlist to autolinked bare URLs', () => {
		const html = render('see https://ok.test/a for more')

		expect(html).toContain('href="https://ok.test/a"')
		expect(html).toContain('tabindex="-1"')
	})
})

describe('images', () => {
	it('renders a remote image as a hyperlink and never as an img', () => {
		const html = render('![p](https://example.com/p.png)')

		expect(html).not.toContain('<img')
		expect(html).toContain('<a href="https://example.com/p.png"')
		expect(html).toContain('>p</a>')
	})

	it('renders a file: image as plain alt text, with nothing to open', () => {
		const html = render('![p](file:///C:/Windows/System32/calc.exe)')

		expect(html).not.toContain('<img')
		expect(html).not.toContain('href')
		expect(html).toContain('p')
	})
})

describe('heading remapping', () => {
	it('turns a Markdown heading into a non-heading element', () => {
		const html = render('# Notes\n\ntext')

		expect(html).not.toContain('<h1')
		expect(html).toContain('<div class="note-h1">Notes</div>')
	})

	it('leaves a hash without a space alone, which is not a heading at all', () => {
		const html = render('#Notes')

		expect(html).not.toContain('note-h1')
	})
})

describe('tables', () => {
	it('wraps a table so it scrolls inside its own box', () => {
		const html = render('| a | b |\n| - | - |\n| 1 | 2 |')

		expect(html).toContain('<div class="table-scroll"><table>')
		expect(html).toContain('</table></div>')
	})
})

describe('the render cache', () => {
	it('is keyed on the body string, not on the updated timestamp', () => {
		const spy = vi.spyOn(md, 'render')
		clearCache()

		renderNote({ id: 'nte_1', body: 'first' })
		renderNote({ id: 'nte_1', body: 'first' })
		expect(spy).toHaveBeenCalledTimes(1)

		// A git checkout can restore a historical body and a hand-edited file can
		// change one without touching its timestamp. Either would serve stale HTML
		// from a timestamp-keyed cache.
		renderNote({ id: 'nte_1', body: 'second' })
		expect(spy).toHaveBeenCalledTimes(2)

		spy.mockRestore()
	})

	it('does not re-render when the theme class flips', () => {
		clearCache()
		renderNote({ id: 'nte_1', body: '```js\nconst a = 1\n```' })

		const spy = vi.spyOn(md, 'render')
		document.documentElement.classList.add('dark')
		renderNote({ id: 'nte_1', body: '```js\nconst a = 1\n```' })
		document.documentElement.classList.remove('dark')

		// The generated HTML is theme-agnostic — Shiki carries the dark colours as
		// custom properties — so a theme switch is pure CSS and the cache stays
		// valid. Comparing two return values would pass for a deterministic
		// uncached renderer too, which is why this asserts on the call count.
		expect(spy).toHaveBeenCalledTimes(0)
		spy.mockRestore()
	})

	it('drops entries for notes that no longer exist', () => {
		clearCache()
		renderNote({ id: 'nte_1', body: 'a' })
		renderNote({ id: 'nte_2', body: 'b' })

		pruneCache(['nte_1'])

		const spy = vi.spyOn(md, 'render')
		renderNote({ id: 'nte_1', body: 'a' })
		expect(spy).toHaveBeenCalledTimes(0)
		renderNote({ id: 'nte_2', body: 'b' })
		expect(spy).toHaveBeenCalledTimes(1)
		spy.mockRestore()
	})
})

describe('code fences before the highlighter loads', () => {
	it('matches Shiki in layout: tabindex and a trimmed trailing newline', () => {
		const html = render('```js\nconst a = 1\n```')

		expect(html).toContain('<pre class="shiki-plain" tabindex="0"')
		expect(html).not.toContain('const a = 1\n</code>')
	})
})

describe('with the highlighter installed', () => {
	beforeAll(async () => {
		const highlighter = await ensureHighlighter()
		expect(highlighter, 'the Shiki highlighter should be creatable').not.toBeNull()
	}, 60_000)

	it('emits dual-theme markup carrying the dark colours as custom properties', () => {
		const html = render('```js\nconst a = 1\n```')

		expect(html).toContain('class="shiki')
		expect(html).toContain('--shiki-dark')
	})

	it('falls back to plain text for an unknown language rather than throwing', () => {
		expect(() => render('```brainfuck\n+++.\n```')).not.toThrow()
	})

	it('takes the plain path for a fence too large to tokenise safely', () => {
		const html = render(`\`\`\`js\n${'x'.repeat(30_000)}\n\`\`\``)

		expect(html).toContain('shiki-plain')
	})
})
