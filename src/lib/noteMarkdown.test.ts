import { describe, expect, it } from 'vite-plus/test'

import { buildCopyMarkdown, buildListMarkdown } from './noteMarkdown'

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
