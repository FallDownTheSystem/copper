/**
 * Copper's sound points — all seven of them, and deliberately not one more.
 *
 * The engine under `@/lib/sounds` exposes `play()` and the whole 51-recipe
 * palette. This file does not re-export either. The risk with a sound system in
 * a tool someone keeps open all day is that it grows a chirp per interaction and
 * nobody can list them any more; naming each point as a function is what keeps
 * them enumerable — the exports below *are* the complete list, and adding an
 * eighth means adding it here where it can be argued about.
 *
 * Not sounded, on purpose: navigation, selection, typing, search, scrolling,
 * hover, opening menus. Those are continuous or high-frequency.
 *
 * Sound ships **off**. The design's "capture is silent on success" decision is
 * preserved by the default, not overridden by the existence of `captureSucceeded`
 * — turning sound on by default later would be a change to that decision and
 * should be made as one.
 */

import { play, setEnabled } from '@/lib/sounds'

import { useSettings } from './useSettings'

let installed = false

function install() {
	if (installed) return
	installed = true

	/**
	 * Two decisions here, and both are load-bearing rather than style.
	 *
	 * **The scope is detached.** `install()` runs inside whichever caller reached a
	 * sound point first, which in practice is `Composer`'s `setup()` — and a
	 * `watch` registered there belongs to *that component's* effect scope. The
	 * panel swaps the list out for the settings view, so Composer unmounts, the
	 * watcher is disposed with it, and `installed` stays `true` forever: the
	 * setting could never be applied again, in either direction, from the one
	 * screen that can change it. `useTheme` gets an application-lifetime owner for
	 * free by being called from `App.vue`'s root setup; this cannot rely on that,
	 * because its callers are event handlers in plain modules as well as
	 * components, so it owns its scope explicitly.
	 *
	 * **The watcher is ungated**, unlike `useTheme`'s otherwise identical one. The
	 * engine ships `enabled = true`, because the reference app's demo page switches
	 * it on explicitly. Copper's default is the opposite, and `settings` is `null`
	 * until the startup pull lands. `soundsEnabled` reads `false` for that whole
	 * window, so running immediately and ungated is what makes the engine silent
	 * from the first tick — which matters, because a capture can arrive over the
	 * global hotkey before the pull has returned, and `render()` checks `enabled`
	 * *before* it touches `getAudioContext()`. Off by default therefore means no
	 * `AudioContext` is constructed at all. The theme watcher's gate exists because
	 * applying a default there *writes* to disk; nothing here writes.
	 */
	effectScope(true).run(() => {
		const { soundsEnabled } = useSettings()
		// Wrapped rather than passed as the handler directly: a watcher callback is
		// invoked with `(value, previous, onCleanup)`, and handing all three to a
		// third-party function that happens to read only the first is a
		// coincidence, not an interface.
		watch(soundsEnabled, (enabled) => setEnabled(enabled), { immediate: true })
	})
}

/** The click-clack; pairs with the completion control's stroke draw. */
function noteToggled() {
	play('toggle')
}

/** The one routine confirmation. `chime` over `success` — the softer two-note
 *  tink, per ruling OQ3. */
function entrySubmitted() {
	play('chime')
}

/** Global-hotkey capture, which lands with the panel hidden and never focused.
 *  Silent unless the user has explicitly asked for feedback. */
function captureSucceeded() {
	play('tick')
}

/** Redundant reinforcement of a notice that is already visible. */
function captureFailed() {
	play('error')
}

function sectionSwitched() {
	play('pop')
}

/** Once per commit, not once per file — a ten-file drop is one gesture. */
function attachmentsAdded() {
	play('plip')
}

/** Shares `error` with a failed capture: one failure sound, so the meaning stays
 *  legible rather than becoming a vocabulary the user has to learn. */
function actionFailed() {
	play('error')
}

export function useSounds() {
	install()

	return {
		noteToggled,
		entrySubmitted,
		captureSucceeded,
		captureFailed,
		sectionSwitched,
		attachmentsAdded,
		actionFailed,
	}
}
