/**
 * The one module that talks to the Rust link-preview surface, and the one place
 * a preview image's object URL is created or revoked.
 *
 * A sibling adapter under the "one adapter per Rust surface" rule that
 * `useAttachments`, `useSystemClipboard` and `useSpaces` already follow: no
 * component invokes `link_preview` or `preview_image` directly.
 *
 * # What this module is *not* allowed to be
 *
 * It is not the consent gate. `settings.linkPreviews` is read below, and cards
 * disappear the moment it is switched off — but that is a **rendering**
 * decision. Rust reads the same key out of the store before every fetch and
 * answers with nothing when it is false, so nothing here, and no bug here, can
 * cause a request. A gate that lived only in the WebView would be one stale
 * `settings.value` away from a disclosure that cannot be taken back.
 *
 * # Why the state is module-scoped
 *
 * The same reason `useAttachments`' is: refs declared inside the exported
 * function hand every caller a private copy, and the same URL appears in several
 * notes. Keying on the URL rather than on a note is what makes one link in ten
 * notes one fetch — which is the whole point, since each fetch is another
 * disclosure.
 */

import { invoke } from '@tauri-apps/api/core'

import { useSettings } from './useSettings'

/** Mirrors the Rust `LinkPreview` exactly. `image` is a **cache filename** for
 *  `preview_image`, never a remote URL — the WebView is never told where the
 *  picture came from and never asks the third party for it. */
export type LinkPreview = {
	url: string
	siteName: string | null
	title: string | null
	description: string | null
	image: string | null
}

/** What one link turned out to be.
 *
 *  `none` is the overwhelmingly common outcome and is a first-class state, not a
 *  failure: a page with no metadata, an unreachable host, a timeout and a
 *  refused content type all land here, and AC-6 requires every one of them to
 *  render as the plain link the note already had. */
export type PreviewState =
	| { state: 'loading' }
	| { state: 'none' }
	| { state: 'ready'; preview: LinkPreview; imageUrl: string | null }

/**
 * How many previews may be in flight at once.
 *
 * Lower than `useAttachments`' four, and the reason is different in kind. That
 * bound exists to stop the backend being asked for two thousand image decodes;
 * this one governs **requests to other people's servers**, made because a note
 * carrying a link is mounted in a panel that is on screen. A note pasted from a
 * link dump can carry thirty URLs, and showing the panel should not look like a
 * crawler to thirty hosts at once.
 *
 * Nothing here is per-row: every mounted note asks for its links, whether or not
 * it is scrolled into view. Narrowing that to the rows actually on screen is an
 * open lever recorded in task-020 rather than something this bound provides.
 */
const MAX_CONCURRENT_PREVIEWS = 3

// --- module-scope state ------------------------------------------------------

/** Keyed on the normalised href — see {@link previewKey}. */
const previews = ref(new Map<string, PreviewState>())
/** The URLs this module created, so revoking is exhaustive rather than a walk
 *  over whatever the cache happens to hold at the time. */
const objectUrls = new Set<string>()
/** Links a preview has already been asked for, so ten notes sharing one URL
 *  issue one command rather than ten. */
const requested = new Set<string>()

const waiting: string[] = []
let running = 0

/**
 * Whether the panel is on screen, and therefore whether a request may go out.
 *
 * **A fetch is a disclosure, and it may not happen in a window nobody is
 * looking at.** The panel window is mounted hidden at launch and stays that way
 * until the user summons it, so without this a cold start with previews on
 * contacted every host named anywhere in the space before anything was ever
 * shown — the one moment the user could not possibly have asked for it.
 *
 * The gate holds requests rather than dropping them: `requestPreview` still
 * records the link and queues it, and {@link setPanelVisible} drains the queue
 * when the panel appears. Dropping instead would need every note to re-ask, and
 * the only signal that would reach one is a re-render nothing guarantees.
 *
 * Starts false and is answered by `NoteList`, which owns the two signals the
 * reveal machinery already trusts for the same question — the scroll region
 * gaining a height, and `visibilitychange`.
 */
let panelVisible = false

/**
 * Bumped when previews are switched off, captured by every request in flight.
 *
 * It does the job `useAttachments`' epoch does on a space switch: a response
 * that lands after the toggle went off describes a fetch the user has since
 * withdrawn consent for, and publishing it would put a card back on screen from
 * a request they have since taken back. Dropping it is the honest reading.
 *
 * **It guards this module's state and nothing else.** The fetch is already gone
 * by the time a response can be dropped here, and the on-disk cache entry it
 * wrote was written in Rust before this side saw anything — stopping the *fetch*
 * and the cache write on a mid-flight withdrawal is `previews::preview`'s job,
 * which re-reads consent between its legs for exactly that reason.
 */
const generation = ref(0)

/** One frozen object rather than a fresh literal per read: `previewFor` is
 *  called from a card's computed, and a new identity on every evaluation is a
 *  new value for everything downstream of it. */
const LOADING: PreviewState = Object.freeze({ state: 'loading' })
const NONE: PreviewState = Object.freeze({ state: 'none' })

const { linkPreviews } = useSettings()

/**
 * Turning previews off releases every downloaded picture and forgets what was
 * asked for.
 *
 * The **cache on disk is deliberately left alone** — Rust never deletes it on a
 * toggle, because off-then-on would then re-fetch every URL and disclose a
 * second time. What is dropped here is only this session's blobs and its
 * memory of having asked, so switching back on re-reads from that cache rather
 * than from the network.
 */
watch(linkPreviews, (on) => {
	if (!on) reset()
})

function reset() {
	// Before anything else: it is what makes an in-flight response drop itself
	// rather than publish into the state this is about to clear.
	generation.value++
	waiting.length = 0
	for (const url of objectUrls) URL.revokeObjectURL(url)
	objectUrls.clear()
	requested.clear()
	previews.value = new Map()
}

/**
 * Written in place rather than through a replacement map, exactly as
 * `useAttachments` does: `ref(new Map())` is deeply reactive, so `set` tracks
 * per key and only the cards reading *this* URL re-render. Rebuilding the map
 * would subscribe every card to every key through the iteration, and one
 * preview landing would re-render all of them.
 */
function setPreview(url: string, state: PreviewState) {
	previews.value.set(url, state)
}

/**
 * Asks Rust for one link's card, then for its picture.
 *
 * Two round trips rather than one because the picture is bytes and the metadata
 * is JSON, and the card is worth showing before the image arrives — which is
 * also why `LinkPreviewCard` reserves the image box at a constant size rather
 * than growing into it.
 *
 * Nothing here is an error path. `link_preview` answers `null` for every kind of
 * failure by design, so the only `catch` is for the boundary itself giving way.
 */
async function loadPreview(url: string) {
	const issued = generation.value
	try {
		const preview = await invoke<LinkPreview | null>('link_preview', { url })
		if (issued !== generation.value) return
		if (!preview) {
			setPreview(url, NONE)
			return
		}
		setPreview(url, { state: 'ready', preview, imageUrl: null })
		if (preview.image) await loadImage(url, preview)
	} catch {
		// A preview is an adornment. There is no surface for a message about one and
		// there should not be — see AC-6.
		if (issued === generation.value) setPreview(url, NONE)
	}
}

async function loadImage(url: string, preview: LinkPreview) {
	const issued = generation.value
	if (!preview.image) return
	try {
		const bytes = await invoke<ArrayBuffer>('preview_image', { file: preview.image })
		if (issued !== generation.value || bytes.byteLength === 0) return
		// Always PNG: Rust re-encodes through the attachment thumbnail path, whatever
		// the source was, so there is nothing to sniff.
		const imageUrl = URL.createObjectURL(new Blob([bytes], { type: 'image/png' }))
		objectUrls.add(imageUrl)
		setPreview(url, { state: 'ready', preview, imageUrl })
	} catch {
		// The card keeps its reserved box and shows no picture, which is what a page
		// with no `og:image` looks like anyway.
	}
}

/**
 * Queues a request, at most `MAX_CONCURRENT_PREVIEWS` at a time.
 *
 * The queue is drained rather than scheduled: each finishing request starts the
 * next, so the in-flight count is exactly the number of hosts being contacted.
 *
 * **Called from a watcher, never from a read** — it writes the cache, and cards
 * reach it through a computed, so asking as a side effect of reading would mean
 * writing reactive state during a computed's evaluation. `NoteBody` watches its
 * own link list instead.
 */
function requestPreview(href: string) {
	// The rendering gate. Rust refuses independently; this only avoids a round
	// trip that is guaranteed to come back null.
	if (!linkPreviews.value) return
	const url = previewKey(href)
	if (url === '' || requested.has(url)) return
	requested.add(url)
	setPreview(url, LOADING)
	waiting.push(url)
	pump()
}

/**
 * The panel appeared, or went away.
 *
 * Called with the answer rather than asking for it, because the two signals that
 * carry it belong to the list: the scroll region gaining a height is what
 * `NoteList` already observes for a pending reveal, and it is the one signal
 * WebView2 is known to produce for a window that was mounted hidden.
 */
function setPanelVisible(visible: boolean) {
	if (panelVisible === visible) return
	panelVisible = visible
	if (visible) pump()
}

function pump() {
	// Held, not dropped: the queue is drained the moment the panel is shown.
	if (!panelVisible) return
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

/**
 * The identity two links share when they name one page: the href with its
 * fragment dropped.
 *
 * **This is Rust's cache key, restated on this side so the two agree.**
 * `previews::cache_key` drops the fragment and keeps the query, so
 * `…/page` and `…/page#section` are one cache entry there — while a dedup on the
 * raw href made them two requests here, and two requests racing to write the same
 * entry. The query is kept for the reason Rust keeps it: `?v=`, `?id=` and `?p=`
 * routinely *are* the address.
 *
 * A string cut rather than `new URL`, which throws on anything not absolute and
 * would have to be guarded per call; `#` cannot appear in an href except as the
 * fragment delimiter, since a literal one is percent-encoded by the time
 * markdown-it emits it.
 */
function previewKey(href: string): string {
	const trimmed = href.trim()
	const hash = trimmed.indexOf('#')
	return hash === -1 ? trimmed : trimmed.slice(0, hash)
}

/** The state for `href`, or `loading` while there is none. A pure read — see
 *  {@link requestPreview} for the half that asks. */
function previewFor(href: string): PreviewState {
	if (!linkPreviews.value) return NONE
	return previews.value.get(previewKey(href)) ?? LOADING
}

export function usePreviews() {
	return {
		/** Whether cards may be shown at all. Rendering only — Rust decides whether
		 *  a fetch may happen, and does so from `settings.json` rather than from
		 *  anything the WebView says. */
		enabled: linkPreviews,
		previewFor,
		requestPreview,
		setPanelVisible,
		/** Watch it alongside the links: bumping it is what tells a card whose
		 *  preview was just dropped to stop showing one. */
		previewEpoch: readonly(generation),
	}
}
