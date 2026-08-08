import { beforeAll, describe, expect, it, vi } from 'vite-plus/test'

import { useMarkdown } from './useMarkdown'

const { renderNote, noteLinks, pruneCache, clearCache, ensureHighlighter, md } = useMarkdown()

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

describe('malformed URLs with an allowed scheme', () => {
	it('rejects one that passes a prefix match but is not a URL', () => {
		// `http:javascript:alert(1)` starts with an allowed scheme and is not a URL
		// at all, so the regex alone would have let it through to the OS opener.
		const html = render('[x](http:javascript:alert(1))')

		expect(html).not.toContain('href')
	})

	it('rejects an http URL that names no host', () => {
		const html = render('[x](https://)')

		expect(html).not.toContain('href')
	})
})

describe('nested links', () => {
	it('renders a linked image as the author link, not an anchor inside an anchor', () => {
		const html = render('[![alt text](https://img.test/p.png)](https://ok.test/page)')

		// Nesting one anchor inside another is invalid markup the browser takes
		// apart; the visible text has to belong to the author's outer link.
		expect(html).toContain('href="https://ok.test/page"')
		expect(html).not.toContain('https://img.test/p.png')
		expect(html.match(/<a /g) ?? []).toHaveLength(1)
		expect(html).toContain('alt text')
	})
})

describe('image alt text', () => {
	it('renders inline markup rather than showing its source', () => {
		const html = render('![**bold** alt](https://ok.test/p.png)')

		// `token.content` is the literal source, asterisks and all.
		expect(html).not.toContain('**')
		expect(html).toContain('bold alt')
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
	it('matches Shiki in layout, with the fence out of the tab order', () => {
		const html = render('```js\nconst a = 1\n```')

		// A scrollable <pre> is natively tabbable, so Shiki's default tabindex="0"
		// would be a second Tab stop inside a grid that claims to be one. F2
		// interaction mode promotes it when the user asks for it.
		expect(html).toContain('<pre class="shiki-plain" tabindex="-1"')
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

	it('keeps the highlighted fence out of the tab order too', () => {
		const html = render('```js\nconst a = 1\n```')

		expect(html).toContain('tabindex="-1"')
		expect(html).not.toContain('tabindex="0"')
	})

	it('stops highlighting once one note has spent its aggregate budget', () => {
		// Each fence is comfortably under the per-fence cap; together they are not.
		const fence = ['```js', 'const a = 1\n'.repeat(900), '```'].join('\n')
		const html = render([fence, fence, fence, fence, fence, fence].join('\n\n'))

		expect(html).toContain('shiki-plain')
	})

	it('falls back to plain text for an unknown language rather than throwing', () => {
		expect(() => render('```brainfuck\n+++.\n```')).not.toThrow()
	})

	it('takes the plain path for a fence too large to tokenise safely', () => {
		const html = render(`\`\`\`js\n${'x'.repeat(30_000)}\n\`\`\``)

		expect(html).toContain('shiki-plain')
	})
})

/**
 * Task-020. `noteLinks` decides which URLs Copper is willing to fetch a preview
 * for, and a preview is a disclosure to a third party — so this is a security
 * boundary in the same sense the scheme allowlist above is, and the cases that
 * matter are the ones where a naive pattern over the body would find a link the
 * reader cannot see.
 */
describe('the links a preview may be fetched for', () => {
	function links(body: string, id = 'nte_links') {
		clearCache()
		return noteLinks({ id, body })
	}

	it('finds both written links and autolinked bare URLs, in order', () => {
		expect(links('see [one](https://a.example/1) and https://b.example/2')).toEqual([
			'https://a.example/1',
			'https://b.example/2',
		])
	})

	it('returns each URL once however many times the note names it', () => {
		expect(links('https://a.example/ and [again](https://a.example/)')).toEqual([
			'https://a.example/',
		])
	})

	/** The case a regex over the body gets wrong. A URL inside a fence or inline
	 *  code is text the reader can see and *not* a link they can follow, so
	 *  fetching it would disclose a page nobody navigated to. */
	it('ignores a URL inside code, which is text rather than a link', () => {
		expect(links('`https://a.example/`')).toEqual([])
		expect(links('```\nhttps://a.example/\n```')).toEqual([])
	})

	/** The same allowlist the renderer applies, so a link with no `href` in the
	 *  output has no preview either. */
	it('excludes every scheme the renderer refuses to emit an href for', () => {
		expect(links('[x](file:///C:/Windows/win.ini)')).toEqual([])
		expect(links('[x](javascript:alert(1))')).toEqual([])
		expect(links('[x](ms-settings:privacy)')).toEqual([])
		expect(links('[x](../../secrets/notes.copper)')).toEqual([])
	})

	/** `mailto:` passes the render-time allowlist deliberately — it is safe to
	 *  click — and is just as deliberately not something to fetch. */
	it('excludes mailto, which is safe to click and not a thing to fetch', () => {
		expect(links('[mail](mailto:someone@example.com)')).toEqual([])
	})

	/** A Markdown image renders as a link *to the image file*, and an image file
	 *  carries no Open Graph metadata — so a preview for one is a request to a
	 *  third party that cannot succeed. It is excluded structurally rather than by
	 *  guessing at extensions: the token is an `image`, not a `link_open`. */
	it('excludes an image, whose anchor points at a file with no metadata', () => {
		expect(links('![alt](https://a.example/hero.png)')).toEqual([])
	})

	/** Keyed on the body string, never on a timestamp: a `git checkout` can
	 *  restore a historical body without touching `updated`, and a stale list
	 *  would fetch previews for URLs the note no longer contains. */
	it('re-reads when the body changes under the same id', () => {
		clearCache()
		expect(noteLinks({ id: 'nte_1', body: 'https://a.example/' })).toEqual(['https://a.example/'])
		expect(noteLinks({ id: 'nte_1', body: 'https://b.example/' })).toEqual(['https://b.example/'])
	})

	it('forgets a note the list no longer holds', () => {
		clearCache()
		noteLinks({ id: 'nte_1', body: 'https://a.example/' })
		pruneCache(['nte_2'])
		// Nothing observable but the absence of a leak, so the assertion is that the
		// next read still answers correctly rather than from a discarded entry.
		expect(noteLinks({ id: 'nte_1', body: 'https://c.example/' })).toEqual(['https://c.example/'])
	})
})
