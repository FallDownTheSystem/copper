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
import { listen } from '@tauri-apps/api/event'
import type { DeepReadonly } from 'vue'

import { parseCreated } from '@/lib/noteTime'
import { errorMessage } from '@/lib/rustError'
import { createStartup } from '@/lib/startup'

import { useAttachments, type Attachment } from './useAttachments'
import { emptySnapshot, noteRow, revealRow, sectionRow, useSelection } from './useSelection'
import { useMarkdown } from './useMarkdown'
import { useNoteDisclosure } from './useNoteDisclosure'
import { useNoteEditor } from './useNoteEditor'
import { useEditorHandoff } from './useEditorHandoff'
import { useNoteList } from './useNoteList'
import { useNoteSearch } from './useNoteSearch'
import { useSectionEditor } from './useSectionEditor'
import { useSections } from './useSections'
import { useSounds } from './useSounds'
import { useStatusMessage } from './useStatusMessage'

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
	/** `'top' | 'bottom'` and `'copy' | 'edit'` — narrowed in `useSettings` for
	 *  the same reason `theme` and `motion` are. */
	insertionPoint: string
	doubleClick: string
	/** Whether the panel window sits in the topmost band. A genuine boolean,
	 *  unlike the four above — there is nothing for a name to say here. */
	alwaysOnTop: boolean
	/** Whether a note's card shows the `created` the store has recorded since
	 *  task-003. A display preference only — nothing about it changes what is
	 *  written. */
	showCreated: boolean
	/** Whether a capture that lands while the panel is hidden fires a Windows
	 *  notification. Read and acted on entirely in Rust — the panel only renders
	 *  the switch. */
	captureNotifications: boolean
	/** Whether a link in a note may be fetched to build a preview card. The one
	 *  setting whose "on" position makes Copper send anything to a third party,
	 *  and the only consent surface for it — Rust reads this key store-side before
	 *  every fetch rather than trusting the caller. */
	linkPreviews: boolean
	/** Whether the panel wears Acrylic and thins `--surface` so it blurs through.
	 *  The material half lives in Rust; the class half in `useTheme`. */
	translucent: boolean
	/** Palette family names — narrowed in `lib/palette` for the same reason
	 *  `theme` and `motion` are narrowed in `useSettings`. */
	neutral: string
	accent: string
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

export type ChangeReason = 'external' | 'capture' | 'reload' | 'editor' | 'reroute'
export type SpaceChangedPayload = { id: string; path: string; reason: ChangeReason }
export type StoreErrorPayload = { kind: string; message: string }
/** What a composer submission turned out to be. `# Name` is classified in Rust,
 *  above the store, so the composer path and the capture path cannot drift.
 *
 *  The panel *composes* through `submit_entry`, which is the only entry point
 *  that reads a body as anything but opaque text. `add_note` below is the other
 *  one: the capture path reaches it from Rust and task-013's zero-focus paste
 *  reaches it from here, because a pasted `# Heading` is a note rather than a
 *  section directive. */
export type SubmitOutcome = 'note' | 'section-created' | 'section-activated'
export type AddNoteResult = { space: Space; noteId: string }
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

const selection = useSelection()
const markdown = useMarkdown()
const disclosure = useNoteDisclosure()
const editor = useNoteEditor()
const search = useNoteSearch()
const sectionEditor = useSectionEditor()
/** Named for the state rather than the module: `sections` below is the
 *  document's own list, and the two are different things. */
const sectionState = useSections()
/** Named for the state for the same reason `sectionState` is: this is the done
 *  filter and the per-section sort, not the list component. */
const listState = useNoteList()
const handoff = useEditorHandoff()
const attachmentState = useAttachments()
const status = useStatusMessage()

/** Re-exported for the adapters beside this one, which have always taken it from
 *  here. It moved to `lib/` when `useAttachments` needed it too — importing it
 *  from an adapter this module imports would close a cycle. */
export { errorMessage }

/**
 * The scroll the next document owes a just-added note, and which note it is.
 *
 * Armed by the three paths that mean *the user just put a note here* — a composer
 * submit, a zero-focus paste, and a global capture — and consumed by the document
 * that carries the note in.
 *
 * **The request names its note wherever the path can name one**, because a bare
 * "scroll to whatever arrives next" outlives the mutation that asked for it: a
 * command that throws never produces a document, so the arming would be spent on
 * whatever landed afterwards — an external edit or a reload, the two things the
 * paragraph below promises never move the list. `submit_entry` and `add_note` both
 * answer with the id they created, and `mutate` arms this only once that answer is
 * in hand, so a failed command arms nothing at all and a document without that id
 * cannot consume the request.
 *
 * A capture can name nothing: it arrives as an event, and the document is fetched
 * by the shared `refresh()`, which knows nothing about why it was called and
 * coalesces several reasons into one pull. That is what `newest` is for, and the
 * diff below resolves it against the document that actually lands.
 *
 * **Deliberately not armed by an external change or a reload.** Someone editing
 * the `.copper` file in another program adds notes the reader did not ask for, and
 * the anchoring in `useSelection` exists precisely so a document arriving under
 * them does not move the list. Undo is left out for the same reason: a restored
 * note is the reader taking something back, and the row they are looking at is
 * already the one that matters.
 */
type AddedNoteReveal = { kind: 'note'; id: string } | { kind: 'newest' }
let revealAddedNote: AddedNoteReveal | null = null

/**
 * The note in `next` that `previous` did not have, newest first when there are
 * several.
 *
 * Several is reachable: captures queue while the store is busy, and one refresh
 * can carry two of them. The newest is the one to show — "the last note they
 * added" — and reading it off `created` rather than off document order is what
 * makes that true under either insertion point, since a top insertion puts the
 * newest note first and a bottom insertion puts it last.
 */
function addedNoteId(previous: Space | null, next: Space): string | null {
	if (!previous) return null
	const before = new Set(previous.notes.map((note) => note.id))

	let landed: Space['notes'][number] | null = null
	for (const note of next.notes) {
		if (before.has(note.id)) continue
		// A note whose `created` cannot be parsed sorts as the oldest possible rather
		// than winning by accident — the same "unknown is not a claim" rule
		// `sortByCreated` follows.
		if (!landed || (parseCreated(note.created) ?? 0) > (parseCreated(landed.created) ?? 0)) {
			landed = note
		}
	}
	return landed?.id ?? null
}

/** Null when the named note is not in this document: a request only ever fires
 *  for a document that actually carries the note it was made for. */
function landedNoteId(wanted: AddedNoteReveal, previous: Space | null, next: Space): string | null {
	if (wanted.kind === 'newest') return addedNoteId(previous, next)
	return next.notes.some((note) => note.id === wanted.id) ? wanted.id : null
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
	if (generation !== issued.generation || epoch.value !== issued.epoch) {
		// The request belonged to the mutation that issued this document, and this
		// document is not going on screen. Left armed it would wait for the next one
		// instead, which is how a reveal reaches a reader who never asked for it.
		revealAddedNote = null
		return false
	}

	// Must be taken *before* the assignment: afterwards `visibleNoteIds` holds
	// only the new order and the focused note's former index is gone, so the
	// nearest-survivor rule cannot be evaluated at all.
	const taken = selection.snapshot()
	const identityChanged = space.value !== null && space.value.id !== next.id

	// Read before the assignment, like the snapshot above and for the same reason:
	// afterwards there is no previous document to diff against. Cleared whether or
	// not it produced a row — a capture whose note a later refresh had already
	// carried in resolves to nothing, and leaving it set would fire on whatever
	// landed next.
	const wanted = revealAddedNote
	revealAddedNote = null
	const landedNote = wanted && !identityChanged ? landedNoteId(wanted, space.value, next) : null

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
		// The done filter and the per-section sorts go with them, and for the same
		// reason: both are questions asked about the document that has just been
		// replaced. This is the reset AC3 asks for — a space switch is the only
		// gesture inside the panel that changes which sections exist.
		listState.reset()
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
	// Beside the search index and before `syncDocument` for the same reason: the
	// orders that walk is about to build are filtered by the done set and ordered
	// by the created index, so rebuilding afterwards would leave one frame in which
	// the new document is filtered against the previous document's `done`.
	listState.rebuild(next)
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
		// After the scroll restore, and that order is the point: the restore puts the
		// list back where the reader left it, which for a capture that landed off
		// screen is exactly the position they need moving away from. Requested rather
		// than performed — a capture usually arrives at a hidden panel, and the
		// request survives until there is a list to scroll.
		if (landedNote) revealRow(noteRow(landedNote))
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
 * Every reason is live and each has exactly one producer. Only `capture` is
 * branched on here; the rest reach the same re-pull, which is the point of an
 * identity-only payload. `reroute` — a capture notification's button filing the
 * note elsewhere — is deliberately *not* `capture`, because it would otherwise
 * play the capture sound and ask the list to scroll to a note nothing added.
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
	if (payload.reason === 'capture') {
		useSounds().captureSucceeded()
		// The panel is almost never on screen for this one — that is what a global
		// capture is — so the reveal it arms is a request the list flushes whenever
		// it next has somewhere to scroll. `newest` rather than a named note because
		// the payload carries identity only: which note was written is knowable here
		// solely by diffing the document the pull below returns.
		revealAddedNote = { kind: 'newest' }
	}
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

/** `load()` rather than a plain pull, because task-003 §8A.3 makes startup three
 *  calls: the document, the status only `get_status` carries, and the settings
 *  copy the panel reads. */
const { initialize, dispose } = createStartup(
	() =>
		Promise.all([
			listen<SpaceChangedPayload>('space-changed', (event) => void onSpaceChanged(event.payload)),
			listen<StoreErrorPayload>('store-error', (event) => void onStoreError(event.payload)),
		]),
	load,
)

// --- mutations ---------------------------------------------------------------

/**
 * The standing toast belongs to the mutation that set it, and this is a
 * different one.
 *
 * **Every store mutation passes through `mutate` or `restore`, which is why the
 * rule lives here rather than at the callers.** The toast carries an `Undo`
 * button bound to the top of the store's undo stack, and most mutations set no
 * toast at all — a composer submit, a zero-focus paste, a drag, an Alt+Arrow, a
 * `Move to ▸` — so each of them used to leave the *previous* action's pill on
 * screen over a button that now undid theirs. Marking a note done and then
 * composing one left "Moved 1 note to Done · Undo" standing over a press that
 * removed the note just written.
 *
 * The message and the button go together. A pill whose button has gone stale has
 * no honest remainder: "Moved 1 note to Done" is a claim about what the last
 * thing to happen was, and it is no longer true either.
 *
 * Callers that *do* report set their message after their command resolves, so
 * this clears a moment before they write, never over them.
 */
function retireStandingToast() {
	status.clear()
}

/**
 * Nothing is cleared optimistically and nothing is retried automatically: a
 * failure leaves the text in place with a visible message, because a silently
 * lost capture is the failure mode the whole product is designed against.
 */
async function mutate<T>(
	run: () => Promise<T>,
	toSpace: (result: T) => Space,
	options: {
		scope: ActionErrorScope
		// A predicate rather than a flag, because `submit_entry` cannot answer the
		// question until it has run: creating a section is deterministically undoable,
		// activating one takes no snapshot at all, and the outcome says which happened.
		repullStatus?: (result: T) => boolean
		/** The note this command created, or null when it created none. Read here
		 *  rather than armed by the caller before the invoke, because a command that
		 *  throws must leave nothing armed — a request made before the answer is in
		 *  hand is a request some later, unrelated document would spend. */
		revealNote?: (result: T) => string | null
	},
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

	// After the command, not before it: a refused mutation changed nothing, so the
	// pill on screen is still describing the most recent thing that happened.
	retireStandingToast()

	const revealId = options.revealNote?.(result) ?? null
	if (revealId) revealAddedNote = { kind: 'note', id: revealId }

	const applied = applyDocument(toSpace(result), issued, { animate: true })

	// The status is updated on every path, superseded or not: the store carried
	// the mutation out either way, and skipping it left `canUndo` false after a
	// real, undoable change. What differs is *how* it is learned.
	if (options.repullStatus?.(result) || !applied) {
		// `edit_note`, `set_active_section`, and `submit_entry` when it only
		// activated an existing section, take no undo snapshot of their own — but a
		// write that had to be re-applied over an external change clears both stacks
		// and emits nothing, and a re-pull is the only way to learn about it.
		//
		// Supersession takes this path for a sharper reason. The document that
		// overtook this one may have been an *external* reload, and a reload clears
		// both stacks (spec 4.6) — so the optimistic `canUndo: true` below would
		// light up an Undo control with nothing behind it. Asking the store is the
		// only way to tell that case from a merely reordered response.
		await pullStatus()
	} else {
		// Deterministic for an ordinary structural mutation, so no round trip.
		//
		// The token bump makes this write take part in the same discard discipline
		// a pull does. A `get_status` issued before it — an event handler's, say —
		// can still be outstanding, and it was answered by a store that had not yet
		// carried this mutation out; landing afterwards it would put `canUndo` back
		// to false with nothing further coming to correct it.
		statusToken++
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
			// Null on both section outcomes: a submit that turned out to be a section
			// directive added no note, so there is nothing to scroll to.
			revealNote: (value) => value.noteId,
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

/**
 * Task-013's zero-focus paste. Maps to `add_note`, **not** to `submit_entry`.
 *
 * A paste is a capture, not a composition: text lifted out of a document whose
 * whole body happens to be `# Heading` has to become a note, and `submit_entry`
 * would read it as a section directive and silently create a section instead.
 * That split is the same one the global capture relies on, and it lives in Rust
 * so the two paths cannot drift.
 *
 * No `section` argument, for the reason `submitEntry` gives: the store already
 * defaults to `activeSection`, and sending our own view of it would race an
 * external change.
 *
 * Focus is deliberately not moved. The note lands silently in the active
 * section, wherever the user was looking — that is what makes this different
 * from a composer submit, which puts the roving target on what it just created.
 * It is still scrolled to: not moving focus is about not stealing the keyboard,
 * and a note the reader cannot see is not a capture they can trust.
 */
async function addNote(body: string) {
	return mutate(
		() => invoke<AddNoteResult>('add_note', { body, section: null }),
		(value) => value.space,
		{ scope: 'composer', revealNote: (value) => value.noteId },
	)
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
	// Sounded here rather than at the three callers — the checkbox, Space and the
	// context menu all funnel through this one command, and one gesture over a
	// whole selection is still one toggle.
	const result = await listCommand('set_notes_done', { ids, done })
	if (result) useSounds().noteToggled()
	return result
}

/**
 * Queued behind `applyDocument`'s own `nextTick`, which is what makes this win:
 * that callback restores the scroll the document arrived with, and this is a
 * deliberate move away from it.
 *
 * `start` rather than `nearest`, unlike a captured note. Choosing a section is
 * choosing a place to be, so the list lands *at* its heading with the section
 * below it — where `nearest` would scroll a heading just off the bottom edge into
 * the bottom edge and leave the section itself still out of sight.
 *
 * Callers must gate this on the mutation having been **applied**. A superseded
 * response returns before scheduling the restoration tick this queues behind, so
 * both halves of the ordering above are false for it — and the section it names
 * belongs to a document that was discarded.
 */
function revealSectionSoon(id: string) {
	void nextTick(() => revealRow(sectionRow(id), 'start'))
}

async function setActiveSection(id: string) {
	const result = await mutate(
		() => invoke<Space>('set_active_section', { id }),
		(value) => value,
		{ scope: 'list', repullStatus: () => true },
	)
	// Only a deliberate switch. `activeSection` is computed off the document, so a
	// watcher on it would also fire for an external edit, a reload, an undo and
	// every refresh — none of which is a user action. The scroll goes with the
	// sound for exactly that reason: both mark a choice somebody made.
	//
	// `applied` rather than a truthiness test on the result, because a superseded
	// mutation resolves with one: the store did carry the switch out, but the
	// document on screen is a fresher one this side of the boundary has not read
	// yet, and confirming a move to a section it may not even have is worse than
	// staying quiet until the refresh behind it lands.
	if (result?.applied) {
		useSounds().sectionSwitched()
		revealSectionSoon(id)
	}
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
/**
 * The section is scrolled to by *id*, read out of the returned document rather
 * than matched by the name that was asked for: the store normalises whitespace and
 * allows duplicates, so a name is not an identity there.
 *
 * Captured before the command, because by the time it resolves `sections` is
 * already the new document's.
 */
async function addSection(name: string) {
	const before = new Set(sections.value.map((section) => section.id))
	const result = await listCommand('add_section', { name })
	// `applied`, for the reason `setActiveSection` gives: a superseded response
	// names a section out of a document that was discarded, and scrolling to it
	// would jump the list on the strength of a view nobody is looking at.
	const created = result?.applied
		? result.value.sections.find((section) => !before.has(section.id))
		: null
	if (created) revealSectionSoon(created.id)
	return result
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

	// The other half of the rule `mutate` states: a step walked back is a step the
	// pill can no longer offer to walk back. Leaving it up after its own `Undo` was
	// pressed invites a second press, which takes the step *before* the one the
	// pill names.
	retireStandingToast()

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
		addNote,
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
