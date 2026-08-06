/**
 * The recents collection, its availability state, and the open/create/switch/
 * remove actions.
 *
 * **Plural**, and distinct from `useSpace` — which holds the one open document.
 * That is the agreed amendment to task-004's "only `useSpace.ts` calls `invoke`"
 * rule, restated there as one adapter per Rust surface: this file may invoke the
 * space-*management* commands, and it never loads a document itself. When a
 * switch succeeds, `activate_space` hands back the authoritative `Space` and this
 * file passes it straight to `useSpace().adopt()`.
 *
 * Two things it deliberately does not do:
 *
 * - **It never switches on its own.** No startup heuristic, no foreground-app
 *   inference. If the active space was unavailable at startup the store has
 *   already re-pointed to a loadable recents entry, and this reflects whatever is
 *   actually active afterwards rather than initiating anything.
 * - **It never starts a probe from an event.** `settings-changed` does re-list
 *   recents and pull the document — see `onSettingsChanged` for why that pull is
 *   load-bearing — but listing is a pure read of cached availability. Probing is
 *   started only by `refresh_recents`, on a menu open or an explicit retry;
 *   probing from the event would close a loop, since probe results would then ask
 *   for another list.
 */

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

import { createStartup } from '@/lib/startup'

import { errorMessage, type Space } from './useSpace'

/** Only `unavailable` carries a cause. `unresponsive` is a transient state —
 *  the probe has concluded nothing, which is not the same as concluding that a
 *  drive is gone. */
export type UnavailableReason =
	| 'drive-unavailable'
	| 'missing'
	| 'not-a-file'
	| 'unreadable'
	| 'invalid'

export type Availability =
	| { state: 'pending' }
	| { state: 'available' }
	| { state: 'unresponsive'; message: string }
	| { state: 'unavailable'; reason: UnavailableReason; message: string }

export type RecentEntry = {
	path: string
	displayPath: string
	/** The lexical identity Rust patches rows by. Never shown. */
	key: string
	name: string
	active: boolean
	availability: Availability
}

/** `changed: true` always carries the document; `changed: false` always carries
 *  `null`, and is what preserves scroll position and selection on a redundant
 *  open. */
export type ActivateOutcome = { changed: boolean; space: Space | null }

type AvailabilityChanged = {
	generation: number
	key: string
	availability: Availability
	name: string | null
}

// --- module-scope state ------------------------------------------------------

const recents = ref<RecentEntry[]>([])

/**
 * Serialised through a promise tail, because response order is not request
 * order.
 *
 * Two overlapping `list_recents` calls can resolve in either order, and the
 * loser would then overwrite the fresher list with a stale one — with nothing
 * further coming to correct it. Chaining each call behind the previous one costs
 * a menu open nothing and removes the interleaving entirely. A counter would not
 * do: it assumes the backend answers in the order it was asked, which is the
 * assumption being ruled out.
 */
let tail: Promise<void> = Promise.resolve()

function refresh(): Promise<void> {
	tail = tail.then(async () => {
		try {
			recents.value = await invoke<RecentEntry[]>('list_recents')
		} catch (error) {
			console.error('[copper] could not list recent spaces', error)
		}
	})
	return tail
}

/**
 * Every row with that key, patched in place.
 *
 * **Every**, not the first: a comparison key is deliberately many-to-one over
 * stored paths, so a hand-edited `%APPDATA%` entry and the same file opened
 * through the picker are two rows sharing one key. Rust probes such a key once,
 * so there is exactly one result for both rows — and patching only the first
 * would leave the second saying "Checking…" forever, with no further event
 * coming.
 *
 * Rust already discards a result whose snapshot is stale or whose entry has been
 * removed, so an unknown key here is not an error to report — it is an entry
 * that left the list while its probe was in flight.
 */
function patch(result: AvailabilityChanged) {
	for (const entry of recents.value) {
		if (entry.key !== result.key) continue
		entry.availability = result.availability
		if (result.name) entry.name = result.name
	}
}

/**
 * `settings-changed` is the **only** signal that a space was opened by something
 * the frontend did not invoke.
 *
 * This is load-bearing rather than tidy. `store::open_space` emits exactly one
 * `settings-changed` on its happy path and no `space-changed` — the latter is
 * reserved for watcher reloads, captures and editor read-backs — so a forwarded
 * launch or an Explorer double-click into an already-running app changes the
 * active document with nothing else announcing it. Re-listing recents alone
 * would leave the panel revealed and still rendering the *previous* space's
 * notes, which is precisely the failure the launch path exists to avoid.
 *
 * Refreshed unconditionally rather than only when the path looks new: the event
 * carries no payload to compare against, `refresh()` in `useSpace` is already
 * single-in-flight with a monotonic guard, and its epoch resets selection and
 * focus when document identity actually changes. A burst of three forwarded
 * opens therefore costs one pull, and a `remove_recent` that changed no document
 * costs one that applies the same document and nothing else.
 */
async function onSettingsChanged() {
	await Promise.all([refresh(), useSpace().refresh()])
}

const { initialize, dispose } = createStartup(
	() =>
		Promise.all([
			listen<AvailabilityChanged>('spaces-availability-changed', (event) => patch(event.payload)),
			listen('settings-changed', () => void onSettingsChanged()),
		]),
	refresh,
)

/** Called when the `...` menu opens and on an explicit retry. The only thing
 *  that starts probes, which is why listing can stay a pure read. */
async function probeRecents() {
	try {
		await invoke('refresh_recents')
	} catch (error) {
		console.error('[copper] could not start availability probes', error)
	}
}

// --- actions -----------------------------------------------------------------

const { adopt, reportActionError, clearActionError } = useSpace()

/**
 * Every action funnels through here so a failure lands in the same place a
 * failed list mutation does, which is the band `StatusLine` renders.
 *
 * On success with `changed: false` nothing happens at all — that is the
 * already-active case, and reloading an identical document would cost the
 * selection and the scroll position for nothing.
 */
async function activate(run: () => Promise<ActivateOutcome>): Promise<ActivateOutcome | null> {
	clearActionError('list')
	let outcome: ActivateOutcome
	try {
		outcome = await run()
	} catch (error) {
		reportActionError('list', errorMessage(error))
		return null
	}

	// Nothing to re-read on an unchanged outcome: A23 returns before the store is
	// touched, so no settings write, no event and no availability result happened
	// — and a cancelled dialog reports the same. Re-listing would be a round trip
	// that returns the list already on screen, and would replace the array and
	// re-render the menu for it.
	if (!outcome.changed) return outcome
	if (outcome.space) await adopt(outcome.space)
	await refresh()
	return outcome
}

/** Switching and opening are the same operation: selecting a recents entry calls
 *  this, and there is no separate `switchSpace`. */
function openSpace(path: string) {
	return activate(() => invoke<ActivateOutcome>('activate_space', { path }))
}

function pickAndOpenSpace() {
	return activate(() => invoke<ActivateOutcome>('pick_and_open_space'))
}

function createSpace() {
	return activate(() => invoke<ActivateOutcome>('create_space_interactive'))
}

/** Refused by Rust on the active entry as well, so the disabled control in the
 *  menu is a courtesy rather than the enforcement. */
async function removeRecent(path: string) {
	clearActionError('list')
	try {
		await invoke('remove_recent', { path })
	} catch (error) {
		reportActionError('list', errorMessage(error))
		return false
	}
	await refresh()
	return true
}

export function useSpaces() {
	return {
		recents: readonly(recents),
		initialize,
		dispose,
		refresh,
		probeRecents,
		openSpace,
		pickAndOpenSpace,
		createSpace,
		removeRecent,
	}
}
