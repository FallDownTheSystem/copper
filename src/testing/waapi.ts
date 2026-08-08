/**
 * The two Web Animations constructors happy-dom does not ship, for the suites
 * that exercise auto-animate's plugin path.
 *
 * happy-dom stubs `Element.animate`, which is why the *options* form of
 * auto-animate always worked under test — but the plugin form constructs
 * `new KeyframeEffect(...)` and the library wraps that in `new Animation(...)`,
 * and both names are simply absent from the global. The shim finishes
 * synchronously on `play()`: in a DOM with no layout there is nothing to
 * animate over time, and firing `finish` at once is what lets the library's
 * own `finish` listener run its bookkeeping exactly as it does in a browser.
 *
 * Guarded assignments, so the day happy-dom grows the real constructors this
 * file stops doing anything rather than shadowing them.
 */

class KeyframeEffectShim {
	constructor(
		public target: Element | null,
		public keyframes: Keyframe[] | PropertyIndexedKeyframes | null,
		public options?: number | KeyframeEffectOptions,
	) {}
}

class AnimationShim extends EventTarget {
	playState: AnimationPlayState = 'idle'

	constructor(public effect?: unknown) {
		super()
	}

	/** A microtask, never synchronous: auto-animate attaches its `finish`
	 *  listener *after* it calls `play()`, so a finish dispatched inside the call
	 *  would fire into silence and the library's cleanup — which is what removes
	 *  an exiting row from the DOM — would never run. */
	play() {
		this.playState = 'running'
		queueMicrotask(() => {
			if (this.playState !== 'running') return
			this.playState = 'finished'
			this.dispatchEvent(new Event('finish'))
		})
	}

	finish() {
		this.playState = 'finished'
		this.dispatchEvent(new Event('finish'))
	}

	pause() {
		this.playState = 'paused'
	}

	cancel() {
		this.playState = 'idle'
	}
}

const g = globalThis as Record<string, unknown>
g.KeyframeEffect ??= KeyframeEffectShim
g.Animation ??= AnimationShim
