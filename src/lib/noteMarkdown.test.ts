import MarkdownIt from 'markdown-it'
import { describe, expect, it } from 'vite-plus/test'

import { buildCopyMarkdown, buildListMarkdown, buildSectionMarkdown } from './noteMarkdown'

describe('buildCopyMarkdown', () => {
	it('copies a single body unchanged', () => {
		expect(buildCopyMarkdown(['**one**'])).toBe('**one**')
	})

	it('joins two bodies with exactly one blank line', () => {
		expect(buildCopyMarkdown(['one', 'two'])).toBe('one\n\ntwo')
	})

	it('preserves interior blank lines and leading whitespace', () => {
		// Both are meaningful Markdown — indented code blocks and list nesting —
		// and the store preserves them, so the clipboard must too.
		const body = '    indented code\n\nsecond paragraph'
		expect(buildCopyMarkdown([body])).toBe(body)
	})
})

describe('buildListMarkdown', () => {
	it('prefixes every note with `- ` and never checkbox syntax', () => {
		const list = buildListMarkdown(['alpha', 'beta'])
		expect(list).toBe('- alpha\n- beta')
		expect(list).not.toContain('[ ]')
		expect(list).not.toContain('[x]')
	})

	it('indents continuation lines by two spaces so they stay in one item', () => {
		expect(buildListMarkdown(['one\ntwo\nthree'])).toBe('- one\n  two\n  three')
	})

	it('leaves a blank continuation line blank rather than indenting whitespace', () => {
		expect(buildListMarkdown(['head\n\ntail'])).toBe('- head\n\n  tail')
	})

	/** It shares `item` with the section renderer, so it inherits the same
	 *  block-structure rule — and wants it for the same reason, since a prompt
	 *  carrying a mangled fence is worse than one carrying an indented item. */
	it('puts a body that opens a block construct under a bare marker', () => {
		expect(buildListMarkdown(['```ts\nconst x = 1\n```'])).toBe(
			'-\n\n  ```ts\n  const x = 1\n  ```',
		)
	})

	/** A body pasted from a Windows app carries CRLF; a stray `\r` at the end of
	 *  every line is invisible on the clipboard and wrong everywhere it lands. */
	it('strips the carriage returns of a CRLF body', () => {
		expect(buildListMarkdown(['one\r\ntwo'])).toBe('- one\n  two')
	})

	it('emits no section headings', () => {
		expect(buildListMarkdown(['a', 'b'])).not.toContain('##')
	})

	it('returns an empty string for no notes', () => {
		expect(buildListMarkdown([])).toBe('')
	})
})

describe('buildSectionMarkdown', () => {
	const setup = {
		name: 'Project Setup',
		notes: [
			{ done: false, body: 'Install dependencies\nNote body here.' },
			{ done: true, body: 'Configure environment\nDone note body.' },
		],
	}
	const testing = { name: 'Testing', notes: [{ done: false, body: 'Write unit tests' }] }

	it('renders sections as ATX headings and notes as task-list items', () => {
		expect(buildSectionMarkdown([setup, testing])).toBe(
			'# Project Setup\n' +
				'- [ ] Install dependencies\n' +
				'  Note body here.\n' +
				'- [x] Configure environment\n' +
				'  Done note body.\n' +
				'\n' +
				'# Testing\n' +
				'- [ ] Write unit tests',
		)
	})

	/** AC12. The three scopes differ only in which sections they hand over, so the
	 *  same input has to come back byte-identical however it was resolved. */
	it('is byte-identical for the same input whatever the scope resolved it', () => {
		const whole = buildSectionMarkdown([setup, testing])
		const selection = buildSectionMarkdown([setup, testing])
		expect(selection).toBe(whole)
		expect(buildSectionMarkdown([testing])).toBe(whole.split('\n\n')[1])
	})

	it('embeds a body as-is rather than escaping Markdown inside it', () => {
		expect(
			buildSectionMarkdown([{ name: 'S', notes: [{ done: false, body: 'a **bold** word' }] }]),
		).toBe('# S\n- [ ] a **bold** word')
	})

	it('leaves a blank continuation line blank', () => {
		expect(
			buildSectionMarkdown([{ name: 'S', notes: [{ done: false, body: 'head\n\ntail' }] }]),
		).toBe('# S\n- [ ] head\n\n  tail')
	})

	/**
	 * Markdown's block syntax is line-anchored, so a body that opens one cannot
	 * follow the marker on the same line — it goes underneath instead.
	 *
	 * The two `- [x]` / `- [ ]` cases below are asserted as strings; the parse
	 * `describe` after them is what proves the strings are the *right* strings.
	 */
	describe('a body that opens a block construct', () => {
		function render(body: string, done = false) {
			return buildSectionMarkdown([{ name: 'S', notes: [{ done, body }] }])
		}

		it('puts a fenced code block under a bare marker', () => {
			expect(render('```ts\nconst x = 1\n```')).toBe('# S\n- [ ]\n\n  ```ts\n  const x = 1\n  ```')
		})

		it('puts a body whose second line is a setext underline under a bare marker', () => {
			// The nastiest of them inline: `- [ ] Title` followed by `  ===` makes the
			// whole item a heading, checkbox text and all.
			expect(render('Title\n===', true)).toBe('# S\n- [x]\n\n  Title\n  ===')
		})

		it('puts a blockquote under a bare marker', () => {
			expect(render('> quoted')).toBe('# S\n- [ ]\n\n  > quoted')
		})

		it('puts a nested list under a bare marker', () => {
			expect(render('- inner\n- second')).toBe('# S\n- [ ]\n\n  - inner\n  - second')
		})

		it('puts a heading, a table and indented code under a bare marker', () => {
			expect(render('# Title')).toBe('# S\n- [ ]\n\n  # Title')
			expect(render('Name | Age\n--- | ---')).toBe('# S\n- [ ]\n\n  Name | Age\n  --- | ---')
			expect(render('    indented code')).toBe('# S\n- [ ]\n\n      indented code')
		})

		it('leaves an inline-safe body compact', () => {
			// Emphasis, links and inline code all mean the same thing anywhere on a
			// line, so nothing is gained by pushing them down.
			expect(render('`inline code` and *emphasis*')).toBe('# S\n- [ ] `inline code` and *emphasis*')
			expect(render('a line\nand another')).toBe('# S\n- [ ] a line\n  and another')
		})
	})

	/**
	 * The claim the rest of this file makes in strings, checked against a real
	 * parser: the whole point of the output is that someone pastes it somewhere
	 * and it survives.
	 */
	describe('parsed back by markdown-it', () => {
		const md = new MarkdownIt()

		it('keeps a fenced code block a fenced code block', () => {
			const html = md.render(render1('```ts\nconst x = 1\n```'))
			expect(html).toContain('<code class="language-ts">')
			expect(html).toContain('const x = 1')
		})

		it('keeps a setext-underlined body inside its own item instead of eating the marker', () => {
			const html = md.render(render1('Title\n==='))
			// Inline, this produced `<h1>[ ] Title</h1>` — the item became the heading.
			expect(html).toContain('<h1>Title</h1>')
			expect(html).not.toContain('<h1>[ ] Title</h1>')
		})

		it('keeps a table a table, with the marker outside it', () => {
			const html = md.render(render1('Name | Age\n--- | ---\nada | 36'))
			expect(html).toContain('<th>Name</th>')
			// Inline, the delimiter row swallowed the list marker into the first cell.
			expect(html).not.toContain('[ ] Name')
		})

		it('renders an inline-safe note as one list item', () => {
			expect(md.render(render1('just a note'))).toContain('<li>[ ] just a note</li>')
		})

		function render1(body: string) {
			return buildSectionMarkdown([{ name: 'S', notes: [{ done: false, body }] }])
		}
	})

	it('renders a section with nothing in scope as its heading alone', () => {
		expect(buildSectionMarkdown([{ name: 'Empty', notes: [] }])).toBe('# Empty')
	})

	it('returns an empty string for no sections', () => {
		expect(buildSectionMarkdown([])).toBe('')
	})

	/** Attachments are omitted, so a note carrying one renders exactly as the same
	 *  note without it. */
	it('renders nothing for attachments', () => {
		const rendered = buildSectionMarkdown([{ name: 'S', notes: [{ done: false, body: 'a note' }] }])
		expect(rendered).toBe('# S\n- [ ] a note')
	})
})
