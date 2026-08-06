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

import { errorMessage } from '@/lib/rustError'

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

const pendingCount = computed(() => pending.value.length)
const hasPending = computed(() => pending.value.length > 0)

/** The composer's `Attached 1 file` / `Attached N files` chip. */
const pendingLabel = computed(() =>
	pending.value.length === 1 ? 'Attached 1 file' : `Attached ${pending.value.length} files`,
)

// --- previews ----------------------------------------------------------------

function setPreview(file: string, preview: Preview) {
	const next = new Map(previews.value)
	next.set(file, preview)
	previews.value = next
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
	if (requested.has(file)) return
	requested.add(file)
	setPreview(file, { state: 'loading' })

	try {
		const bytes = await invoke<ArrayBuffer>('attachment_thumb', { file })
		if (bytes.byteLength === 0) {
			setPreview(file, { state: 'ready', url: null })
			return
		}
		const url = URL.createObjectURL(new Blob([bytes], { type: 'image/png' }))
		objectUrls.add(url)
		setPreview(file, { state: 'ready', url })
	} catch (error) {
		setPreview(file, { state: 'missing', reason: errorMessage(error) })
	}
}

/**
 * The preview for `file`, requesting it on first ask.
 *
 * Reading and requesting are one call deliberately: a card that had to remember
 * to call a separate `load` in `onMounted` is a card that renders a permanent
 * spinner the day someone forgets.
 */
function previewFor(file: string): Preview {
	const known = previews.value.get(file)
	if (!known) void loadPreview(file)
	return known ?? { state: 'loading' }
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
	for (const url of objectUrls) URL.revokeObjectURL(url)
	objectUrls.clear()
	requested.clear()
	previews.value = new Map()
}

// --- the pending tray --------------------------------------------------------

/** Refused with a message rather than silently truncated, so the user knows
 *  which files did not make it. */
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
		clearPreviews,
		pasteAttachment,
		pickAttachments,
		attachPaths,
		removePending,
		clearPending,
		openAttachment,
	}
}
