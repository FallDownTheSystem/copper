/**
 * The keyboard state machine behind "press the keys you want".
 *
 * Module scope, and deliberately so: `SettingsView`'s Escape handler has to be
 * able to ask whether a recording is open. Propagation alone does not cover that
 * — it only shadows the view handler while focus is *inside* the recorder, and
 * one Tab moves focus to Back, to Cancel or to another row while recording is
 * still live. The state is shared by construction, so refusing to read it would
 * buy no decoupling, only a bug.
 *
 * All IPC goes through `useSettings`, which is the one adapter for this Rust
 * surface. What lives here is the keyboard, and nothing else.
 */

import { useSettings, type ShortcutTarget } from './useSettings'

/**
 * The DOM reports sides; the Rust parser does not.
 *
 * Main keys need no table at all — `KeyK`, `Digit1`, `Space`, `ArrowUp` and `F12`
 * come off `event.code` already matching the parser token for token. Every
 * modifier does, which is every chord.
 */
const MODIFIERS: Record<string, string> = {
	ShiftLeft: 'Shift',
	ShiftRight: 'Shift',
	ControlLeft: 'Ctrl',
	ControlRight: 'Ctrl',
	AltLeft: 'Alt',
	AltRight: 'Alt',
	MetaLeft: 'Super',
	MetaRight: 'Super',
}

/** Windows' order, matching how Rust renders a chord back. */
const MODIFIER_ORDER = ['Ctrl', 'Alt', 'Shift', 'Super']

/** Only these three can be double-tapped. Win fights the Start menu and Super is
 *  not offered as a bare trigger at all. */
const DOUBLE_TAPPABLE = new Set(['Shift', 'Ctrl', 'Alt'])

/**
 * The keys a chord may consist of with no modifier at all.
 *
 * F13–F24 exist for exactly this: no keyboard emits them by accident and nothing
 * else is listening for them. Every other single key — F1–F12 very much included,
 * since they are live in almost every application — would be taken from the whole
 * machine by a global binding. Rust refuses these too; this is the layer that
 * stops one being sent at all.
 */
const BARE_KEYS = new Set([
	'F13',
	'F14',
	'F15',
	'F16',
	'F17',
	'F18',
	'F19',
	'F20',
	'F21',
	'F22',
	'F23',
	'F24',
])

/** `KeyK` and `Digit1` are what the parser accepts; `K` and `1` are what a person
 *  reads, and the parser accepts those too. */
function mainKeyLabel(code: string): string {
	if (code.startsWith('Key')) return code.slice(3)
	if (code.startsWith('Digit')) return code.slice(5)
	return code
}

const isRecording = ref(false)
const target = ref<ShortcutTarget>('summon')
/** What has been pressed so far, for the live display. */
const pending = ref<string[]>([])

let token: number | null = null
let held: string[] = []
let sawMainKey = false

/**
 * Bumped by everything that opens or closes a session.
 *
 * `start` has to await the lease before it can install any state, and the view
 * can be left inside that await — an unmount, a panel hide, an Escape. Without a
 * generation to compare against, the continuation installed a recording session
 * over a view that had already gone, and reopening settings showed a row stuck
 * mid-recording that no keystroke would answer.
 */
let session = 0

/** Leaves `session` alone: it is what tells a superseded `start` to stand down. */
function reset() {
	isRecording.value = false
	pending.value = []
	token = null
	held = []
	sawMainKey = false
}

/**
 * Takes the lease before listening.
 *
 * A registered chord is delivered as `WM_HOTKEY` to the registering message
 * window and never reaches this webview's `keydown`, so the live chords have to
 * come down first — otherwise a user pressing their current hotkey while
 * rebinding triggers the old action instead of being recorded.
 */
async function start(which: ShortcutTarget): Promise<boolean> {
	const mine = ++session
	const { beginRecording, cancelRecording } = useSettings()
	const leased = await beginRecording()
	if (leased === null) return false

	// Something ended the session while the lease was in flight. Installing the
	// state now would show a recording row nobody opened — and the lease is real
	// and held by Rust, so it has to be handed back rather than dropped.
	if (mine !== session) {
		await cancelRecording()
		return false
	}

	reset()
	token = leased
	target.value = which
	isRecording.value = true
	return true
}

async function cancel(): Promise<void> {
	// Ahead of the early return, so a `start` still awaiting its lease is
	// superseded and gives the lease straight back.
	session += 1
	if (!isRecording.value) return
	reset()
	await useSettings().cancelRecording()
}

async function commit(chord: string): Promise<void> {
	session += 1
	const leased = token
	const which = target.value
	reset()
	if (leased === null) return
	await useSettings().commitRecording(leased, which, chord)
}

/** The chord as it stands, in the parser's order. */
function chordFrom(modifiers: string[], main: string | null): string {
	const ordered = MODIFIER_ORDER.filter((name) => modifiers.includes(name))
	return main ? [...ordered, main].join('+') : ordered.join('+')
}

/**
 * Every key while recording, and the escape hatches that keep it from becoming a
 * trap.
 *
 * `Tab` and `Shift+Tab` end recording **without** `preventDefault`, so native
 * focus movement still happens. Preventing them would strand the user inside the
 * recorder — the opposite of the way out they are there to provide. Every other
 * key is prevented, including `Escape`, which is cancel and never a binding.
 */
function onKeydown(event: KeyboardEvent) {
	if (!isRecording.value) return

	if (event.key === 'Tab') {
		void cancel()
		return
	}

	event.preventDefault()
	event.stopPropagation()

	if (event.key === 'Escape') {
		void cancel()
		return
	}

	// Windows delivers repeated key-downs while a key is physically held; a hold
	// is one press.
	if (event.repeat) return

	const modifier = MODIFIERS[event.code]
	if (modifier) {
		if (!held.includes(modifier)) held.push(modifier)
		pending.value = MODIFIER_ORDER.filter((name) => held.includes(name))
		return
	}

	// A bare key is not a binding. Ignored rather than committed, so the session
	// stays open and the next attempt — the same key with a modifier — simply
	// works; committing it would spend a round trip to be told what is already
	// known here.
	if (held.length === 0 && !BARE_KEYS.has(event.code)) return

	// A chord settles on the first non-modifier press, with whatever is held —
	// waiting for the key-up would leave the user staring at an unchanged row
	// while they let go.
	sawMainKey = true
	const chord = chordFrom(held, mainKeyLabel(event.code))
	pending.value = chord.split('+')
	void commit(chord)
}

/**
 * The bare-modifier case, which has no main key to settle on.
 *
 * Only capture can be a double-tap, and only for the three families the hook
 * recognises. Committing on the release of a lone modifier is the same gesture
 * the binding itself describes, which is why it reads as obvious rather than as a
 * second rule.
 */
function onKeyup(event: KeyboardEvent) {
	if (!isRecording.value) return
	event.preventDefault()
	event.stopPropagation()

	const modifier = MODIFIERS[event.code]
	if (!modifier) return

	// The double-tap decision comes first, because it is about the modifier being
	// released and has to see `held` as it was before the removal below.
	if (
		target.value === 'capture' &&
		!sawMainKey &&
		held.length === 1 &&
		held[0] === modifier &&
		DOUBLE_TAPPABLE.has(modifier)
	) {
		void commit(`${modifier} ${modifier}`)
		return
	}

	// A modifier the user has let go of is not part of the chord they are
	// building. Without this, pressing and releasing Ctrl and then pressing K
	// committed `Ctrl+K` — a chord nobody held — and the stale chip stayed on
	// screen saying so.
	held = held.filter((name) => name !== modifier)
	pending.value = MODIFIER_ORDER.filter((name) => held.includes(name))
}

export function useShortcutRecorder() {
	return {
		isRecording: readonly(isRecording),
		target: readonly(target),
		pending: readonly(pending),
		start,
		cancel,
		onKeydown,
		onKeyup,
	}
}
