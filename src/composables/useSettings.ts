/**
 * The settings surface: theme, the two global shortcuts, and launch at login.
 *
 * One adapter per Rust surface, the same amendment `useSpaces` records: this file
 * may invoke the shell commands and nothing else does. It holds its own copy of
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
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

import { errorMessage, type Settings } from './useSpace'

export type ThemePreference = 'system' | 'light' | 'dark'

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
export type SettingsScope = ThemeScope | ShortcutTarget
type ThemeScope = 'theme' | 'autostart'

// --- module-scope state ------------------------------------------------------

const settings = ref<Settings | null>(null)
const shortcuts = ref<ShortcutState | null>(null)
const autostartEnabled = ref(false)
const errors = ref<Partial<Record<SettingsScope, string>>>({})

let initPromise: Promise<void> | null = null
let unlisteners: UnlistenFn[] = []

const theme = computed<ThemePreference>(() => {
	const stored = settings.value?.theme
	return stored === 'light' || stored === 'dark' ? stored : 'system'
})

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
function refresh(): Promise<void> {
	return Promise.all([pullSettings(), pullShortcuts(), pullAutostart()]).then(() => undefined)
}

/**
 * Listen, then pull — in that order, and "listen" means awaited registration.
 * `listen()` returns a promise, so calling it earlier in source order than the
 * pull still leaves the window where an event fires between them and is lost.
 *
 * Both listeners re-pull rather than trusting a payload: one code path for
 * "something changed elsewhere, re-read the truth" beats reconciling two payload
 * shapes from two emitters.
 */
function initialize(): Promise<void> {
	initPromise ??= (async () => {
		unlisteners = await Promise.all([
			listen('settings-changed', () => void pullSettings()),
			// Its own event, not `settings-changed`. Autostart lives in the Windows
			// registry and deliberately not in `settings.json`, so a listener that
			// responded by re-pulling `get_settings` would learn nothing at all.
			listen('autostart-changed', () => void pullAutostart()),
		])
		await Promise.all([pullSettings(), pullShortcuts(), pullAutostart()])
	})()

	return initPromise
}

function dispose() {
	for (const unlisten of unlisteners) unlisten()
	unlisteners = []
	initPromise = null
}

// --- setters -----------------------------------------------------------------

async function setTheme(next: ThemePreference): Promise<boolean> {
	clear('theme')
	try {
		settings.value = await invoke<Settings>('set_theme_preference', { theme: next })
		return true
	} catch (error) {
		fail('theme', error)
		return false
	}
}

async function setAutostart(enabled: boolean): Promise<boolean> {
	clear('autostart')
	try {
		// The answer is what the registry now says, not what was asked for.
		autostartEnabled.value = await invoke<boolean>('set_autostart_enabled', { enabled })
		return true
	} catch (error) {
		fail('autostart', error)
		return false
	}
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

async function commitRecording(
	token: number,
	target: ShortcutTarget,
	chord: string,
): Promise<boolean> {
	clear(target)
	try {
		shortcuts.value = await invoke<ShortcutState>('commit_shortcut_recording', {
			token,
			target,
			chord,
		})
		return true
	} catch (error) {
		// Nothing changed, so nothing changes on screen except this: the row keeps
		// showing the binding that is still live.
		fail(target, error)
		return false
	}
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
	clear(target)
	try {
		const command = target === 'summon' ? 'set_summon_shortcut' : 'set_capture_trigger'
		const args = target === 'summon' ? { chord: fallback } : { trigger: fallback }
		shortcuts.value = await invoke<ShortcutState>(command, args)
		return true
	} catch (error) {
		fail(target, error)
		return false
	}
}

export function useSettings() {
	return {
		settings: readonly(settings),
		shortcuts: readonly(shortcuts),
		autostartEnabled: readonly(autostartEnabled),
		theme,
		errorFor,
		initialize,
		dispose,
		refresh,
		setTheme,
		setAutostart,
		beginRecording,
		commitRecording,
		cancelRecording,
		resetShortcut,
	}
}
