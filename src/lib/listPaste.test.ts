import { describe, expect, it } from 'vite-plus/test'

import { splitFlatList } from './listPaste'

describe('splitFlatList', () => {
	it('splits a flat dash list into its item bodies, markers stripped', () => {
		expect(splitFlatList('- Empty dishwasher\n- Take out trash\n- Defrost fridge')).toEqual([
			'Empty dishwasher',
			'Take out trash',
			'Defrost fridge',
		])
	})

	it('accepts every top-level marker Markdown does, mixed freely', () => {
		expect(splitFlatList('* one\n+ two\n- three\n1. four\n2) five')).toEqual([
			'one',
			'two',
			'three',
			'four',
			'five',
		])
	})

	it('survives CRLF line endings and a trailing newline, which is how clipboards arrive', () => {
		expect(splitFlatList('- one\r\n- two\r\n')).toEqual(['one', 'two'])
	})

	it('ignores blank lines between items — a loose list is still flat', () => {
		expect(splitFlatList('- one\n\n- two')).toEqual(['one', 'two'])
	})

	it('refuses a single item: there is nothing to split', () => {
		expect(splitFlatList('- alone')).toBeNull()
		expect(splitFlatList('plain text')).toBeNull()
		expect(splitFlatList('')).toBeNull()
	})

	it('refuses any structure beyond a flat list', () => {
		// A heading over the list.
		expect(splitFlatList('# Chores\n- one\n- two')).toBeNull()
		// A nested item: indentation is hierarchy, and splitting would flatten it.
		expect(splitFlatList('- one\n  - nested\n- two')).toBeNull()
		// Prose between bullets.
		expect(splitFlatList('- one\nremember this\n- two')).toBeNull()
	})

	it('refuses a marker with no content or no following space', () => {
		// `-text` is not a Markdown list item, and `- ` alone has no body to keep.
		expect(splitFlatList('-one\n-two')).toBeNull()
		expect(splitFlatList('- one\n- ')).toBeNull()
	})

	it('keeps inner whitespace and inline markup, trimming only the line end', () => {
		expect(splitFlatList('- Sand paper fridge, add **rust remover**  \n- Paint glossy')).toEqual([
			'Sand paper fridge, add **rust remover**',
			'Paint glossy',
		])
	})
})
