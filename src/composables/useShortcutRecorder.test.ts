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
	it('collapses the side out of every modifier in a chord', async () => {
		// `event.code` matches the Rust parser for main keys and for nothing else:
		// the DOM reports sides, and a chord cannot carry one. Not a simplification
		// — `RegisterHotKey` has no way to say *which* Ctrl — so `ControlLeft` and
		// `ControlRight` must both arrive as plain `Ctrl`.
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'ControlLeft')
		press(recorder, 'ShiftRight')
		press(recorder, 'KeyK')
		await Promise.resolve()

		expect(committed()).toBe('Ctrl+Shift+K')
	})

	it('keeps the side on a double-tap, which is the one shape that can carry it', async () => {
		// The other half of the split above. A double-tap is recognised by Copper's
		// own keyboard hook, which sees the two Ctrl keys as the different keys they
		// are — so the binding records the key the user actually tapped.
		for (const [code, expected] of [
			['ControlLeft', 'LCtrl LCtrl'],
			['ControlRight', 'RCtrl RCtrl'],
			['ShiftLeft', 'LShift LShift'],
			['ShiftRight', 'RShift RShift'],
			['AltLeft', 'LAlt LAlt'],
			['AltRight', 'RAlt RAlt'],
		] as const) {
			// `committed()` reads the first commit of the run, so each key starts from
			// a clean call log as well as a clean module.
			mocks.invoke.mockClear()
			const recorder = await freshRecorder()
			await recorder.start('capture')

			press(recorder, code)
			release(recorder, code)
			await Promise.resolve()

			expect(committed()).toBe(expected)
		}
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

	it('drops a modifier once it is released', async () => {
		// The bug this covers: `held` only ever grew, so a Ctrl pressed and released
		// on the way to Shift+K committed `Ctrl+Shift+K` — a chord nobody held.
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'ControlLeft')
		press(recorder, 'ShiftLeft')
		release(recorder, 'ControlLeft')

		// And the display follows, rather than leaving a chip for a key that is up.
		expect([...recorder.pending.value]).toEqual(['Shift'])

		press(recorder, 'KeyK')
		await Promise.resolve()

		expect(committed()).toBe('Shift+K')
	})

	it('refuses a key with no modifier, and keeps recording so the next try works', async () => {
		// A bare binding would be taken from every other app on the machine. Rust
		// refuses it too; this is the layer that stops it being sent at all.
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'KeyK')
		await Promise.resolve()

		expect(committed()).toBeNull()
		expect(recorder.isRecording.value).toBe(true)

		press(recorder, 'ControlLeft')
		press(recorder, 'KeyK')
		await Promise.resolve()
		expect(committed()).toBe('Ctrl+K')
	})

	it('allows the high function keys bare, since that is what they are for', async () => {
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'F13')
		await Promise.resolve()

		expect(committed()).toBe('F13')
	})

	it('still refuses F1 to F12, which every other app is listening for', async () => {
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'F5')
		await Promise.resolve()

		expect(committed()).toBeNull()
		expect(recorder.isRecording.value).toBe(true)
	})

	it('takes a bare modifier as a double-tap, which summon can now be bound to', async () => {
		// This used to be refused: `Shift Shift` was a *capture* shape and releasing
		// a lone modifier while recording summon committed nothing. The keyboard hook
		// runs a recogniser per role from task-020, so the gesture means the same
		// thing whichever row it is recorded in.
		const recorder = await freshRecorder()
		await recorder.start('summon')

		press(recorder, 'ShiftLeft')
		release(recorder, 'ShiftLeft')
		await Promise.resolve()

		expect(committed()).toBe('LShift LShift')
		expect(recorder.isRecording.value).toBe(false)
	})
})

/**
 * Every case here runs against **both** rows. The double-tap shape stopped being
 * capture's alone in task-020, and a rule that held for one row and not the other
 * would be exactly the kind of divergence a shared recorder is supposed to make
 * impossible.
 */
describe.each(['capture', 'summon'] as const)('recording a %s trigger', (which) => {
	it('commits a double-tap on the release of a lone modifier', async () => {
		const recorder = await freshRecorder()
		await recorder.start(which)

		press(recorder, 'ControlRight')
		release(recorder, 'ControlRight')
		await Promise.resolve()

		expect(committed()).toBe('RCtrl RCtrl')
	})

	it('still takes a conventional chord, since R-Q52 allows either shape', async () => {
		const recorder = await freshRecorder()
		await recorder.start(which)

		press(recorder, 'ControlLeft')
		press(recorder, 'AltLeft')
		press(recorder, 'KeyC')
		await Promise.resolve()

		expect(committed()).toBe('Ctrl+Alt+C')
	})

	it('does not turn the tail of a chord into a double-tap', async () => {
		// The releases after `Ctrl+C` must not commit a second time. Recording is
		// already over by then, which is what makes this safe rather than lucky.
		const recorder = await freshRecorder()
		await recorder.start(which)

		press(recorder, 'ControlLeft')
		press(recorder, 'KeyC')
		release(recorder, 'ControlLeft')
		await Promise.resolve()

		const commits = mocks.invoke.mock.calls.filter(([name]) => name === 'commit_shortcut_recording')
		expect(commits).toHaveLength(1)
		expect(committed()).toBe('Ctrl+C')
	})

	it('does not commit a double-tap for a modifier let go of mid-chord', async () => {
		// Ctrl comes up while Shift is still down, so this release is part of
		// building a chord rather than a gesture of its own.
		const recorder = await freshRecorder()
		await recorder.start(which)

		press(recorder, 'ControlLeft')
		press(recorder, 'ShiftLeft')
		release(recorder, 'ControlLeft')
		await Promise.resolve()

		expect(committed()).toBeNull()
		expect(recorder.isRecording.value).toBe(true)
	})

	it('refuses a double-tap of a modifier the hook cannot watch', async () => {
		// Win fights the Start menu, which opens on the release of a bare press —
		// and it has no sided spelling either, for the same reason.
		const recorder = await freshRecorder()
		await recorder.start(which)

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

	it('stands down when the session ends while the lease is still in flight', async () => {
		// The window that unmounting the settings view falls into: `start` cannot
		// install any state until Rust hands back a token, and the view can be gone
		// by then. Installing it anyway left a recording row nobody opened, still
		// showing on the next visit and answering no keystroke.
		let release: (token: number) => void = () => {}
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'begin_shortcut_recording') {
				return new Promise<number>((resolve) => {
					release = resolve
				})
			}
			return null
		})
		const recorder = await freshRecorder()

		const pending = recorder.start('summon')
		// The view goes away mid-await, exactly as `onBeforeUnmount` does.
		await recorder.cancel()
		release(7)

		expect(await pending).toBe(false)
		expect(recorder.isRecording.value).toBe(false)
		// And the lease Rust really did hand out is given straight back, rather than
		// left open until the watchdog notices.
		expect(mocks.invoke).toHaveBeenCalledWith('cancel_shortcut_recording')
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
