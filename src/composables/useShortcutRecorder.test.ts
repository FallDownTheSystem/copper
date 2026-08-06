import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

/**
 * The chord builder is the one piece of this task's frontend that can be wrong in
 * a way nothing else notices: it produces a string Rust parses, and a chord that
 * fails to parse there arrives as "Copper couldn't read that shortcut" with no
 * clue that the DOM spelling was the problem.
 */
const mocks = vi.hoisted(() => ({ invoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }))

/** A fresh module per case: the recorder's state is module-scoped by design, so a
 *  cached instance would carry the previous test's session. */
async function freshRecorder() {
	vi.resetModules()
	const module = await import('./useShortcutRecorder')
	return module.useShortcutRecorder()
}

type Recorder = Awaited<ReturnType<typeof freshRecorder>>

function press(recorder: Recorder, code: string, key = code) {
	const event = new KeyboardEvent('keydown', { code, key, cancelable: true })
	recorder.onKeydown(event)
	return event
}

function release(recorder: Recorder, code: string, key = code) {
	const event = new KeyboardEvent('keyup', { code, key, cancelable: true })
	recorder.onKeyup(event)
	return event
}

/** The chord handed to `commit_shortcut_recording`, or null if none was. */
function committed(): string | null {
	const call = mocks.invoke.mock.calls.find(([name]) => name === 'commit_shortcut_recording')
	return call ? (call[1] as { chord: string }).chord : null
}

beforeEach(() => {
	mocks.invoke.mockReset()
	mocks.invoke.mockImplementation(async (command: string) => {
		if (command === 'begin_shortcut_recording') return 7
		return null
	})
})

describe('recording a summon chord', () => {
	it('normalises the DOM spelling of every modifier to the parser token', async () => {
		// `event.code` matches the Rust parser for main keys and for nothing else:
		// the DOM reports sides, and the parser wants SHIFT / CONTROL / ALT / SUPER.
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'ControlLeft')
		press(recorder, 'ShiftRight')
		press(recorder, 'KeyK')
		await Promise.resolve()

		expect(committed()).toBe('Ctrl+Shift+K')
	})

	it('emits modifiers before the main key, in a fixed order', async () => {
		// The parser requires modifiers first and exactly one non-modifier, so the
		// order they were physically pressed in cannot be the order that is sent.
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'ShiftLeft')
		press(recorder, 'AltLeft')
		press(recorder, 'ControlLeft')
		press(recorder, 'Space')
		await Promise.resolve()

		expect(committed()).toBe('Ctrl+Alt+Shift+Space')
	})

	it('leaves main keys the parser already accepts alone', async () => {
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'AltLeft')
		press(recorder, 'ArrowUp')
		await Promise.resolve()

		expect(committed()).toBe('Alt+ArrowUp')
	})

	it('settles on the first non-modifier press rather than waiting for the release', async () => {
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'ControlLeft')
		expect(recorder.isRecording.value).toBe(true)
		press(recorder, 'Digit1')

		// Recording ends on the down stroke, so the row settles while the user is
		// still letting go rather than after.
		expect(recorder.isRecording.value).toBe(false)
		await Promise.resolve()
		expect(committed()).toBe('Ctrl+1')
	})

	it('ignores auto-repeat, because a hold is one press', async () => {
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'ControlLeft')
		recorder.onKeydown(new KeyboardEvent('keydown', { code: 'KeyK', repeat: true }))

		expect(recorder.isRecording.value).toBe(true)
		expect(committed()).toBeNull()
	})

	it('never treats a bare modifier as a binding', async () => {
		// A summon chord needs a main key; releasing Shift on its own must not
		// commit `Shift Shift`, which is a *capture* shape.
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'ShiftLeft')
		release(recorder, 'ShiftLeft')
		await Promise.resolve()

		expect(committed()).toBeNull()
		expect(recorder.isRecording.value).toBe(true)
	})
})

describe('recording a capture trigger', () => {
	it('commits a double-tap on the release of a lone modifier', async () => {
		const recorder = await freshRecorder()
		await recorder.start('capture')

		press(recorder, 'ControlRight')
		release(recorder, 'ControlRight')
		await Promise.resolve()

		expect(committed()).toBe('Ctrl Ctrl')
	})

	it('still takes a conventional chord, since R-Q52 allows either shape', async () => {
		const recorder = await freshRecorder()
		await recorder.start('capture')

		press(recorder, 'ControlLeft')
		press(recorder, 'AltLeft')
		press(recorder, 'KeyC')
		await Promise.resolve()

		expect(committed()).toBe('Ctrl+Alt+C')
	})

	it('does not turn the tail of a chord into a double-tap', async () => {
		// The releases after `Ctrl+Alt+C` must not commit a second time. Recording is
		// already over by then, which is what makes this safe rather than lucky.
		const recorder = await freshRecorder()
		await recorder.start('capture')

		press(recorder, 'ControlLeft')
		press(recorder, 'KeyC')
		release(recorder, 'ControlLeft')
		await Promise.resolve()

		const commits = mocks.invoke.mock.calls.filter(([name]) => name === 'commit_shortcut_recording')
		expect(commits).toHaveLength(1)
		expect(committed()).toBe('Ctrl+C')
	})

	it('refuses a double-tap of a modifier the hook cannot watch', async () => {
		// Win fights the Start menu, which opens on the release of a bare press.
		const recorder = await freshRecorder()
		await recorder.start('capture')

		press(recorder, 'MetaLeft')
		release(recorder, 'MetaLeft')
		await Promise.resolve()

		expect(committed()).toBeNull()
	})
})

describe('leaving the recorder', () => {
	it('treats Escape as cancel and never as a binding', async () => {
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'Escape')
		await Promise.resolve()

		expect(committed()).toBeNull()
		expect(mocks.invoke).toHaveBeenCalledWith('cancel_shortcut_recording')
		expect(recorder.isRecording.value).toBe(false)
	})

	it('lets Tab move focus instead of preventing it', async () => {
		// Preventing Tab would strand the user inside the recorder — the opposite of
		// the way out it is there to provide.
		const recorder = await freshRecorder()
		await recorder.start('summon')

		const event = press(recorder, 'Tab')

		expect(event.defaultPrevented).toBe(false)
		expect(recorder.isRecording.value).toBe(false)
		await Promise.resolve()
		expect(mocks.invoke).toHaveBeenCalledWith('cancel_shortcut_recording')
	})

	it('prevents every other key, so a chord cannot also do what it normally does', async () => {
		const recorder = await freshRecorder()
		await recorder.start('summon')

		expect(press(recorder, 'ControlLeft').defaultPrevented).toBe(true)
		expect(press(recorder, 'KeyF').defaultPrevented).toBe(true)
	})

	it('records nothing when the lease could not be taken', async () => {
		// Rust owns the lease; without one the live chords are still registered and
		// would fire instead of being captured.
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'begin_shortcut_recording') throw new Error('busy')
			return null
		})
		const recorder = await freshRecorder()

		expect(await recorder.start('summon')).toBe(false)
		expect(recorder.isRecording.value).toBe(false)

		// And a stray key with no session open changes nothing.
		expect(press(recorder, 'KeyK').defaultPrevented).toBe(false)
		expect(committed()).toBeNull()
	})
})
