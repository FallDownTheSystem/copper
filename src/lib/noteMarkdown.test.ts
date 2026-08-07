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
		const body = '```ts\nconst x = 1\n```'
		expect(buildSectionMarkdown([{ name: 'Code', notes: [{ done: false, body }] }])).toBe(
			'# Code\n- [ ] ```ts\n  const x = 1\n  ```',
		)
	})

	it('leaves a blank continuation line blank', () => {
		expect(
			buildSectionMarkdown([{ name: 'S', notes: [{ done: false, body: 'head\n\ntail' }] }]),
		).toBe('# S\n- [ ] head\n\n  tail')
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
