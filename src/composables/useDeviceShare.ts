/**
 * The share surface: the relay configuration, and sending a note to the other
 * device.
 *
 * One adapter per Rust surface, the same rule `useSpaces` and `useSettings`
 * record. This file may invoke the five `share_*` commands and nothing else
 * does.
 *
 * **It holds no secret and cannot.** `get_share_config` answers with `tokenSet`
 * and `secretSet` booleans; there is no command that reads a stored value back.
 * The one exception is [`generateSecret`], which receives the value Rust has
 * just created so the user can copy it to their other machine — held in a ref
 * that the Settings view clears when it unmounts, and never re-read from Rust.
 *
 * **Initialised from `App.vue`, not from the Settings view.** The note context
 * menu has to know whether sharing is configured, and a composable that only
 * woke up once Settings had been opened would leave that item in an unknown
 * state for the whole of a first session. [`ready`] is what the menu reads
 * before the first pull resolves: false, so the item is disabled rather than
 * wrongly enabled.
 *
 * **`useDeviceShare`, not `useShare`.** `@vueuse/core` exports a `useShare` of
 * its own — the browser's Web Share API — and `@vueuse/core` is in this
 * project's auto-import list, so a bare `useShare()` anywhere in the app
 * resolves to *that* one. A collision that silently hands a caller the wrong
 * composable is not worth the shorter name, so the file and the function both
 * carry the longer one and there is nothing to remember.
 */

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

import { errorMessage } from '@/lib/rustError'
import { createStartup } from '@/lib/startup'

import { generations } from './useSettings'

/** Which mailbox this device reads and which it writes. The two machines must
 *  differ; if both are the same, nothing is delivered in either direction. */
export type ShareRole = 'first' | 'second'

export type ShareConfig = {
	enabled: boolean
	relayUrl: string
	role: ShareRole
	/** Whether a relay token is stored. Never the token. */
	tokenSet: boolean
	/** Whether a pairing secret is stored. Never the secret. */
	secretSet: boolean
	/** Whether Rust's own `resolve` accepts this configuration — the same check
	 *  the send path and the poller run. Not derivable from the three fields
	 *  above: a URL can be present and refused, a secret stored and the wrong
	 *  length. */
	configured: boolean
	/** The most recent poll failure, or null. Set by Rust, cleared by Rust. */
	lastError: string | null
}

/**
 * A partial update. The two secret fields are three-state: leave the key out to
 * keep the stored value, pass a string to replace it, pass `null` to clear it.
 */
export type ShareConfigPatch = {
	enabled?: boolean
	/** Setting this to a different value **clears the stored relay token**, so a
	 *  credential is never sent to a host it was not issued for. */
	relayUrl?: string
	role?: ShareRole
	token?: string | null
	secret?: string | null
}

export type ShareTestOutcome =
	| { kind: 'ok' }
	| { kind: 'unauthorised' }
	| { kind: 'unconfigured'; missing: string }
	| { kind: 'unreachable'; message: string }

export type ShareSendOutcome =
	| { kind: 'sent'; notes: number }
	/** The relay stored it but did not announce it. The next send announces it. */
	| { kind: 'delayed'; notes: number }
	/** The request began and its answer never arrived. It may have been delivered. */
	| { kind: 'unknown'; message: string }
	| { kind: 'too-large'; bytes: number; limit: number }
	| { kind: 'unconfigured'; missing: string }
	| { kind: 'failed'; message: string }

const DEFAULTS: ShareConfig = {
	enabled: false,
	relayUrl: '',
	role: 'first',
	tokenSet: false,
	secretSet: false,
	configured: false,
	lastError: null,
}

// Module scope, so every caller reads one value — the same singleton shape every
// other composable in this directory uses in place of a store.
const config = ref<ShareConfig>({ ...DEFAULTS })
/** False until the first pull resolves. The context menu reads this before it
 *  decides whether **Send to my other device** is available. */
const ready = ref(false)
/** A failed patch or test, shown under the Share section. Distinct from
 *  `config.lastError`, which is the *poller's* most recent failure. */
const actionError = ref<string | null>(null)
/** The value `generateSecret` just created, shown once. Never pulled. */
const revealedSecret = ref<string | null>(null)
const testing = ref(false)
/** True while a Generate is in flight, so the button cannot start a second one. */
const generating = ref(false)
const lastTest = ref<ShareTestOutcome | null>(null)

/**
 * Which answer about the configuration is allowed to win.
 *
 * `useSettings`' counter, reused rather than reinvented — and the reason it
 * compares against what has been **applied** rather than what has been *issued*
 * is the one stated there: a newer request that goes on to reject applies
 * nothing, so discarding an older success on its behalf would leave a value that
 * reached `share.json` and never reached the screen.
 *
 * The case that needs it here is not hypothetical. `share-changed` fires when
 * the poller's `lastError` changes, and Rust emits **no second event for an
 * unchanged error** — so a slow pull landing after the one that event triggered
 * would overwrite the new failure with the old state and hide it for good.
 */
const generation = generations()

async function pullConfig() {
	const issued = generation.issue()
	try {
		const pulled = await invoke<ShareConfig>('get_share_config')
		// The object check is not defensive padding, it is the type's promise: every
		// caller — the Settings section, the context menu's enabled state — reads
		// `config.value` unconditionally, so `ref<ShareConfig>` has to stay true. A
		// reply that is not an object leaves the previous value standing rather than
		// turning every reader into a null dereference.
		if (pulled && typeof pulled === 'object' && generation.settle(issued)) config.value = pulled
	} catch (error) {
		if (generation.settle(issued)) actionError.value = errorMessage(error)
	} finally {
		// In `finally`, so a failing pull still lets the context menu settle on
		// "disabled" rather than leaving it in an unknown state for ever. Outside
		// the generation guard for the same reason: it is a fact about this session,
		// not about which answer won.
		ready.value = true
	}
}

/**
 * Applies a patch and adopts the reply.
 *
 * The reply is applied rather than a re-pull awaited, for the same reason every
 * `useSettings` setter does it: Rust emits nothing for a change the frontend
 * initiated, so waiting for an event would wait for ever.
 */
async function patchConfig(patch: ShareConfigPatch): Promise<boolean> {
	actionError.value = null
	const issued = generation.issue()
	// A patch that sets or clears the secret makes any revealed value a claim
	// about a secret that is no longer stored — worse than showing nothing,
	// because the user would go and copy it to the other machine.
	if (patch.secret !== undefined) clearRevealedSecret()
	try {
		const updated = await invoke<ShareConfig>('set_share_config', { patch })
		if (generation.settle(issued)) {
			config.value = updated
			// A configuration change makes the previous test result a claim about a
			// setup that no longer exists.
			lastTest.value = null
		}
		return true
	} catch (error) {
		if (generation.settle(issued)) actionError.value = errorMessage(error)
		return false
	}
}

/**
 * Creates a pairing secret and reveals it once.
 *
 * The only moment a secret value exists on this side of the boundary. It is put
 * in a ref rather than returned so the Settings view can keep showing it while
 * the user copies it across, and [`clearRevealedSecret`] is what takes it back.
 */
async function generateSecret(): Promise<boolean> {
	// Serialised rather than merely guarded on arrival. Two overlapping generates
	// would each store their secret in Rust and then race to display one, so the
	// value on screen could be the one the *loser* replaced.
	if (generating.value) return false
	actionError.value = null
	generating.value = true
	const issued = ++revealGeneration
	try {
		const { secret } = await invoke<{ secret: string }>('generate_share_secret')
		// Dropped rather than shown when it is no longer the current reveal: the
		// Settings view has closed and already cleared it, or the secret has since
		// been replaced. Showing a secret that is not the stored one is worse than
		// showing none, because the user would copy it to the other machine.
		if (issued === revealGeneration) {
			revealedSecret.value = secret
			lastTest.value = null
		}
		return true
	} catch (error) {
		actionError.value = errorMessage(error)
		return false
	} finally {
		generating.value = false
		// **Outside the reveal guard, and always.** The secret is stored in Rust
		// whatever this side decided to display, so `secretSet` has to catch up even
		// when the reveal was dropped — otherwise closing Settings mid-generate
		// leaves the row reading `Not set` over a secret that exists.
		await pullConfig()
	}
}

/** Which reveal is the current one. Bumped by a new Generate **and** by every
 *  clear, so a reply landing after the view has gone finds itself stale. */
let revealGeneration = 0

function clearRevealedSecret() {
	revealGeneration++
	revealedSecret.value = null
}

async function testRelay(): Promise<ShareTestOutcome | null> {
	actionError.value = null
	testing.value = true
	try {
		lastTest.value = await invoke<ShareTestOutcome>('share_test_relay')
		return lastTest.value
	} catch (error) {
		actionError.value = errorMessage(error)
		return null
	} finally {
		testing.value = false
	}
}

/**
 * Sends the named notes, and answers with the outcome rather than throwing.
 *
 * A rejection is normalised into `{ kind: 'failed' }` so the caller has exactly
 * one shape to switch on — `useNoteActions` reports every branch, and a thrown
 * error there would be the one path with no message.
 */
async function sendNotes(ids: string[]): Promise<ShareSendOutcome> {
	try {
		return await invoke<ShareSendOutcome>('share_send_notes', { ids })
	} catch (error) {
		return { kind: 'failed', message: errorMessage(error) }
	}
}

const { initialize, dispose } = createStartup(
	// One listener, so `createStartup`'s array is built from it directly rather
	// than wrapped in a `Promise.all` of one.
	async () => [
		// Emitted by the poll thread when `lastError` changes value, so a failure
		// that happened while Settings was already open reaches the view without it
		// having to poll for one.
		await listen('share-changed', () => void pullConfig()),
	],
	pullConfig,
)

/**
 * Whether **Send to my other device** can do anything.
 *
 * `configured` is **Rust's own answer** — the result of the same `resolve` the
 * send path and the poller run — rather than a second, weaker validation written
 * here. A frontend rule of its own would enable the item for a URL carrying a
 * query string, or for a pairing secret of the wrong length, and the one
 * direction a network action must not fail in is "looked ready and was not".
 */
const canSend = computed(() => ready.value && config.value.enabled && config.value.configured)

export function useDeviceShare() {
	return {
		config: readonly(config),
		ready: readonly(ready),
		canSend,
		actionError: readonly(actionError),
		revealedSecret: readonly(revealedSecret),
		testing: readonly(testing),
		generating: readonly(generating),
		lastTest: readonly(lastTest),
		initialize,
		dispose,
		pullConfig,
		patchConfig,
		generateSecret,
		clearRevealedSecret,
		testRelay,
		sendNotes,
	}
}
