/**
 * The only lever that reaches VueUse's `usePreferredReducedMotion`, which is why
 * both suites that exercise `useReducedMotion` need it — the composable is a
 * `createSharedComposable` and captures `matchMedia` when the first consumer
 * builds it, so a suite that swaps the preference mid-file has to swap this
 * global and unmount between cases. Not a `.test.ts`, so the runner does not
 * collect it as a suite; nothing in the app imports it.
 */
export function setReducedMotion(reduce: boolean) {
	Object.defineProperty(window, 'matchMedia', {
		configurable: true,
		writable: true,
		value: (query: string) => ({
			matches: reduce && query.includes('prefers-reduced-motion'),
			media: query,
			onchange: null,
			addEventListener: () => {},
			removeEventListener: () => {},
			addListener: () => {},
			removeListener: () => {},
			dispatchEvent: () => false,
		}),
	})
}

export function clearReducedMotion() {
	Reflect.deleteProperty(window as unknown as Record<string, unknown>, 'matchMedia')
}
