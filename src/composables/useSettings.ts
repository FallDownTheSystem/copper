/**
 * The settings surface: theme, the two global shortcuts, and launch at login.
 *
 * One adapter per Rust surface, the same amendment `useSpaces` records: this file
 * may invoke the theme, shortcut and autostart commands and nothing else does.
 * `hide_panel` is deliberately not among them — it belongs to the Escape ladder
 * that is its only caller, not to this surface. It holds its own copy of
 * `settings.json` rather than sharing `useSpace`'s, because the two are pulled
 * for different reasons and coupling them would mean a theme change re-reading
 * the document.
 *
 * **Every setter applies its own return value.** Rust emits nothing for a change
 * the frontend initiated — its return value already carries it — so waiting for
 * an event would wait forever, and echoing one back would overwrite whatever the
 * user is in the middle of doing.
 */

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

import { errorMessage } from '@/lib/rustError'
import { createStartup } from '@/lib/startup'

// Type-only, and it has to stay that way. `useSounds` imports this module and
// `useAttachments` imports `useSounds`, so a *value* import of `useSpace` here
// closes the cycle `useAttachments → useSounds → useSettings → useSpace →
// useAttachments` — and `useSpace` calls `useAttachments()` at module scope, so
// whichever of the two is entered first evaluates against a half-built module.
// `errorMessage` is taken from `lib/` above for exactly this reason.
import type { Settings } from './useSpace'

export type ThemePreference = 'system' | 'light' | 'dark'

/**
 * `auto` follows the OS, `off` forces animation off. There is deliberately no
 * value that animates *against* an OS `prefers-reduced-motion: reduce` — that
 * preference is an accessibility signal and an app setting is not entitled to
 * override it. `useReducedMotion` enforces this by OR-ing the two rather than
 * choosing between them, so the guarantee is structural and not a rule someone
 * has to remember.
 */
export type MotionPreference = 'auto' | 'off'

/** Everything `get_shortcut_state` carries: current bindings, the shipped
 *  defaults so Reset needs no second copy of them here, and whether registration
 *  actually took. */
export type ShortcutState = {
	capture: string
	summon: string
	defaults: { capture: string; summon: string }
	summonRegistered: boolean
	summonError: string | null
	captureRegistered: boolean
	captureError: string | null
	/** Present only while the keyboard hook is down and a conventional chord is
	 *  standing in for the double-tap. */
	captureFallback: string | null
}

/** Which shortcut a recording or a rebind is aimed at. Matches the `target`
 *  argument `commit_shortcut_recording` takes. */
export type ShortcutTarget = 'summon' | 'capture'

/** Which row an error belongs under. A failure has to render next to the control
 *  that produced it, exactly as `useSpace`'s scoped action errors do. */
export type SettingsScope = PreferenceScope | ShortcutTarget
type PreferenceScope = 'theme' | 'autostart' | 'sounds' | 'motion'

// --- module-scope state ------------------------------------------------------

const settings = ref<Settings | null>(null)
const shortcuts = ref<ShortcutState | null>(null)
const autostartEnabled = ref(false)
const errors = ref<Partial<Record<SettingsScope, string>>>({})

const theme = computed<ThemePreference>(() => {
	const stored = settings.value?.theme
	return stored === 'light' || stored === 'dark' ? stored : 'system'
})

/** Off unless the file says otherwise, so an unreadable or older `settings.json`
 *  leaves the app silent rather than making noise nobody asked for. */
const soundsEnabled = computed(() => settings.value?.sounds === true)

/** Named here rather than validated in Rust, the same split `theme` uses: the
 *  store repairs wrong *types*, and a value of the right type that names nothing
 *  collapses to the default on read. */
const motionPreference = computed<MotionPreference>(() =>
	settings.value?.motion === 'off' ? 'off' : 'auto',
)

function fail(scope: SettingsScope, error: unknown) {
	errors.value = { ...errors.value, [scope]: errorMessage(error) }
}

function clear(scope: SettingsScope) {
	if (!(scope in errors.value)) return
	const next = { ...errors.value }
	delete next[scope]
	errors.value = next
}

/** The message for one row, or null. Mirrors `useSpace`'s `errorFor`. */
function errorFor(scope: SettingsScope) {
	return computed(() => errors.value[scope] ?? null)
}

// --- pulls -------------------------------------------------------------------

async function pullSettings() {
	try {
		settings.value = await invoke<Settings>('get_settings')
	} catch (error) {
		console.error('[copper] could not read settings', error)
	}
}

/**
 * Pulled rather than pushed, and that is the whole reason a startup registration
 * failure is state instead of an event: `setup()` runs before the webview has
 * loaded, Tauri replays nothing, and a listener registered afterwards could never
 * have heard it.
 */
async function pullShortcuts() {
	try {
		shortcuts.value = await invoke<ShortcutState>('get_shortcut_state')
	} catch (error) {
		console.error('[copper] could not read the shortcut state', error)
	}
}

/**
 * Re-read every time the settings view opens, not only at startup: a startup
 * manager, Task Manager or a registry edit can change this while Copper is
 * running, and nothing notifies us.
 */
async function pullAutostart() {
	try {
		autostartEnabled.value = await invoke<boolean>('get_autostart_enabled')
	} catch (error) {
		console.error('[copper] could not read the autostart state', error)
	}
}

/** Everything the settings view needs, refreshed together. */
async function refresh(): Promise<void> {
	await Promise.all([pullSettings(), pullShortcuts(), pullAutostart()])
}

/**
 * Both listeners re-pull rather than trusting a payload: one code path for
 * "something changed elsewhere, re-read the truth" beats reconciling two payload
 * shapes from two emitters.
 */
const { initialize, dispose } = createStartup(
	() =>
		Promise.all([
			listen('settings-changed', () => void pullSettings()),
			// Its own event, not `settings-changed`. Autostart lives in the Windows
			// registry and deliberately not in `settings.json`, so a listener that
			// responded by re-pulling `get_settings` would learn nothing at all.
			listen('autostart-changed', () => void pullAutostart()),
		]),
	refresh,
)

// --- setters -----------------------------------------------------------------

/**
 * A request generation for one piece of state a setter writes.
 *
 * Two writes to the same state can be in flight at once — the sounds and motion
 * switches are one click apart — and nothing makes them resolve in issue order.
 * The loser would then apply its older answer over the newer one: a `Settings`
 * object taken before the newer write, or a rejection that puts back an error
 * the newer call had already cleared. `useSpace.mutate`'s issued-generation
 * check is the same idea; this is the form it takes where there is no document
 * to be superseded.
 *
 * **One counter per state, not one for the module.** A theme write and a
 * shortcut rebind land in different refs, so letting either supersede the other
 * would drop an answer that nothing is coming to replace — the row would keep
 * showing the old binding with the rebind actually applied in Rust.
 */
type Writes = { issue: () => number; current: (token: number) => boolean }

function generations(): Writes {
	let latest = 0
	return {
		issue: () => ++latest,
		current: (token: number) => token === latest,
	}
}

const settingsWrites = generations()
const shortcutWrites = generations()
const autostartWrites = generations()

/**
 * The shape every setter here shares: clear this row's message, invoke, apply the
 * command's own return value, and report a failure against the row that produced
 * it. `useSpace`'s `mutate` is the same idea, minus the document machinery there
 * is none of here.
 */
async function attempt<T>(
	writes: Writes,
	scope: SettingsScope,
	run: () => Promise<T>,
	apply: (value: T) => void,
): Promise<boolean> {
	clear(scope)
	const issued = writes.issue()
	try {
		const value = await run()
		// Superseded. Reported as the success it was — the command did go through —
		// but its answer is stale and applying it is the reordering above.
		if (writes.current(issued)) apply(value)
		return true
	} catch (error) {
		// The same rule for the failure half, and it is the half that shows: a
		// stale rejection would put back an error the newer call has cleared, next
		// to a row that is no longer in that state.
		if (writes.current(issued)) fail(scope, error)
		return false
	}
}

function setTheme(next: ThemePreference): Promise<boolean> {
	return attempt(
		settingsWrites,
		'theme',
		() => invoke<Settings>('set_theme_preference', { theme: next }),
		(value) => {
			settings.value = value
		},
	)
}

/**
 * Both preferences below go through the general `update_settings` patch rather
 * than earning a command each: unlike the theme, neither has a native side to
 * apply — no window to re-tint, no registry key — so a dedicated Rust command
 * would be a pass-through to the writer that already exists. The patch is one
 * key wide at every call site, so writing one cannot clear the other.
 */
function patchSettings(scope: PreferenceScope, patch: Partial<Settings>): Promise<boolean> {
	return attempt(
		settingsWrites,
		scope,
		() => invoke<Settings>('update_settings', { patch }),
		(value) => {
			settings.value = value
		},
	)
}

function setSounds(enabled: boolean): Promise<boolean> {
	return patchSettings('sounds', { sounds: enabled })
}

function setMotion(preference: MotionPreference): Promise<boolean> {
	return patchSettings('motion', { motion: preference })
}

function setAutostart(enabled: boolean): Promise<boolean> {
	return attempt(
		autostartWrites,
		'autostart',
		() => invoke<boolean>('set_autostart_enabled', { enabled }),
		// The answer is what the registry now says, not what was asked for.
		(value) => {
			autostartEnabled.value = value
		},
	)
}

// --- the recording lease -----------------------------------------------------

/**
 * The lease belongs to Rust; these are only the calls into it.
 *
 * The frontend cannot be responsible for restoring the suspended chords, because
 * the paths that lose them — navigating back, the panel being hidden from the
 * tray, unmount, a WebView reload, a failed IPC call — are exactly the paths that
 * bypass frontend cleanup, while the Rust process stays alive throughout.
 */
async function beginRecording(): Promise<number | null> {
	try {
		return await invoke<number>('begin_shortcut_recording')
	} catch (error) {
		console.error('[copper] could not start recording a shortcut', error)
		return null
	}
}

function commitRecording(token: number, target: ShortcutTarget, chord: string): Promise<boolean> {
	// A failure changed nothing, so nothing changes on screen except the message:
	// the row keeps showing the binding that is still live.
	return attempt(
		shortcutWrites,
		target,
		() => invoke<ShortcutState>('commit_shortcut_recording', { token, target, chord }),
		(value) => {
			shortcuts.value = value
		},
	)
}

async function cancelRecording(): Promise<void> {
	try {
		shortcuts.value = await invoke<ShortcutState>('cancel_shortcut_recording')
	} catch (error) {
		console.error('[copper] could not cancel the shortcut recording', error)
	}
}

/**
 * Reset, which is a rebind to the shipped default rather than a separate
 * operation — so it takes the same anti-lockout path and can fail the same way.
 */
async function resetShortcut(target: ShortcutTarget): Promise<boolean> {
	const fallback = shortcuts.value?.defaults[target]
	if (!fallback) return false
	const command = target === 'summon' ? 'set_summon_shortcut' : 'set_capture_trigger'
	const args = target === 'summon' ? { chord: fallback } : { trigger: fallback }
	return attempt(
		shortcutWrites,
		target,
		() => invoke<ShortcutState>(command, args),
		(value) => {
			shortcuts.value = value
		},
	)
}

export function useSettings() {
	return {
		settings: readonly(settings),
		shortcuts: readonly(shortcuts),
		autostartEnabled: readonly(autostartEnabled),
		theme,
		soundsEnabled,
		motionPreference,
		errorFor,
		initialize,
		dispose,
		refresh,
		setTheme,
		setSounds,
		setMotion,
		setAutostart,
		beginRecording,
		commitRecording,
		cancelRecording,
		resetShortcut,
	}
}
