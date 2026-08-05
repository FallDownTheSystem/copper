import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

/**
 * Small but load-bearing: the whole scrollbar, caret and text-field treatment is
 * native, so a `colorScheme` that does not follow the `.dark` class renders a
 * light scrollbar inside a dark panel. And the storage key has to be the one
 * `index.html`'s pre-hydration script reads, or launch shows a light flash.
 */
/** A fresh module per case: the composable's state is module-scoped by design,
 *  so a cached instance would carry the previous test's mode and make the next
 *  assignment a silent no-op. */
async function freshTheme() {
	vi.resetModules()
	const module = await import('./useTheme')
	return module.useTheme()
}

beforeEach(() => {
	document.documentElement.classList.remove('dark', 'light')
	document.documentElement.style.colorScheme = ''
	localStorage.clear()
})

describe('useTheme', () => {
	it('reads and writes the same localStorage key as the pre-hydration script', async () => {
		const theme = await freshTheme()
		theme.mode.value = 'dark'
		await new Promise((resolve) => setTimeout(resolve, 0))

		// VueUse defaults to `vueuse-color-scheme`. Tracking a second preference
		// alongside the one index.html reads is a light flash on every launch where
		// the two disagree.
		expect(localStorage.getItem('color-scheme')).toBe('dark')
		expect(localStorage.getItem('vueuse-color-scheme')).toBeNull()
	})

	it('keeps colorScheme in step with the resolved appearance', async () => {
		const theme = await freshTheme()

		theme.mode.value = 'dark'
		await new Promise((resolve) => setTimeout(resolve, 0))
		expect(theme.isDark.value).toBe(true)
		expect(document.documentElement.style.colorScheme).toBe('dark')

		theme.mode.value = 'light'
		await new Promise((resolve) => setTimeout(resolve, 0))
		expect(theme.isDark.value).toBe(false)
		expect(document.documentElement.style.colorScheme).toBe('light')
	})

	it('puts the .dark class on the element the stylesheet keys off', async () => {
		const theme = await freshTheme()

		theme.mode.value = 'dark'
		await new Promise((resolve) => setTimeout(resolve, 0))
		expect(document.documentElement.classList.contains('dark')).toBe(true)

		theme.mode.value = 'light'
		await new Promise((resolve) => setTimeout(resolve, 0))
		expect(document.documentElement.classList.contains('dark')).toBe(false)
	})
})
