/**
 * The two palettes the appearance settings choose from: a neutral tone for the
 * panel's greys and an accent for everything copper.
 *
 * **A family is a hue and a chroma multiplier, never a set of colours.** The
 * tokens in `main.css` carry lightness values with measured contrast behind them,
 * so a palette turns `--neutral-h` / `--accent-h` and scales `--neutral-c` /
 * `--accent-c`, and every lightness stays where it was. The comment block above
 * `--neutral-h` in `main.css` is the long form of that rule; this file is the
 * data half.
 *
 * In `lib/` rather than in `useTheme`, and for the reason `useSettings` already
 * takes `errorMessage` from here: `useTheme` imports `useSettings` for values, so
 * a map that both of them need cannot live in either without closing a cycle.
 * This module imports nothing.
 *
 * **Hues are the real Tailwind v4 values**, read out of
 * `node_modules/tailwindcss/theme.css` rather than remembered — its palette is
 * already oklch, so the 500 step's hue angle transfers directly.
 */

/** One family, as the panel applies it. */
export type Family = {
	/** What the settings row and the command palette call it. */
	label: string
	/** The oklch hue angle, from the family's Tailwind 500 step. */
	hue: number
	/** Multiplied into each token's existing chroma. 1 is the shipped palette. */
	chroma: number
	/** The circle in the picker: the family's real Tailwind 500, so a swatch shows
	 *  what the family *is* rather than an exaggerated version of it. The two
	 *  shipped entries show Copper's own tokens for the same reason. */
	swatch: string
}

/**
 * The panel's greys.
 *
 * Chroma multipliers are each family's own Tailwind 500 chroma over `stone`'s
 * 0.013 — stone being the Tailwind family that lands on Copper's warm grey, so
 * the ratio says "how much more tinted than the shipped panel is this". `slate`
 * computes to 3.54 and is capped at 2.5: at full strength its blue reaches the
 * surface tokens and the panel stops reading as a grey with a cast and starts
 * reading as a pale blue sheet, which is not what a *neutral* picker is for.
 */
export const NEUTRAL_TONES = {
	warm: { label: 'Warm', hue: 60, chroma: 1, swatch: 'oklch(0.55 0.012 60)' },
	stone: { label: 'Stone', hue: 58, chroma: 1, swatch: 'oklch(0.553 0.013 58.071)' },
	neutral: { label: 'Neutral', hue: 0, chroma: 0, swatch: 'oklch(0.556 0 0)' },
	zinc: { label: 'Zinc', hue: 286, chroma: 1.25, swatch: 'oklch(0.552 0.016 285.938)' },
	gray: { label: 'Gray', hue: 264, chroma: 2, swatch: 'oklch(0.551 0.027 264.364)' },
	slate: { label: 'Slate', hue: 257, chroma: 2.5, swatch: 'oklch(0.554 0.046 257.417)' },
} as const satisfies Record<string, Family>

/**
 * The accents.
 *
 * Multipliers are each family's Tailwind 500 chroma over 0.205 — Tailwind's own
 * chroma at Copper's hue, interpolated between `orange-500` (0.213 at H 47.6) and
 * `amber-500` (0.188 at H 70.1). That ratio is what keeps every family as *muted*
 * as the copper is: Copper sits at roughly half of what Tailwind would give hue
 * 55, and dividing by the same reference carries that restraint across the set
 * instead of letting each family arrive at full Tailwind strength on a panel that
 * is mostly grey.
 *
 * It also reproduces the fact that hues do not all reach the same chroma — teal
 * and cyan land near 0.7 because cyan genuinely cannot be as vivid as violet at
 * the same lightness, which is the behaviour that keeps the set looking like one
 * palette. The 1.25 ceiling catches `fuchsia` (1.44) and `purple` (1.29), the two
 * that would otherwise glow against the muted surface.
 */
export const ACCENT_COLORS = {
	copper: { label: 'Copper', hue: 55, chroma: 1, swatch: 'oklch(0.62 0.11 55)' },
	red: { label: 'Red', hue: 25, chroma: 1.16, swatch: 'oklch(0.637 0.237 25.331)' },
	orange: { label: 'Orange', hue: 48, chroma: 1.04, swatch: 'oklch(0.705 0.213 47.604)' },
	amber: { label: 'Amber', hue: 70, chroma: 0.92, swatch: 'oklch(0.769 0.188 70.08)' },
	yellow: { label: 'Yellow', hue: 86, chroma: 0.9, swatch: 'oklch(0.795 0.184 86.047)' },
	lime: { label: 'Lime', hue: 131, chroma: 1.14, swatch: 'oklch(0.768 0.233 130.85)' },
	green: { label: 'Green', hue: 150, chroma: 1.07, swatch: 'oklch(0.723 0.219 149.579)' },
	emerald: { label: 'Emerald', hue: 162, chroma: 0.83, swatch: 'oklch(0.696 0.17 162.48)' },
	teal: { label: 'Teal', hue: 183, chroma: 0.68, swatch: 'oklch(0.704 0.14 182.503)' },
	cyan: { label: 'Cyan', hue: 215, chroma: 0.7, swatch: 'oklch(0.715 0.143 215.221)' },
	sky: { label: 'Sky', hue: 237, chroma: 0.82, swatch: 'oklch(0.685 0.169 237.323)' },
	blue: { label: 'Blue', hue: 260, chroma: 1.04, swatch: 'oklch(0.623 0.214 259.815)' },
	indigo: { label: 'Indigo', hue: 277, chroma: 1.14, swatch: 'oklch(0.585 0.233 277.117)' },
	violet: { label: 'Violet', hue: 293, chroma: 1.22, swatch: 'oklch(0.606 0.25 292.717)' },
	purple: { label: 'Purple', hue: 304, chroma: 1.25, swatch: 'oklch(0.627 0.265 303.9)' },
	fuchsia: { label: 'Fuchsia', hue: 322, chroma: 1.25, swatch: 'oklch(0.667 0.295 322.15)' },
	pink: { label: 'Pink', hue: 354, chroma: 1.18, swatch: 'oklch(0.656 0.241 354.308)' },
	rose: { label: 'Rose', hue: 16, chroma: 1.2, swatch: 'oklch(0.645 0.246 16.439)' },
} as const satisfies Record<string, Family>

export type NeutralTone = keyof typeof NEUTRAL_TONES
export type AccentColor = keyof typeof ACCENT_COLORS

/** The panel as it ships, and what an unrecognised name collapses to. */
export const DEFAULT_NEUTRAL: NeutralTone = 'warm'
export const DEFAULT_ACCENT: AccentColor = 'copper'

/**
 * Both narrowings, written once.
 *
 * `settings.json` stores a bare string and the store repairs only its *type*, so
 * a hand-edited or older file can hold a name nothing recognises — and the answer
 * is the shipped palette rather than a broken panel. Exactly the split `theme`
 * and `motion` use.
 */
export function neutralTone(name: string | undefined): NeutralTone {
	return name !== undefined && name in NEUTRAL_TONES ? (name as NeutralTone) : DEFAULT_NEUTRAL
}

export function accentColor(name: string | undefined): AccentColor {
	return name !== undefined && name in ACCENT_COLORS ? (name as AccentColor) : DEFAULT_ACCENT
}
