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
 * **`localStorage['color-scheme']` is a render-time cache, not persistence, and
 * deleting the write would silently restore the flash.** `settings.json` stays
 * the single source of truth and nothing ever reads this key as authority. Its
 * one reader is the pre-hydration script in `index.html`, which runs before IPC
 * is even possible — which is precisely why the value cannot come from Rust. A
 * cache with one writer, one reader and no authority is not a second source of
 * truth; clearing it costs one frame of the wrong theme on the next launch and
 * nothing else.
 */

import { useSettings, type ThemePreference } from './useSettings'

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

/**
 * The three-way preference, mapped onto VueUse's own three modes.
 *
 * `system` stores the string `auto`, which is what `useColorMode` writes for its
 * third mode and what the pre-hydration script already reads correctly: it treats
 * anything that is neither `light` nor `dark` as "consult
 * `prefers-color-scheme`". Storing nothing instead would be undone on the next
 * tick, since `useColorMode` owns the key and writes `auto` back.
 */
function apply(preference: ThemePreference) {
	mode.value = preference === 'system' ? 'auto' : preference
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

	// Driven from the setting rather than from a component, so both moments that
	// matter are covered by one watcher: the startup pull, which repairs a
	// hand-edited `settings.json` or an earlier failed write, and every successful
	// `set_theme_preference`. A component doing this would have to remember to,
	// and the startup one would be the easy half to forget.
	//
	// Gated on the settings having actually arrived. `theme` reads `system` until
	// the pull lands, and applying that would write `auto` over a stored `dark`
	// for the length of one IPC round trip — so a launch interrupted in that
	// window would flash the wrong theme on the next one, which is precisely the
	// defect this write exists to repair.
	const { settings, theme } = useSettings()
	watch(
		[settings, theme],
		([loaded, preference]) => {
			if (loaded) apply(preference)
		},
		{ immediate: true },
	)
}

export function useTheme() {
	install()

	return {
		isDark: readonly(isDark),
		/** `'auto' | 'light' | 'dark'`, VueUse's own vocabulary. Prefer `apply`. */
		mode,
		apply,
	}
}
