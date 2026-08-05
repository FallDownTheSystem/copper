/**
 * Owns the `.dark` class and `documentElement.style.colorScheme`.
 *
 * `colorScheme` is not cosmetic here: the scrollbar, the caret and both text
 * fields are native controls, and without it they render light-on-light inside
 * a dark panel.
 *
 * The storage key is configured explicitly. VueUse defaults to
 * `vueuse-color-scheme`, which would track a second preference alongside the one
 * `index.html`'s pre-hydration script reads — and the two disagreeing is exactly
 * a light flash on launch.
 *
 * Phase 3 ships no theme *setting*; task-008 puts the preference in
 * `settings.json`. Until then nothing writes a deliberate choice, so in practice
 * the mode stays `auto` and follows the OS.
 */

const COLOR_SCHEME_KEY = 'color-scheme'

const mode = useColorMode({ storageKey: COLOR_SCHEME_KEY, emitAuto: true })

/** The resolved appearance, with `auto` already collapsed to light or dark. */
const isDark = computed(() => mode.state.value === 'dark')

/**
 * Suppresses transitions for one frame.
 *
 * Two nested `requestAnimationFrame`s, not one: the first only guarantees we are
 * before the next paint, and removing the class there can land in the same frame
 * that applies the new colours, which is the frame the guard exists for.
 */
function guardOneFrame() {
	const root = document.documentElement
	root.classList.add('no-transitions')
	requestAnimationFrame(() => {
		requestAnimationFrame(() => root.classList.remove('no-transitions'))
	})
}

let installed = false

function install() {
	if (installed) return
	installed = true

	// `pre` rather than `post`: useColorMode applies the class in a post-flush
	// watcher, so the guard has to be in place before that runs or the frame it
	// is guarding has already painted.
	watch(
		isDark,
		(dark) => {
			guardOneFrame()
			document.documentElement.style.colorScheme = dark ? 'dark' : 'light'
		},
		{ immediate: true, flush: 'pre' },
	)
}

export function useTheme() {
	install()

	return {
		isDark: readonly(isDark),
		/** `'auto' | 'light' | 'dark'` — task-008's settings view writes this. */
		mode,
	}
}
