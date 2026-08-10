/**
 * Every settings-surface action, as one list the command palette can search.
 *
 * **Keyed by `PreferenceScope`, which is the whole point.** A plain array would
 * drift from `SettingsView` the first time a toggle was added and nobody
 * remembered this file; a `Record<PreferenceScope, …>` written out per scope
 * makes the omission a *compile* error instead — the same tripwire
 * `useSettings`' own `rowWrites` uses, and for the same reason. The two shortcut
 * rows are outside `PreferenceScope` and therefore outside this list, which is
 * the right cut: recording a chord needs Rust's lease and a palette row could
 * not hold one. `Open Settings` is how they stay reachable.
 *
 * **Derived inside a function, never as a module-scope constant.** The labels
 * carry live state — `Keep on top` reads `On` or `Off` — so the list has to be
 * built while `useSettings()` has an answer. Evaluated at import time it would
 * capture a `settings.value` that is still `null`.
 *
 * **Failures borrow the panel's one error band.** Every setter already reports
 * into `errorFor(scope)`, which only the settings view renders; the palette is
 * in exactly the position `PanelHeader`'s pin is — a control outside Settings
 * with no inline slot of its own — so it re-reports the row's own message into
 * the `list` scope, which is the band `StatusLine` shows. Rust's sentence rather
 * than one of ours: it names which half failed, and a sentence of our own could
 * only be vaguer.
 */

import { ACCENT_COLORS, NEUTRAL_TONES } from '@/lib/palette'

import { formatVibrancy, type PreferenceScope } from './useSettings'

export type PaletteAction = {
	/** Stable across re-derivations: it is the `v-for` key, the listbox value and
	 *  the handle a test reaches for. */
	id: string
	/** What the palette shows and what `fuzzyMatch` scores against. */
	label: string
	/** The trailing state, e.g. `On` or `Dark`. Deliberately **not** part of the
	 *  match text: typing `on` would otherwise pull in every switch that happens
	 *  to be enabled. */
	value?: string
	run: () => void | Promise<unknown>
}

/** The only thing said when a refusal left no cause behind. Anything Rust
 *  explained is preferred over it. */
const REFUSED = 'Copper could not change that setting.'

function onOff(enabled: boolean) {
	return enabled ? 'On' : 'Off'
}

export function settingsActions(): PaletteAction[] {
	const {
		theme,
		soundsEnabled,
		motionPreference,
		insertionPoint,
		doubleClickAction,
		alwaysOnTop,
		showCreated,
		captureNotifications,
		linkPreviews,
		translucent,
		neutralTone,
		accentColor,
		vibrancy,
		resizable,
		panelWidth,
		panelHeight,
		errorFor,
		setTheme,
		setSounds,
		setMotion,
		setInsertionPoint,
		setDoubleClick,
		setShowCreated,
		setCaptureNotifications,
		setLinkPreviews,
		setAlwaysOnTop,
		setTranslucency,
		setResizable,
		setAutostart,
		autostartEnabled,
	} = useSettings()
	const { reportActionError, clearActionError } = useSpace()
	const { showSettings } = useView()
	const { canInstall, available, checkForUpdate, installUpdate } = useUpdater()

	async function write(scope: PreferenceScope, run: () => Promise<boolean>) {
		clearActionError('list')
		if (await run()) return
		reportActionError('list', errorFor(scope).value ?? REFUSED)
	}

	/** Three rows rather than one that cycles: `system`, `light` and `dark` are
	 *  three destinations, and a palette is a place you name the one you want. */
	const themeActions = (['system', 'light', 'dark'] as const).map((preference) => ({
		id: `theme-${preference}`,
		label: `Theme: ${preference === 'system' ? 'System' : preference === 'light' ? 'Light' : 'Dark'}`,
		value: theme.value === preference ? 'Current' : undefined,
		run: () => write('theme', () => setTheme(preference)),
	}))

	const byScope: Record<PreferenceScope, PaletteAction | PaletteAction[]> = {
		theme: themeActions,
		alwaysOnTop: {
			id: 'always-on-top',
			label: 'Keep on top',
			value: onOff(alwaysOnTop.value),
			run: () => write('alwaysOnTop', () => setAlwaysOnTop(!alwaysOnTop.value)),
		},
		insertionPoint: {
			id: 'insertion-point',
			label: 'New notes go',
			value: insertionPoint.value === 'top' ? 'Top' : 'Bottom',
			run: () =>
				write('insertionPoint', () =>
					setInsertionPoint(insertionPoint.value === 'top' ? 'bottom' : 'top'),
				),
		},
		doubleClick: {
			id: 'double-click',
			label: 'Double-click a note',
			value: doubleClickAction.value === 'edit' ? 'Edit' : 'Copy',
			run: () =>
				write('doubleClick', () =>
					setDoubleClick(doubleClickAction.value === 'edit' ? 'copy' : 'edit'),
				),
		},
		showCreated: {
			id: 'show-created',
			label: 'Date added',
			value: onOff(showCreated.value),
			run: () => write('showCreated', () => setShowCreated(!showCreated.value)),
		},
		sounds: {
			id: 'sounds',
			label: 'Sound',
			value: onOff(soundsEnabled.value),
			run: () => write('sounds', () => setSounds(!soundsEnabled.value)),
		},
		// The switch is the presence of animation; the setting is a two-value
		// preference, because `auto` means "defer to Windows" and a boolean has
		// nowhere to say that. Off is the only thing this can assert.
		motion: {
			id: 'motion',
			label: 'Animate controls',
			value: onOff(motionPreference.value === 'auto'),
			run: () =>
				write('motion', () => setMotion(motionPreference.value === 'auto' ? 'off' : 'auto')),
		},
		captureNotifications: {
			id: 'capture-notifications',
			label: 'Capture notifications',
			value: onOff(captureNotifications.value),
			run: () =>
				write('captureNotifications', () => setCaptureNotifications(!captureNotifications.value)),
		},
		linkPreviews: {
			id: 'link-previews',
			label: 'Link previews',
			value: onOff(linkPreviews.value),
			run: () => write('linkPreviews', () => setLinkPreviews(!linkPreviews.value)),
		},
		translucent: {
			id: 'translucent',
			label: 'Translucent background',
			value: onOff(translucent.value),
			run: () => write('translucent', () => setTranslucency(!translucent.value)),
		},
		// **These two open Settings rather than changing anything**, and they are the
		// first rows here that do. A palette has six and eighteen members, so the
		// cycle every other row uses would need eighteen presses to cross and would
		// repaint the whole panel at each one; and unlike `Dark` or `Top`, a colour is
		// not something a person can name from a list they cannot see. So the palette
		// does what it can do well — say which one is currently on, and be the way to
		// the place where the choice is visible. `Open Settings` below covers the same
		// ground for the two recorder rows, for a different reason and in the same
		// shape.
		neutral: {
			id: 'neutral-tone',
			label: 'Gray tone',
			value: NEUTRAL_TONES[neutralTone.value].label,
			run: showSettings,
		},
		accent: {
			id: 'accent-color',
			label: 'Accent color',
			value: ACCENT_COLORS[accentColor.value].label,
			run: showSettings,
		},
		// Opens Settings for the same reason the two above do, and one more of its
		// own: this is a continuous dial, so there is no next value to cycle to —
		// only thirty of them — and the point of moving it is watching the panel
		// change while you do. A palette row can say where it currently stands and
		// take you to the track; it cannot be the track.
		vibrancy: {
			id: 'vibrancy',
			label: 'Vibrancy',
			value: formatVibrancy(vibrancy.value),
			run: showSettings,
		},
		resizable: {
			id: 'resizable',
			label: 'Resizable',
			value: onOff(resizable.value),
			run: () => write('resizable', () => setResizable(!resizable.value)),
		},
		// Two numbers with no second value to flip to, so this joins the rows that
		// open Settings rather than the rows that act.
		panelSize: {
			id: 'panel-size',
			label: 'Panel size',
			value: `${panelWidth.value} × ${panelHeight.value}`,
			run: showSettings,
		},
		autostart: {
			id: 'autostart',
			label: 'Launch Copper at login',
			value: onOff(autostartEnabled.value),
			run: () => write('autostart', () => setAutostart(!autostartEnabled.value)),
		},
	}

	/**
	 * The rows with no preference behind them, so no scope to key them by.
	 *
	 * `Open Settings` is required by the acceptance criteria and is also what
	 * covers the two recorder rows above. The update row is the settings view's
	 * one button in both of its states: `canInstall` decides which, exactly as it
	 * does there, so the palette cannot offer an install that does not exist.
	 */
	const unscoped: PaletteAction[] = [
		{ id: 'open-settings', label: 'Open Settings', run: showSettings },
		// The reference lives in Settings → Shortcuts; the palette row is how
		// "what was that key" gets answered without knowing where the list lives.
		{ id: 'keyboard-shortcuts', label: 'Keyboard shortcuts', run: showSettings },
		{
			id: 'update',
			label: canInstall.value
				? `Install ${available.value?.version ?? ''}`.trim()
				: 'Check for updates',
			run: () => (canInstall.value ? installUpdate() : checkForUpdate()),
		},
	]

	return [...Object.values(byScope).flat(), ...unscoped]
}
