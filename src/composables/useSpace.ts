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

import { errorMessage } from '@/lib/rustError'

import { useAttachments, type Attachment } from './useAttachments'
import { emptySnapshot, noteRow, useSelection } from './useSelection'
import { useMarkdown } from './useMarkdown'
import { useNoteDisclosure } from './useNoteDisclosure'
import { useNoteEditor } from './useNoteEditor'
import { useEditorHandoff } from './useEditorHandoff'
import { useNoteSearch } from './useNoteSearch'
import { useSectionEditor } from './useSectionEditor'
import { useSections } from './useSections'
import { useSounds } from './useSounds'

// --- the document, mirroring task-003 exactly --------------------------------

export type Note = {
	id: string
	section: string
	order: number
	done: boolean
	body: string
	/** Absent on a note with none — the field is omitted from the document
	 *  entirely, which is what keeps every pre-task-011 file byte-identical. */
	attachments?: Attachment[]
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
	sounds: boolean
	/** `'auto' | 'off'` — see `MotionPreference` in `useSettings`, which is where
	 *  the value is narrowed. Typed loosely here for the same reason `theme` is:
	 *  this mirrors the file, and the file can hold anything. */
	motion: string
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

export type ChangeReason = 'external' | 'capture' | 'reload' | 'editor'
export type SpaceChangedPayload = { id: string; path: string; reason: ChangeReason }
export type StoreErrorPayload = { kind: string; message: string }
/** What a composer submission turned out to be. `# Name` is classified in Rust,
 *  above the store, so the composer path and the capture path cannot drift.
 *
 *  There is no type for `add_note` here any more: the panel submits through
 *  `submit_entry`, and `add_note` is now reached only from Rust, by the capture
 *  path. */
export type SubmitOutcome = 'note' | 'section-created' | 'section-activated'
export type SubmitResult = {
	space: Space
	outcome: SubmitOutcome
	/** Null on both section outcomes — no note was created. */
	noteId: string | null
	sectionId: string
}

/** The *initial* pull only. Named `loadState`, not `status`: task-003 already
 *  owns `get_status`/`StoreStatus` and the collision reads as the same thing. */
export type LoadState = 'loading' | 'ready' | 'error'

/** Which surface a failed mutation belongs to. A failure has to render next to
 *  the text it left in place, and one global string put the editor's error under
 *  the composer as well. */
export type ActionErrorScope = 'composer' | 'editor' | 'list'
export type ActionError = { scope: ActionErrorScope; message: string }

/**
 * What a mutation gives back: the command's own result, plus whether the
 * document it returned is the one now on screen.
 *
 * The two are genuinely separate. A superseded response means the store carried
 * the mutation out — so this is not a failure — but a fresher document had
 * already landed and this one was discarded, with a refresh scheduled behind it.
 * A caller that then moves focus or the selection on the strength of its own
 * result would be reasoning about a document nobody is looking at.
 */
export type MutationResult<T> = { value: T; applied: boolean }

/** Whether a mutation's own document is the one now on screen. `null` — the
 *  mutation failed — is not applied either. */
export function applied<T>(result: MutationResult<T> | null): boolean {
	return result?.applied ?? false
}

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
const actionError = ref<ActionError | null>(null)
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
/** Long enough for a checkout's unlink-and-rewrite window to close, short
 *  enough not to read as a hang. */
const REFRESH_RETRY_MS = 60
let initPromise: Promise<void> | null = null
let unlisteners: UnlistenFn[] = []

const selection = useSelection()
const markdown = useMarkdown()
const disclosure = useNoteDisclosure()
const editor = useNoteEditor()
const search = useNoteSearch()
const sectionEditor = useSectionEditor()
/** Named for the state rather than the module: `sections` below is the
 *  document's own list, and the two are different things. */
const sectionState = useSections()
const handoff = useEditorHandoff()
const attachmentState = useAttachments()

/** Re-exported for the adapters beside this one, which have always taken it from
 *  here. It moved to `lib/` when `useAttachments` needed it too — importing it
 *  from an adapter this module imports would close a cycle. */
export { errorMessage }

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
		// Content hashes never go stale, so this is not the cache-invalidation the
		// others are — it is a *revoke*. The blobs live in the previous space's
		// assets directory, and holding object URLs for bytes no note can reference
		// any more would leak them for the life of the process.
		attachmentState.clearPreviews()
		// The pending tray goes with them, and for a sharper reason: its blobs were
		// written into the *previous* space's assets directory, so submitting them
		// here would write this document with references to files that are not, and
		// never will be, beside it. Rust refuses that submission independently —
		// but leaving the tray populated would mean showing files that cannot be
		// attached and failing only when the user pressed Enter.
		attachmentState.clearPending()
		// Collapse and the switcher are document-scoped: section ids mean something
		// else now, and an open switcher is closed rather than re-pointed.
		sectionState.reset()
	} else {
		// Before the assignment, so a section revealed by a new note is on screen for
		// the same flush the scroll pin measures. After it, `previous` is gone and
		// there is nothing to diff against.
		sectionState.reconcile(space.value, next)
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

	// Before `syncDocument`, because the orders it feeds are filtered by the
	// index's own result set: rebuilding afterwards would leave one frame in which
	// the new document is grouped against the previous document's matches.
	search.rebuild(next)
	selection.syncDocument(next)
	selection.reconcile(snapshot)
	markdown.pruneCache(next.notes.map((note) => note.id))
	editor.reconcile(next, identityChanged)
	sectionEditor.reconcile(next)

	// Rust owns handoff reconciliation — it is the only side that knows the temp
	// path and the baseline bytes — but it has no signal that covers every writer:
	// task-003 §8.4 emits nothing for a command the frontend invoked, so a
	// Rust-side hook on `space-changed` would miss precisely the undo, merge and
	// mark-done cases the reconciliation exists for. This path sees all of them.
	// Skipped entirely when nothing is handed off, which is the ordinary case.
	if (handoff.activeHandoffIds.value.size > 0) void handoff.reconcile()

	void nextTick().then(() => {
		selection.restoreDom(snapshot)
		listAnimated.value = true
	})

	return true
}

/**
 * `canUndo`/`canRedo`/`errored`/`watching` are store state no `Space` payload
 * carries, so §8.2's "no follow-up round trip" covers document contents only.
 *
 * Under the same discard discipline as the document, and for the same reason:
 * two `get_status` calls can be in flight at once — an event handler's and a
 * mutation's — and nothing makes them resolve in issue order. A late one
 * carrying `errored: true` landing after the `reload` that cleared it would put
 * the banner back with no further event coming, which is precisely the failure
 * §3.6a exists to prevent.
 */
let statusToken = 0

async function pullStatus() {
	const issued = ++statusToken
	try {
		const status = await invoke<StoreStatus>('get_status')
		if (issued !== statusToken) return
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
	// A failed pull would otherwise drop the refresh entirely: the flag is already
	// cleared, so the event that asked for it is simply lost. One bounded retry
	// covers the transient case — a file briefly absent mid-checkout, which the
	// store's own write path retries for the same reason — without spinning on a
	// space that is genuinely unreadable.
	let retried = false
	try {
		do {
			refreshQueued = false
			const issued = { generation, epoch: epoch.value }
			try {
				const next = await invoke<Space>('get_active_space')
				// A superseded response is dropped and another refresh scheduled,
				// never applied late.
				if (!applyDocument(next, issued, { animate: false })) refreshQueued = true
				retried = false
			} catch (error) {
				// §3.6 keeps the in-memory document alive when a file becomes
				// unreadable, so the list stays rendered while the banner reports it.
				console.error('[copper] refresh failed', error)
				if (!retried) {
					retried = true
					refreshQueued = true
					await new Promise((resolve) => setTimeout(resolve, REFRESH_RETRY_MS))
				}
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
async function onSpaceChanged(payload: SpaceChangedPayload) {
	// The only signal the frontend gets that a capture *succeeded* — there is no
	// `capture://succeeded` event, because a capture is silent on success by
	// design. `append_capture` is the sole producer of this reason and emits only
	// after the write, so this cannot fire for a capture that failed.
	if (payload.reason === 'capture') useSounds().captureSucceeded()
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
	const [pulled, status, nextSettings] = await Promise.allSettled([
		invoke<Space>('get_active_space'),
		invoke<StoreStatus>('get_status'),
		invoke<Settings>('get_settings'),
	])

	if (status.status === 'fulfilled') storeStatus.value = status.value
	if (nextSettings.status === 'fulfilled') settings.value = nextSettings.value

	// An event may have installed a document while this pull was outstanding. If
	// so the panel has something real to show, and replacing it with the fatal
	// error screen would be a strictly worse view of the same store.
	const superseded = generation !== issued.generation

	if (pulled.status === 'rejected') {
		if (superseded) {
			loadState.value = 'ready'
			return
		}
		// A store failure must never be indistinguishable from an empty space.
		loadState.value = 'error'
		loadError.value = errorMessage(pulled.reason)
		return
	}

	applyDocument(pulled.value, issued, { animate: false })
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
		// Superseded by a document that landed while the re-open was in flight:
		// drop this one and pull again rather than writing it over the fresher one.
		if (!applyDocument(next, issued, { animate: false })) void refresh()
		await pullStatus()
		loadState.value = 'ready'
	} catch (error) {
		if (generation !== issued.generation) {
			loadState.value = 'ready'
			return
		}
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
	toSpace: (result: T) => Space,
	// A predicate rather than a flag, because `submit_entry` cannot answer the
	// question until it has run: creating a section is deterministically undoable,
	// activating one takes no snapshot at all, and the outcome says which happened.
	options: { scope: ActionErrorScope; repullStatus?: (result: T) => boolean },
): Promise<MutationResult<T> | null> {
	// Only this surface's own error is cleared: a failure belongs to the text it
	// left in place, and another surface's message is still explaining itself.
	if (actionError.value?.scope === options.scope) actionError.value = null
	const issued = { generation, epoch: epoch.value }

	let result: T
	try {
		result = await run()
	} catch (error) {
		actionError.value = { scope: options.scope, message: errorMessage(error) }
		useSounds().actionFailed()
		return null
	}

	const applied = applyDocument(toSpace(result), issued, { animate: true })

	// Keyed on the command resolving, not on the document being applied: the
	// store carried the mutation out either way, and supersession is a decision
	// this side of the boundary makes about a stale *document*. Skipping the
	// status update there left `canUndo` false after a real, undoable change.
	if (options.repullStatus?.(result)) {
		// `edit_note`, `set_active_section`, and `submit_entry` when it only
		// activated an existing section, take no undo snapshot of their own — but a
		// write that had to be re-applied over an external change clears both stacks
		// and emits nothing, and a re-pull is the only way to learn about it.
		await pullStatus()
	} else {
		// Deterministic for an ordinary structural mutation, so no round trip.
		storeStatus.value = { ...storeStatus.value, canUndo: true, canRedo: false }
	}

	// Drop the stale document and pull again rather than writing it over a
	// fresher one.
	if (!applied) void refresh()

	return { value: result, applied }
}

/**
 * The composer's submit. Maps to `submit_entry`, **not** to `add_note`.
 *
 * `add_note` is still the command the capture path uses from Rust, and it is
 * deliberately not this one: a captured selection whose whole body is `# Name`
 * is an ordinary note, while the same text typed into the composer is a section
 * directive. Both rules live in one Rust module, so the two paths cannot drift.
 *
 * Nothing here inspects the body. Asking the frontend whether a string "looks
 * like a directive" would be a second copy of the rule and the first thing to go
 * stale.
 */
async function submitEntry(body: string, attachments: Attachment[] = []) {
	// No `section` argument: the store already defaults to `activeSection`, and
	// sending our own view of it would race an external change to it.
	//
	// `attachments` carries metadata only — the blobs were written at paste, drop
	// or pick time — so this stays the fast metadata-only write it was before.
	const result = await mutate(
		() => invoke<SubmitResult>('submit_entry', { body, attachments }),
		(value) => value.space,
		{
			// `section-created` is an ordinary structural mutation, so `canUndo` is
			// deterministic. `section-activated` pushed no snapshot, and its effect on
			// the stacks is not knowable from here.
			scope: 'composer',
			repullStatus: (value) => value.outcome === 'section-activated',
		},
	)

	// The roving target follows the new note; DOM focus stays in the composer so
	// consecutive captures need no mouse. Neither section outcome touches focus or
	// the selection — the switch is visible in the chip and the header instead.
	if (result?.applied && result.value.outcome === 'note' && result.value.noteId) {
		selection.focusRow(noteRow(result.value.noteId))
	}
	return result
}

/** Maps to `edit_note` — not to a command named after this method. */
async function updateNoteBody(id: string, body: string) {
	return mutate(
		() => invoke<Space>('edit_note', { id, body }),
		(value) => value,
		{ scope: 'editor', repullStatus: () => true },
	)
}

/**
 * The shape every list-scope mutation shares: invoke, take the returned `Space`
 * as the new document, and report a failure on the surface that produced it.
 * The eight wrappers below were this same five-line body with one command name
 * and one argument object changed.
 *
 * `setActiveSection` stays out of it deliberately — it re-pulls status, because
 * it takes no undo snapshot of its own and `canUndo` cannot be assumed.
 */
function listCommand(command: string, args: Record<string, unknown>) {
	return mutate(
		() => invoke<Space>(command, args),
		(value) => value,
		{ scope: 'list' },
	)
}

/** There is no singular set-done command; `set_notes_done` takes an array. Phase
 *  5 calls this with a whole selection, with no signature change. */
async function setNotesDone(ids: string[], done: boolean) {
	// Sounded here rather than at the three call sites above it — the checkbox,
	// Space, and the context menu all funnel through this one command, and one
	// gesture over a whole selection is still one toggle.
	const result = await listCommand('set_notes_done', { ids, done })
	if (result) useSounds().noteToggled()
	return result
}

async function setActiveSection(id: string) {
	const result = await mutate(
		() => invoke<Space>('set_active_section', { id }),
		(value) => value,
		{ scope: 'list', repullStatus: () => true },
	)
	// Only a deliberate switch. `activeSection` is computed off the document, so a
	// watcher on it would also fire for an external edit, a reload, an undo and
	// every refresh — none of which is a user action.
	if (result) useSounds().sectionSwitched()
	return result
}

/**
 * The batch mutations, each **one** store command and therefore one undo
 * snapshot. Looping a singular command over a selection would push one snapshot
 * per note and make undoing a five-note operation take five presses.
 */
async function moveNotes(ids: string[], section: string) {
	return listCommand('move_notes', { ids, section })
}

async function mergeNotes(ids: string[]) {
	return listCommand('merge_notes', { ids })
}

async function deleteNotes(ids: string[]) {
	return listCommand('delete_notes', { ids })
}

/** `index` is interpreted against the target list **after** the note has been
 *  removed from it, and clamped by the store. */
async function reorderNote(id: string, section: string, index: number) {
	return listCommand('reorder_note', { id, section, index })
}

/** Appended last and **made active immediately** by the store, which is what
 *  makes the section the `...` menu just created the one a capture lands in. */
async function addSection(name: string) {
	return listCommand('add_section', { name })
}

async function renameSection(id: string, name: string) {
	return listCommand('rename_section', { id, name })
}

/** Deletes the section **and the notes in it** — undo covers both. Refused by
 *  the store for the last remaining section, so a capture target always exists. */
async function deleteSection(id: string) {
	return listCommand('delete_section', { id })
}

async function reorderSection(id: string, index: number) {
	return listCommand('reorder_section', { id, index })
}

/**
 * `undo`/`redo` return `Space | null`, and `null` — an empty stack — is not an
 * error. So they cannot go through `mutate`, whose contract is that a resolved
 * command carries a document.
 *
 * The status re-pull is unconditional (task-003 §8.1a): both commands can clear
 * or refill the stacks in ways no `Space` payload describes, and a `conflict`
 * failure leaves them exactly as they were.
 */
async function restore(command: 'undo' | 'redo'): Promise<'applied' | 'empty' | 'error'> {
	if (actionError.value?.scope === 'list') actionError.value = null
	const issued = { generation, epoch: epoch.value }

	let result: Space | null
	try {
		result = await invoke<Space | null>(command)
	} catch (error) {
		actionError.value = { scope: 'list', message: errorMessage(error) }
		return 'error'
	}

	if (result === null) {
		await pullStatus()
		return 'empty'
	}

	// Routed through the ordinary applied-document path, so task-004's selection
	// reconciliation runs and prunes ids the restored document no longer has.
	// There is deliberately no second pruning mechanism.
	const applied = applyDocument(result, issued, { animate: true })
	await pullStatus()
	if (!applied) void refresh()
	return 'applied'
}

function undo() {
	return restore('undo')
}

function redo() {
	return restore('redo')
}

/**
 * Adopts a document handed over by a command this module did not invoke.
 *
 * `activate_space` returns the authoritative `Space` for the space it switched
 * to, so pulling it again here would be a second read of state we were just
 * given — and a second chance for the two to disagree. The status re-pull is not
 * optional though: `path`, `watching` and both undo flags all belong to the
 * space that was just closed until it happens.
 */
async function adopt(next: Space) {
	const issued = { generation, epoch: epoch.value }
	// Synchronous, so it cannot be superseded between issuing and applying — the
	// guard is kept anyway because `applyDocument` is the only assignment point
	// and giving it an exception is how a second one starts.
	if (!applyDocument(next, issued, { animate: false })) void refresh()
	await pullStatus()
	// The switch rewrote `recents` and `activeSpace`, and this is the copy the
	// panel reads.
	try {
		settings.value = await invoke<Settings>('get_settings')
	} catch (error) {
		console.error('[copper] could not read settings after a space switch', error)
	}
}

/** For the adapters beside this one — `useSpaces` — so a failed space action
 *  renders in the same place a failed list mutation does instead of inventing a
 *  fourth error surface. */
function reportActionError(scope: ActionErrorScope, message: string) {
	actionError.value = { scope, message }
}

/** Scoped, so dismissing the composer's message does not silently drop the
 *  editor's. */
function clearActionError(scope?: ActionErrorScope) {
	if (!scope || actionError.value?.scope === scope) actionError.value = null
}

/** The message for one surface, or null. */
function errorFor(scope: ActionErrorScope) {
	return computed(() => (actionError.value?.scope === scope ? actionError.value.message : null))
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

/** Id lookup for the action layer, which resolves a selection into note objects
 *  on every menu open. A linear `find` per id is quadratic over a selection. */
const notesById = computed(() => new Map((space.value?.notes ?? []).map((note) => [note.id, note])))

function noteById(id: string): Note | null {
	return notesById.value.get(id) ?? null
}

/** The targeted notes as objects, in the order the ids were given. Ids that no
 *  longer exist are dropped rather than yielding holes. */
function notesByIds(ids: readonly string[]): Note[] {
	const lookup = notesById.value
	return ids.flatMap((id) => {
		const note = lookup.get(id)
		return note ? [note] : []
	})
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
		noteById,
		notesByIds,
		applied,
		errorFor,
		initialize,
		dispose,
		load,
		refresh,
		retry,
		adopt,
		submitEntry,
		updateNoteBody,
		setNotesDone,
		setActiveSection,
		moveNotes,
		mergeNotes,
		deleteNotes,
		reorderNote,
		addSection,
		renameSection,
		deleteSection,
		reorderSection,
		undo,
		redo,
		clearActionError,
		reportActionError,
	}
}
