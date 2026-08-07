/**
 * The one module that talks to the Rust attachment surface, and the one place a
 * thumbnail object URL is created or revoked.
 *
 * A sibling adapter under the "one adapter per Rust surface" rule that
 * `useSystemClipboard` and `useSpaces` already follow: no component invokes
 * `attach_*` or `attachment_*` directly, so each command string exists once.
 *
 * Two pieces of state, both module-scope for the usual reason — refs declared
 * inside the exported function hand every caller a private copy, and the
 * composer, the tray and every note card have to be looking at the same ones.
 *
 * **The pending tray.** Attachments are ingested at paste, drop and pick time
 * rather than at submit, so the tray can show real metadata immediately and
 * submit stays a metadata-only document write. The cost is that abandoning a
 * draft leaves blobs nothing references, which is exactly what the Rust sweep's
 * 24-hour grace window exists to collect.
 *
 * **The thumbnail cache**, keyed on the content hash. Unlike task-004's Markdown
 * cache there is no staleness question at all: the key *is* the content, so an
 * entry can never describe different bytes than the ones it was built from. It
 * is still cleared on an epoch change, because the blobs of the previous space
 * are not in the new space's assets directory and the URLs have to be revoked
 * rather than merely forgotten.
 */

import { invoke } from '@tauri-apps/api/core'

import { imageMime } from '@/lib/imageMime'
import { errorMessage } from '@/lib/rustError'

import { useSounds } from './useSounds'

/** Mirrors the Rust `Attachment` exactly. `file` is a bare filename inside the
 *  space's assets directory — never a path, and never rendered as one. */
export type Attachment = {
	id: string
	file: string
	name: string
	mime: string
	bytes: number
	width?: number | null
	height?: number | null
}

/** What one attachment's preview turned out to be.
 *
 *  `ready` with a null `url` is the honest description of a `.pdf`: the file is
 *  there, it simply has no picture. That is a different state from `missing`,
 *  and collapsing the two would render every non-image in the unavailable
 *  treatment. */
export type Preview =
	| { state: 'loading' }
	| { state: 'ready'; url: string | null }
	| { state: 'missing'; reason: string }

/** Rust's `ATTACHMENT_MAX_PER_NOTE`. Duplicated here so the tray can refuse a
 *  drop before ingesting bytes it will not be allowed to use; the store enforces
 *  it again on write, which is the authoritative check. */
export const MAX_PER_NOTE = 10

// --- module-scope state ------------------------------------------------------

/** Waiting to be committed with the next composer submission. */
const pending = ref<Attachment[]>([])
/** Keyed on `file`, which is the content hash plus its sniffed extension. */
const previews = ref(new Map<string, Preview>())
/** The URLs this module created, so revoking is exhaustive rather than a walk
 *  over whatever the cache happens to hold at the time. */
const objectUrls = new Set<string>()
/** Files a preview has already been requested for, so N cards mounting against
 *  one attachment issue one command rather than N. */
const requested = new Set<string>()

/**
 * How many previews may be decoding at once.
 *
 * Each one is a full image decode in Rust, and the panel can hold two hundred
 * notes carrying ten attachments each — so the ceiling that matters is not the
 * cost of one decode but how many can be in flight together. Without a bound,
 * scrolling a large space fires every request in one frame and asks the backend
 * for two thousand simultaneous decodes.
 *
 * Four rather than one: previews are what the user is waiting to see, and
 * serialising them entirely would make a screen of attachments fill in visibly
 * one at a time.
 */
const MAX_CONCURRENT_PREVIEWS = 4

const waiting: string[] = []
let running = 0

/**
 * Bumped by `clearPreviews`, captured by every request in flight, and **watched
 * by every mounted card**. It does two jobs, and the second one is easy to
 * lose.
 *
 * *Discarding.* A space switch revokes the cache while requests against the
 * *previous* space are still outstanding. Without this token a response landing
 * afterwards writes into the new epoch's cache — and because the old space's
 * blob is genuinely absent from the new space's assets directory, what it
 * writes is `missing`. The result was a present attachment rendered permanently
 * unavailable until the next switch.
 *
 * *Re-asking.* Revoking the cache leaves every mounted card with no preview and
 * nothing outstanding, so something has to ask again. Back when the request was
 * a side effect of reading, the next render did that on its own; now that the
 * request comes from a watcher, this is the reactive input that makes a card
 * notice. A `ref` rather than a plain counter for exactly that reason — and
 * read as `.value` inside `loadPreview`, which is an async function and not an
 * effect, so no dependency is tracked there.
 */
const generation = ref(0)

/** One frozen object rather than a fresh literal per read: `previewFor` is
 *  called from a card's computed, and a new identity on every evaluation is a
 *  new value for everything downstream of it. Frozen because it is handed to
 *  every card at once — a mutation would be seen by all of them. */
const LOADING: Preview = Object.freeze({ state: 'loading' })

const pendingCount = computed(() => pending.value.length)
const hasPending = computed(() => pending.value.length > 0)

/** The composer's `Attached 1 file` / `Attached N files` chip. */
const pendingLabel = computed(() =>
	pending.value.length === 1 ? 'Attached 1 file' : `Attached ${pending.value.length} files`,
)

// --- previews ----------------------------------------------------------------

/**
 * Written in place rather than through a replacement map.
 *
 * `ref(new Map())` is deeply reactive, so `set` tracks per key and only the
 * cards reading *this* hash re-render. Rebuilding the map instead copied the
 * whole cache on every arrival and — because this is reached from inside a
 * card's own computed — subscribed every card to every key through the
 * iteration, so one thumbnail landing re-rendered all of them.
 */
function setPreview(file: string, preview: Preview) {
	previews.value.set(file, preview)
}

/**
 * Requests a preview once per content hash.
 *
 * An error means one thing and one thing only: this attachment is unavailable.
 * Rust answers "the file is there but has nothing to show" with an empty
 * response instead, so a `.pdf` proves its blob exists through the same call an
 * image proves it with — one round trip per attachment, not two.
 */
async function loadPreview(file: string) {
	const issued = generation.value
	try {
		const bytes = await invoke<ArrayBuffer>('attachment_thumb', { file })
		// The cache this response was issued against has been revoked, so the
		// answer describes a space nobody is looking at. Dropped rather than
		// applied late — applying it is how a present attachment ends up marked
		// unavailable in the space that replaced it.
		if (issued !== generation.value) return
		if (bytes.byteLength === 0) {
			setPreview(file, { state: 'ready', url: null })
			return
		}
		const url = URL.createObjectURL(new Blob([bytes], { type: 'image/png' }))
		objectUrls.add(url)
		setPreview(file, { state: 'ready', url })
	} catch (error) {
		if (issued !== generation.value) return
		setPreview(file, { state: 'missing', reason: errorMessage(error) })
	}
}

/**
 * Queues a preview request, at most `MAX_CONCURRENT_PREVIEWS` at a time.
 *
 * The queue is drained rather than scheduled: each finishing request starts the
 * next, so the in-flight count is exactly the number of decodes the backend is
 * being asked for.
 *
 * **Called from a watcher, never from a read.** It writes the cache, and the
 * card that wants a preview reaches it through a computed — so asking for one
 * as a side effect of reading it would mean writing reactive state during a
 * computed's evaluation. `AttachmentCard` watches its own `file` instead, which
 * keeps the pair as inseparable as the old single call did without the write.
 */
function requestPreview(file: string) {
	if (requested.has(file)) return
	requested.add(file)
	setPreview(file, LOADING)
	waiting.push(file)
	pump()
}

function pump() {
	while (running < MAX_CONCURRENT_PREVIEWS && waiting.length > 0) {
		const next = waiting.shift()
		if (next === undefined) return
		running++
		void loadPreview(next).finally(() => {
			running--
			pump()
		})
	}
}

/** The preview for `file`, or `loading` while there is none. A pure read — see
 *  [`requestPreview`] for the half that asks. */
function previewFor(file: string): Preview {
	return previews.value.get(file) ?? LOADING
}

/**
 * Drops every cached preview and revokes its URL.
 *
 * Called on an epoch change. Not on unmount of a card: several cards can point
 * at one hash — the same screenshot on two notes — so revoking when one goes
 * away would blank the others. The set is bounded by the number of distinct
 * attachments in one space, and a space switch is what clears it.
 */
function clearPreviews() {
	// Before anything else: it is what makes an in-flight response drop itself
	// rather than publish into the cache this is about to replace.
	generation.value++
	// Queued-but-not-started requests are simply dropped — they name blobs in a
	// space that is no longer open.
	waiting.length = 0
	for (const url of objectUrls) URL.revokeObjectURL(url)
	objectUrls.clear()
	requested.clear()
	previews.value = new Map()
}

// --- the full-size read ------------------------------------------------------

/**
 * The whole image, for the in-panel viewer.
 *
 * **Deliberately not through `pump`.** That queue exists to stop two hundred
 * cards asking for two thousand decodes at once; this is one request the user
 * made by opening one image, and putting it behind four thumbnail decodes would
 * make the viewer feel broken for the sake of a bound it does not stress.
 *
 * **Not cached either.** A ten-megabyte blob per image the user has ever glanced
 * at is not a cache, it is a leak with a lookup table; the URL is revoked when
 * the viewer closes. It still joins `objectUrls`, so a space switch mid-view
 * revokes it along with everything else rather than leaving one URL alive in an
 * epoch nobody is looking at.
 *
 * The epoch guard is the thumbnail path's, for the same reason: a response
 * landing after a switch describes a space that is no longer open.
 */
async function loadFullImage(file: string): Promise<{ url: string } | { reason: string }> {
	const issued = generation.value
	try {
		const bytes = await invoke<ArrayBuffer>('attachment_full', { file })
		if (issued !== generation.value) return { reason: 'The space changed while loading.' }

		const type = imageMime(bytes)
		if (!type) {
			// Rust gates on the sniffed type before sending anything, so this means the
			// two sniffs disagree rather than that the user attached something odd.
			return { reason: 'Copper could not tell what kind of image this is.' }
		}

		const url = URL.createObjectURL(new Blob([bytes], { type }))
		objectUrls.add(url)
		return { url }
	} catch (error) {
		return { reason: errorMessage(error) }
	}
}

/** Drops one full-size URL. Separate from `clearPreviews` because the viewer
 *  closing is not an epoch change: every thumbnail on screen stays valid. */
function revokeFullImage(url: string) {
	if (!objectUrls.delete(url)) return
	URL.revokeObjectURL(url)
}

// --- the pending tray --------------------------------------------------------

/** How many more files the tray will take. Whatever does not fit is truncated
 *  *and* reported by `accept`, never dropped silently. */
function room(): number {
	return MAX_PER_NOTE - pending.value.length
}

/**
 * Adds what a command returned, up to the per-note cap.
 *
 * Returns the message to show, or null when everything landed. The caller
 * decides where it goes; this module has no opinion about error surfaces.
 */
function accept(added: Attachment[]): string | null {
	if (added.length === 0) return null

	const available = room()
	if (available <= 0) {
		return `A note can carry ${MAX_PER_NOTE} attachments. Remove one first.`
	}

	pending.value = [...pending.value, ...added.slice(0, available)]
	// Below both early returns, so a no-op paste and a refusal at capacity stay
	// silent.
	useSounds().attachmentsAdded()
	return added.length > available
		? `Only ${available} more file${available === 1 ? '' : 's'} fit on one note; the rest were not attached.`
		: null
}

/** Runs an ingest command and folds its result into the tray. A failure comes
 *  back as its message; nothing is added and nothing already there is lost. */
async function ingest(command: string, args?: Record<string, unknown>): Promise<string | null> {
	try {
		return accept(await invoke<Attachment[]>(command, args))
	} catch (error) {
		return errorMessage(error)
	}
}

/**
 * `Ctrl+V` in the composer.
 *
 * Returns `false` when the clipboard held nothing attachable, which is the
 * signal to let the native text paste happen. Text always wins and that
 * decision is Rust's — asking here would need a second clipboard read and a
 * second copy of the rule.
 */
async function pasteAttachment(): Promise<{ handled: boolean; message: string | null }> {
	let added: Attachment[]
	try {
		added = await invoke<Attachment[]>('attach_paste')
	} catch (error) {
		// A refusal — an oversized image, an unreadable file — is still a paste that
		// was handled: falling through to the native text paste would insert
		// whatever unrelated text was on the clipboard.
		return { handled: true, message: errorMessage(error) }
	}
	if (added.length === 0) return { handled: false, message: null }
	return { handled: true, message: accept(added) }
}

function pickAttachments() {
	return ingest('attach_pick')
}

function attachPaths(paths: string[]) {
	return ingest('attach_paths', { paths })
}

function removePending(id: string) {
	pending.value = pending.value.filter((attachment) => attachment.id !== id)
}

/**
 * Emptied only after a submission the store accepted.
 *
 * Task-004's rule that nothing is cleared optimistically applies here for a
 * sharper reason than it does to the composer's text: a cleared tray is the
 * only remaining reference to blobs the sweep will eventually collect, so
 * clearing it on a failure is the one action in this module that can lose a
 * file for good.
 */
function clearPending() {
	pending.value = []
}

// --- opening -----------------------------------------------------------------

/**
 * Images open in the OS viewer; everything else is revealed in Explorer. Which
 * of the two happens is decided in Rust from the bytes on disk, so nothing here
 * branches on `mime` — that field is hand-editable and would be the wrong thing
 * to trust with a shell call.
 */
async function openAttachment(file: string): Promise<string | null> {
	try {
		await invoke('attachment_open', { file })
		return null
	} catch (error) {
		return errorMessage(error)
	}
}

// --- formatting --------------------------------------------------------------

/** Sizes in the units a person reading a file list expects. */
export function formatBytes(bytes: number): string {
	if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
	if (bytes >= 1024) return `${Math.ceil(bytes / 1024)} KB`
	return `${bytes} bytes`
}

export function useAttachments() {
	return {
		pending: readonly(pending),
		pendingCount,
		hasPending,
		pendingLabel,
		previewFor,
		requestPreview,
		/** Watch it alongside the file: bumping it is what tells a card whose
		 *  preview was just revoked to ask again. */
		previewEpoch: readonly(generation),
		clearPreviews,
		loadFullImage,
		revokeFullImage,
		pasteAttachment,
		pickAttachments,
		attachPaths,
		removePending,
		clearPending,
		openAttachment,
	}
}
