import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

/**
 * The engine is a verbatim copy from the reference app, so these do not re-test
 * its synthesis. They pin the two properties Copper actually depends on and that
 * a future edit could quietly remove:
 *
 * - a disabled engine touches **no** Web Audio at all, which is what makes
 *   "sound ships off" mean "no `AudioContext` is ever constructed" rather than
 *   "a context exists but stays quiet" (task-012 AC9); and
 * - audio that the browser refuses is a no-op rather than a throw, which is the
 *   whole reason a capture from a hidden, never-focused window is safe to sound.
 */

/** Every recipe `useSounds` names, so a rename there is caught here too. */
const COPPER_RECIPES = ['toggle', 'tick', 'chime', 'error', 'pop', 'plip'] as const

type ContextBehaviour = 'blocked' | 'suspended'

let constructed = 0

/**
 * Deliberately never `running`: a context that reports `running` would send the
 * engine into the real node graph, and happy-dom has no Web Audio to build one
 * from. `suspended` exercises everything these tests are about — construction
 * happens, playback does not — without needing a fake `OscillatorNode`.
 */
function installAudioContext(behaviour: ContextBehaviour) {
	constructed = 0
	const Ctor = function AudioContextStub() {
		constructed++
		if (behaviour === 'blocked') throw new Error('audio is blocked')
		return { state: 'suspended', resume: () => Promise.reject(new Error('no gesture')) }
	}
	Object.defineProperty(window, 'AudioContext', {
		value: Ctor,
		configurable: true,
		writable: true,
	})
}

/** The engine caches its context and its enabled flag at module scope, so a
 *  shared instance would carry the previous case's state. */
async function freshEngine() {
	vi.resetModules()
	return await import('./engine')
}

beforeEach(() => {
	installAudioContext('suspended')
})

afterEach(() => {
	Reflect.deleteProperty(window as unknown as Record<string, unknown>, 'AudioContext')
})

describe('the sound engine', () => {
	/**
	 * Task-012 AC9, asserted at the constructor rather than by listening for
	 * silence. `render()` checks the flag *before* it calls `getAudioContext()`,
	 * and that order is the load-bearing part: it is what lets a capture arriving
	 * over the global hotkey — before the settings pull has even returned — cost
	 * nothing at all.
	 */
	it('constructs no AudioContext while disabled', async () => {
		const engine = await freshEngine()
		engine.setEnabled(false)

		for (const recipe of COPPER_RECIPES) engine.play(recipe)

		expect(constructed).toBe(0)
	})

	it('constructs exactly one shared context once enabled', async () => {
		const engine = await freshEngine()
		engine.setEnabled(true)

		engine.play('toggle')
		engine.play('chime')

		// Cached module-wide: a context per sound would exhaust the browser's
		// limit in a session of ordinary use.
		expect(constructed).toBe(1)
	})

	/**
	 * The other half of AC10. A hidden, never-focused WebView2 window has received
	 * no user gesture, so this is the ordinary path there rather than an edge
	 * case — and it has to stay inaudible-but-not-thrown, because these calls sit
	 * inside store mutations that must not fail over a sound.
	 */
	it('does not throw when the browser refuses audio outright', async () => {
		installAudioContext('blocked')
		const engine = await freshEngine()
		engine.setEnabled(true)

		expect(() => {
			for (const recipe of COPPER_RECIPES) engine.play(recipe)
		}).not.toThrow()
	})

	it('does not throw when the context cannot be resumed', async () => {
		const engine = await freshEngine()
		engine.setEnabled(true)

		expect(() => engine.play('tick')).not.toThrow()
		// The rejected `resume()` is handled inside the engine; surfacing it as an
		// unhandled rejection would fail the run.
		await new Promise((resolve) => setTimeout(resolve, 0))
	})

	it('stops constructing again after being disabled mid-session', async () => {
		installAudioContext('blocked')
		const engine = await freshEngine()

		engine.setEnabled(true)
		engine.play('toggle')
		expect(constructed).toBe(1)

		// A blocked constructor caches nothing, so a second play would try again —
		// which is what makes this observable at the constructor at all.
		engine.setEnabled(false)
		engine.play('toggle')
		expect(constructed).toBe(1)
	})

	it('ignores a name that is not a recipe rather than throwing', async () => {
		const engine = await freshEngine()
		engine.setEnabled(true)

		expect(() => engine.play('not-a-sound' as 'toggle')).not.toThrow()
		expect(constructed).toBe(0)
	})
})
