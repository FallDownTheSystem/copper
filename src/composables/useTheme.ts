/**
 * Owns every appearance signal that lives on the root element: the `.dark` class,
 * `documentElement.style.colorScheme`, the `.reduce-motion` and `.translucent`
 * classes, and the four palette custom properties.
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

import {
	ACCENT_COLORS,
	DEFAULT_ACCENT,
	DEFAULT_NEUTRAL,
	NEUTRAL_TONES,
	type AccentColor,
	type NeutralTone,
} from '@/lib/palette'

import { useReducedMotion } from './useReducedMotion'
import { DEFAULT_VIBRANCY, useSettings, type ThemePreference } from './useSettings'

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

/**
 * Writes one palette dial, or takes it away.
 *
 * **The shipped palette is an absence, not a value.** `main.css` declares all
 * four dials on `:root` at the values the panel was designed with, so removing
 * the inline property is what restores them — and it is the only form in which
 * "warm and copper" and "this file has never been touched" render identically.
 * Writing `--neutral-h: 60` instead would work today and quietly pin the default
 * the first time someone retuned the stylesheet.
 */
function dial(name: string, value: number, shipped: boolean) {
	const root = document.documentElement
	if (shipped) root.style.removeProperty(name)
	else root.style.setProperty(name, String(value))
}

/**
 * Both families at once, because they are one visual event and `guardOneFrame`
 * is per-frame rather than per-property.
 *
 * **Vibrancy scales the accent's chroma and deliberately not the grey's.** The
 * complaint it answers is that the shipped families arrive as muted as the
 * copper they were calibrated against, which reads as washed out on the vivid
 * ones; that is a statement about accents. `--neutral-c` is read by every
 * surface, separator, shadow and body-text token in the file, so scaling it
 * would not make the greys more characterful, it would repaint the panel — a
 * different feature, with a different failure mode, and not one to smuggle in
 * under this dial.
 */
function applyPalette(neutral: NeutralTone, accent: AccentColor, vibrancy: number) {
	const tone = NEUTRAL_TONES[neutral]
	const color = ACCENT_COLORS[accent]
	const shippedTone = neutral === DEFAULT_NEUTRAL
	// Both halves of the accent, not just its name. The absence rule below holds
	// only where the inline property and `main.css` would say the same thing, and
	// copper at 1.3 is not what the stylesheet declares — so the shipped family
	// still has to be written out whenever the dial has been moved.
	const shippedColor = accent === DEFAULT_ACCENT && vibrancy === DEFAULT_VIBRANCY

	dial('--neutral-h', tone.hue, shippedTone)
	dial('--neutral-c', tone.chroma, shippedTone)
	dial('--accent-h', color.hue, shippedColor)
	dial('--accent-c', color.chroma * vibrancy, shippedColor)
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
	const { settings, theme, translucent, neutralTone, accentColor, vibrancy } = useSettings()
	watch(
		[settings, theme],
		([loaded, preference]) => {
			if (loaded) apply(preference)
		},
		{ immediate: true },
	)

	// Ungated on `settings`, unlike the theme above, because the failure the gate
	// prevents there does not exist here. `theme` writes through to a localStorage
	// cache the *next launch* reads, so applying its pre-pull value can outlive the
	// session; these four write nothing but the current document, and the pull that
	// corrects them is the same one that would have released the gate. What a gate
	// would cost is a frame of the shipped palette on a panel the user has
	// repainted.
	//
	// The same one-frame guard the theme switch uses, and needed for the same
	// reason: every token in the file resolves against these, so a change without
	// it puts several hundred elements into a colour transition at once.
	//
	// `vibrancy` rides with them rather than earning a watcher of its own: it is a
	// factor *of* the accent, so the two have to be written in the same frame — and
	// while the slider is being dragged this runs once per pointer move, which is
	// precisely the case the frame guard was built for.
	watch(
		[neutralTone, accentColor, vibrancy],
		([neutral, accent, level]) => {
			guardOneFrame()
			applyPalette(neutral, accent, level)
		},
		{ immediate: true, flush: 'pre' },
	)

	// A class rather than a property, because CSS has to be able to reach it from
	// two selectors of different specificity — the dark panel wants a thinner
	// surface than the light one, and `prefers-contrast: more` has to be able to
	// outrank both. The material behind it is Rust's half of the same setting.
	watch(translucent, (on) => document.documentElement.classList.toggle('translucent', on), {
		immediate: true,
	})

	// The `prefers-reduced-motion` block in main.css covers only the OS half of
	// the preference. This mirrors `useReducedMotion`'s OR — OS *or* the app's own
	// "Animate controls" setting — onto a root class, which is the only way CSS
	// can see a value that lives in `settings.json`. It rides here rather than in
	// a component because this is where the once-guard and documentElement
	// already are.
	const reduced = useReducedMotion()
	watch(reduced, (off) => document.documentElement.classList.toggle('reduce-motion', off), {
		immediate: true,
	})
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
