import { describe, expect, it } from 'vite-plus/test'

import { normaliseSectionName, SECTION_NAME_MAX } from './sectionName'

describe('normaliseSectionName', () => {
	it('collapses whitespace runs and trims, without folding case', () => {
		expect(normaliseSectionName('  Deep   Research \n')).toBe('Deep Research')
		expect(normaliseSectionName('Deep\tResearch')).toBe('Deep Research')
		expect(normaliseSectionName('ReSeArCh')).toBe('ReSeArCh')
		expect(normaliseSectionName('   ')).toBe('')
	})

	it('is idempotent, which is what makes it safe to apply on every keystroke', () => {
		const once = normaliseSectionName('  a   b  ')
		expect(normaliseSectionName(once)).toBe(once)
	})

	it('caps at the same length Rust does, counting code points', () => {
		expect(normaliseSectionName('x'.repeat(500))).toHaveLength(SECTION_NAME_MAX)
		// Code points, not UTF-16 units: `String.slice` would cut a surrogate pair in
		// half. Emoji are outside the BMP, so this is the case that catches it.
		const wide = normaliseSectionName('😀'.repeat(200))
		expect(Array.from(wide)).toHaveLength(SECTION_NAME_MAX)
		expect(wide).not.toContain('�')
	})

	it('leaves no trailing space when the cut lands on one', () => {
		const name = normaliseSectionName('a '.repeat(60))
		expect(name).toBe(name.trimEnd())
		expect(Array.from(name)).toHaveLength(SECTION_NAME_MAX - 1)
	})
})
