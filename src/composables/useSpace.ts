/**
 * The one module that talks to the Rust store, and the one place `space` is
 * ever assigned.
 *
 * Two facts from task-003 are binding here, and getting either wrong ships a
 * panel that silently fails to update:
 *
 * - **Every mutating command returns the updated `Space`** (§8.2), so the
 *   coordinator applies each command's return value directly.
 * - **A mutating command called by the frontend emits nothing** (§8.4). An
 *   adapter that fired `invoke()` and then waited for `space-changed` would hang
 *   forever: the composer would clear and the note would never appear.
 *
 * `space-changed` comes only from the watcher and the capture path, and the
 * correct response to it is a fresh `get_active_space` — the payload carries
 * identity only, by design, so a dropped event costs nothing.
 *
 * The refresh protocol is task-003 §8A.4a: a **single-in-flight coalesced
 * refresh**, not a FIFO queue. Applying responses "in request order" does not
 * fix the race it exists for — the mount pull reads document A, a watcher event
 * installs and applies B, the mount pull then resolves with A and overwrites it,
 * and A stays on screen indefinitely because nothing has changed since and no
 * further event is coming. So: at most one refresh in flight, an event during
 * one schedules exactly one more, and a late response for a superseded refresh
 * is **discarded rather than applied**. Mutation responses are subject to the
 * same discard rule.
 *
 * (This is load-bearing only because task-003 declined a store-wide monotonic
 * revision counter — its Open Question 8, still pending. With the counter this
 * collapses to a structural version check.)
 */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { DeepReadonly } from 'vue'

import { noteRow, useSelection, type SelectionSnapshot } from './useSelection'
import { useMarkdown } from './useMarkdown'
import { useNoteDisclosure } from './useNoteDisclosure'
import { useNoteEditor } from './useNoteEditor'

// --- the document, mirroring task-003 exactly --------------------------------

export type Note = {
	id: string
	section: string
	order: number
	done: boolean
	body: string
	created: string
	updated: string
}

export type Section = { id: string; name: string; order: number }

/** `activeSection` is an **id string**, matching the document. The resolved
 *  object is exposed separately as `activeSectionObject`. */
export type Space = {
	id: string
	name: string
	activeSection: string
	sections: Section[]
	notes: Note[]
}

export type StoreStatus = {
	path: string | null
	errored: boolean
	watching: boolean
	canUndo: boolean
	canRedo: boolean
	startupNotice: string | null
}

export type Settings = {
	recents: string[]
	activeSpace: number
	panelPosition: { x: number; y: number } | null
	shortcuts: Record<string, string>
	theme: string
}

/**
 * The document as everything outside the coordinator sees it.
 *
 * `useSpace` exposes `space` through `readonly()`, so consumers receive a
 * deeply-readonly view — which is the point of "one coordinator owns the
 * document", but means every signature they hand it to has to say so. A mutable
 * `Space` is still assignable to this, so tests and the coordinator itself pass
 * plain objects unchanged.
 */
export type SpaceView = DeepReadonly<Space>
export type NoteView = DeepReadonly<Note>

export type ChangeReason = 'external' | 'capture' | 'reload'
export type SpaceChangedPayload = { id: string; path: string; reason: ChangeReason }
export type StoreErrorPayload = { kind: string; message: string }
export type AddNoteResult = { space: Space; noteId: string }

/** The *initial* pull only. Named `loadState`, not `status`: task-003 already
 *  owns `get_status`/`StoreStatus` and the collision reads as the same thing. */
export type LoadState = 'loading' | 'ready' | 'error'

const EMPTY_STATUS: StoreStatus = {
	path: null,
	errored: false,
	watching: false,
	canUndo: false,
	canRedo: false,
	startupNotice: null,
}

// --- module-scope state ------------------------------------------------------
// Declared here, not inside the exported function: refs created inside would
// hand every caller a private copy, which is the classic Pinia-less bug.

const space = shallowRef<Space | null>(null)
const loadState = ref<LoadState>('loading')
const loadError = ref<string | null>(null)
/** A background reload. Sets `aria-busy`; must never unmount the list or the
 *  editor, and must never become `loadState: 'loading'`. */
const refreshing = ref(false)
/** A failed mutation, rendered next to the surface that produced it. Must never
 *  become `loadState: 'error'`, and must never clear the text it belongs to. */
const actionError = ref<string | null>(null)
const storeStatus = ref<StoreStatus>(EMPTY_STATUS)
const storeErrorEvent = ref<StoreErrorPayload | null>(null)
const settings = ref<Settings | null>(null)
/** Bumped when `space.id` changes. Ids are unique only *within* a document, so
 *  a checkout that swaps the space must not carry selection, focus, expansion,
 *  the Markdown cache or an edit session onto a coincidentally matching id. */
const epoch = ref(0)
/** Rows mid-transform report transformed offsets, which invalidates the pixel
 *  offset a scroll restore is anchored on and makes an external reload thrash. */
const listAnimated = ref(false)

// --- coordinator internals ---------------------------------------------------

/** Bumped on every *applied* document. A response issued under an older value
 *  has been superseded. */
let generation = 0
let refreshInFlight = false
let refreshQueued = false
let initPromise: Promise<void> | null = null
let unlisteners: UnlistenFn[] = []

const selection = useSelection()
const markdown = useMarkdown()
const disclosure = useNoteDisclosure()
const editor = useNoteEditor()

function errorMessage(error: unknown): string {
	if (error && typeof error === 'object' && 'message' in error) {
		return String((error as { message: unknown }).message)
	}
	return String(error)
}

function emptySnapshot(): SelectionSnapshot {
	return {
		noteIds: [],
		focusedId: null,
		anchorId: null,
		activeRowId: null,
		inTextSurface: false,
		scroll: null,
	}
}

/**
 * The single assignment point.
 *
 * Returns false when the response was superseded, in which case the caller must
 * discard it — not apply it late, and not reorder it.
 */
function applyDocument(
	next: Space,
	issued: { generation: number; epoch: number },
	options: { animate: boolean },
): boolean {
	if (generation !== issued.generation || epoch.value !== issued.epoch) return false

	// Must be taken *before* the assignment: afterwards `visibleNoteIds` holds
	// only the new order and the focused note's former index is gone, so the
	// nearest-survivor rule cannot be evaluated at all.
	const taken = selection.snapshot()
	const identityChanged = space.value !== null && space.value.id !== next.id

	if (identityChanged) {
		epoch.value++
		selection.resetForNewSpace()
		disclosure.reset()
		markdown.clearCache()
	}

	// A different document is reconciled against an *empty* snapshot, so it takes
	// the first-load path. Reconciling against the old one would relocate focus by
	// the previous document's flattened index and could land on a coincidentally
	// matching id — carrying exactly the state the epoch exists to drop.
	const snapshot = identityChanged ? emptySnapshot() : taken

	const animate = options.animate && !identityChanged
	if (!animate) listAnimated.value = false

	space.value = next
	generation++

	selection.syncDocument(next)
	selection.reconcile(snapshot)
	markdown.pruneCache(next.notes.map((note) => note.id))
	editor.reconcile(next, identityChanged)

	void nextTick().then(() => {
		selection.restoreDom(snapshot)
		listAnimated.value = true
	})

	return true
}

/** `canUndo`/`canRedo`/`errored`/`watching` are store state no `Space` payload
 *  carries, so §8.2's "no follow-up round trip" covers document contents only. */
async function pullStatus() {
	try {
		const status = await invoke<StoreStatus>('get_status')
		storeStatus.value = status
		// The banner's message half is only meaningful while the flag it explains
		// is still set.
		if (!status.errored) storeErrorEvent.value = null
	} catch (error) {
		console.error('[copper] could not read store status', error)
	}
}

/**
 * At most one in flight. An event arriving during one sets a trailing-edge flag
 * — not a queue of N — because every payload is identity-only and every refresh
 * reads current store state, so one coalesced refresh is always at least as
 * fresh as the events it replaces.
 */
async function refresh() {
	if (refreshInFlight) {
		refreshQueued = true
		return
	}

	refreshInFlight = true
	refreshing.value = true
	try {
		do {
			refreshQueued = false
			const issued = { generation, epoch: epoch.value }
			try {
				const next = await invoke<Space>('get_active_space')
				// A superseded response is dropped and another refresh scheduled,
				// never applied late.
				if (!applyDocument(next, issued, { animate: false })) refreshQueued = true
			} catch (error) {
				// §3.6 keeps the in-memory document alive when a file becomes
				// unreadable, so the list stays rendered while the banner reports it.
				console.error('[copper] refresh failed', error)
			}
		} while (refreshQueued)
	} finally {
		refreshInFlight = false
		refreshing.value = false
	}
}

/**
 * All three reasons are live and each has exactly one producer.
 *
 * `reload` is the one with distinct behaviour: it is errored-state recovery, and
 * it fires **even when the recovered document is byte-identical**, deliberately
 * bypassing the semantic-no-op suppression that governs every other watcher
 * path — because the `errored` flag clearing is itself the observable change.
 * Which is why the status re-pull below is unconditional: skipping it when the
 * document came back unchanged is the optimisation that leaves the panel saying
 * "this space is unreadable" forever, with no further event coming.
 */
async function onSpaceChanged(_payload: SpaceChangedPayload) {
	await Promise.all([refresh(), pullStatus()])
}

/**
 * Sets the errored status; it does **not** set `loadState: 'error'`. The
 * in-memory document stays rendered while the panel reports that it is stale.
 */
async function onStoreError(payload: StoreErrorPayload) {
	storeErrorEvent.value = payload
	await pullStatus()
}

// --- loading -----------------------------------------------------------------

/**
 * Three calls, not one (task-003 §8A.3). `get_status` exists specifically for
 * this phase and carries what no event can: whether the space is unreadable,
 * whether watching is running, and `startupNotice`, which bootstrap has no emit
 * capability to deliver.
 */
async function load() {
	loadState.value = 'loading'
	loadError.value = null
	const issued = { generation, epoch: epoch.value }

	// Settled independently rather than through `Promise.all`, which would
	// discard the status when the document pull fails — and that is precisely the
	// case that needs it: `retry()` re-opens by `status.path`, so losing the path
	// on the failing load leaves the error state with a retry control that can
	// only fail the same way.
	const [document, status, nextSettings] = await Promise.allSettled([
		invoke<Space>('get_active_space'),
		invoke<StoreStatus>('get_status'),
		invoke<Settings>('get_settings'),
	])

	if (status.status === 'fulfilled') storeStatus.value = status.value
	if (nextSettings.status === 'fulfilled') settings.value = nextSettings.value

	if (document.status === 'rejected') {
		// A store failure must never be indistinguishable from an empty space.
		loadState.value = 'error'
		loadError.value = errorMessage(document.reason)
		return
	}

	applyDocument(document.value, issued, { animate: false })
	loadState.value = 'ready'
}

/**
 * Re-opens the space **by path**. `get_active_space` returns the in-memory
 * document and does not reread a broken file, so retrying it would appear to
 * succeed while changing nothing.
 */
async function retry() {
	const path = storeStatus.value.path
	if (!path) {
		await load()
		return
	}

	loadState.value = 'loading'
	loadError.value = null
	const issued = { generation, epoch: epoch.value }

	try {
		const next = await invoke<Space>('open_space', { path })
		applyDocument(next, issued, { animate: false })
		await pullStatus()
		loadState.value = 'ready'
	} catch (error) {
		loadState.value = 'error'
		loadError.value = errorMessage(error)
	}
}

/**
 * Idempotent, and it **awaits** registration before the first pull.
 *
 * `listen()` returns a promise and registration is not complete when it returns,
 * so "register handlers first" means awaiting all of them — calling them in
 * source order above the pull leaves exactly the lost-event window the rule
 * exists to close.
 */
function initialize(): Promise<void> {
	initPromise ??= (async () => {
		unlisteners = await Promise.all([
			listen<SpaceChangedPayload>('space-changed', (event) => void onSpaceChanged(event.payload)),
			listen<StoreErrorPayload>('store-error', (event) => void onStoreError(event.payload)),
		])
		await load()
	})()

	return initPromise
}

function dispose() {
	for (const unlisten of unlisteners) unlisten()
	unlisteners = []
	initPromise = null
}

// --- mutations ---------------------------------------------------------------

/**
 * Nothing is cleared optimistically and nothing is retried automatically: a
 * failure leaves the text in place with a visible message, because a silently
 * lost capture is the failure mode the whole product is designed against.
 */
async function mutate<T>(
	run: () => Promise<T>,
	document: (result: T) => Space,
	options: { repullStatus?: boolean } = {},
): Promise<T | null> {
	actionError.value = null
	const issued = { generation, epoch: epoch.value }

	let result: T
	try {
		result = await run()
	} catch (error) {
		actionError.value = errorMessage(error)
		return null
	}

	if (applyDocument(document(result), issued, { animate: true })) {
		if (options.repullStatus) {
			// `edit_note` and `set_active_section` take no undo snapshot of their
			// own, but a write that had to be re-applied over an external change
			// clears both stacks and emits nothing — a re-pull is the only way to
			// learn about it.
			await pullStatus()
		} else {
			// Deterministic for an ordinary structural mutation, so no round trip.
			storeStatus.value = { ...storeStatus.value, canUndo: true, canRedo: false }
		}
	} else {
		// Superseded by a newer applied refresh. Drop it and pull again rather
		// than writing a stale document over a fresh one.
		void refresh()
	}

	return result
}

async function addNote(body: string) {
	// No `section` argument: the store already defaults to `activeSection`, and
	// sending our own view of it would race an external change to it.
	const result = await mutate(
		() => invoke<AddNoteResult>('add_note', { body }),
		(value) => value.space,
	)
	// The roving target follows the new note; DOM focus stays in the composer so
	// consecutive captures need no mouse.
	if (result) selection.focusRow(noteRow(result.noteId))
	return result
}

/** Maps to `edit_note` — not to a command named after this method. */
async function updateNoteBody(id: string, body: string) {
	return mutate(
		() => invoke<Space>('edit_note', { id, body }),
		(value) => value,
		{ repullStatus: true },
	)
}

/** There is no singular set-done command; `set_notes_done` takes an array. Phase
 *  5 calls this with a whole selection, with no signature change. */
async function setNotesDone(ids: string[], done: boolean) {
	return mutate(
		() => invoke<Space>('set_notes_done', { ids, done }),
		(value) => value,
	)
}

async function setActiveSection(id: string) {
	return mutate(
		() => invoke<Space>('set_active_section', { id }),
		(value) => value,
		{ repullStatus: true },
	)
}

function clearActionError() {
	actionError.value = null
}

// --- derived -----------------------------------------------------------------

/** The store repairs every document it loads into canonical order — sections by
 *  `order`, notes grouped by section and ordered within each group — so the
 *  document order *is* the display order. */
const sections = computed(() => space.value?.sections ?? [])

const notesBySection = computed(() => {
	const grouped = new Map<string, Note[]>()
	for (const section of sections.value) grouped.set(section.id, [])
	for (const note of space.value?.notes ?? []) grouped.get(note.section)?.push(note)
	return grouped
})

function notesInSection(sectionId: string): Note[] {
	return notesBySection.value.get(sectionId) ?? []
}

const activeSection = computed(() => space.value?.activeSection ?? null)

const activeSectionObject = computed(
	() => sections.value.find((section) => section.id === activeSection.value) ?? null,
)

const spaceName = computed(() => space.value?.name ?? '')

const noteCount = computed(() => space.value?.notes.length ?? 0)

const readonlyViews = {
	space: readonly(space),
	loadState: readonly(loadState),
	loadError: readonly(loadError),
	refreshing: readonly(refreshing),
	actionError: readonly(actionError),
	storeStatus: readonly(storeStatus),
	storeErrorEvent: readonly(storeErrorEvent),
	settings: readonly(settings),
	epoch: readonly(epoch),
	listAnimated: readonly(listAnimated),
	sections,
	activeSection,
	activeSectionObject,
	spaceName,
	noteCount,
}

export function useSpace() {
	return {
		...readonlyViews,
		notesInSection,
		initialize,
		dispose,
		load,
		refresh,
		retry,
		addNote,
		updateNoteBody,
		setNotesDone,
		setActiveSection,
		clearActionError,
	}
}
