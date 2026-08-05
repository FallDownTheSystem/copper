/**
 * Note bodies are arbitrary pasted content, so this module is a security
 * boundary before it is a rendering one.
 *
 * Three rules carry that weight, and all three are enforced at *render* time
 * rather than at click time — a click handler covers neither middle-click, nor
 * the anchor context menu, nor drag-and-drop, nor native navigation:
 *
 * - `html: false`, so pasted markup can never reach the WebView.
 * - Any link whose scheme is not `http:`, `https:` or `mailto:` is emitted with
 *   **no `href` attribute at all**, which leaves nothing for any of those paths
 *   to act on. The check reads the token's raw attribute, never an anchor's
 *   `.href` property — the property resolves a relative URL against the WebView
 *   origin and would make a malicious relative path look same-origin.
 * - Markdown images become hyperlinks to the image rather than `<img>` tags, so
 *   a note body cannot issue an outbound request (and a read receipt) in an app
 *   whose published promise is "No Tracking".
 *
 * markdown-it's own `validateLink` already refuses `javascript:`, `vbscript:`,
 * `file:` and `data:`. It is kept as defence in depth; the rule below is what
 * covers what it permits — relative paths and every custom protocol handler the
 * OS has registered.
 */

import MarkdownItCallable from 'markdown-it'
import type { MarkdownIt } from 'markdown-it'
import { createHighlighterCore } from 'shiki/core'
import type { BundledLanguage } from 'shiki'
import { createOnigurumaEngine } from 'shiki/engine/oniguruma'
import { fromHighlighter } from '@shikijs/markdown-it/core'

import type { Note } from './useSpace'

const SAFE_SCHEMES = /^(?:https?|mailto):/i

/**
 * Bounds the cost of a pathological pasted fence. Shiki's tokenizer is
 * superlinear on adversarial input and the panel has no worker to move it to,
 * so oversized fences take the plain path permanently.
 */
const MAX_HIGHLIGHT_CHARS = 20_000
const MAX_HIGHLIGHT_LINE = 2_000

/**
 * The per-fence caps alone are not a bound on a *note*: fifty fences each one
 * byte under the limit cost fifty times as much as the limit implies. This is
 * the aggregate budget for one `renderNote`, spent as fences are highlighted;
 * once exhausted the rest of that note takes the plain path.
 */
const MAX_HIGHLIGHT_CHARS_PER_NOTE = 60_000
let highlightBudget = 0

/**
 * Validation, not normalisation: the emitted href stays exactly what the author
 * wrote (as markdown-it normalised it), and this only decides whether it is
 * emitted at all.
 *
 * The scheme test alone is not enough. `http:javascript:alert(1)` passes a
 * prefix match and is not a URL at all, so anything that survives the regex is
 * also required to parse — and an `http:`/`https:` URL is required to name a
 * host, because a hostless survivor would still be handed to the OS opener.
 */
function isSafeHref(raw: string): boolean {
	const trimmed = raw.trim()
	if (!SAFE_SCHEMES.test(trimmed)) return false

	let url: URL
	try {
		url = new URL(trimmed)
	} catch {
		return false
	}

	if (url.protocol === 'mailto:') return true
	return url.host.length > 0
}

/**
 * One of Shiki's *special* languages: resolved at runtime without a grammar, but
 * absent from the option types, which enumerate bundled grammars only. Verified
 * against shiki 4.4.2 — `codeToHtml` accepts `text`, `plaintext` and `txt`.
 */
const PLAIN_TEXT = 'text' as BundledLanguage

/** Components re-render off this. Clearing a plain `Map` triggers nothing. */
const revision = ref(0)

const cache = new Map<string, { body: string; html: string }>()

const md: MarkdownIt = new MarkdownItCallable({
	html: false,
	linkify: true,
	typographer: true,
})

function escape(text: string) {
	return md.utils.escapeHtml(text)
}

/**
 * Layout-identical to Shiki's output: same trimmed trailing newline, same
 * `tabindex`. An unmatched plain path makes every note below a code block jump
 * the moment the highlighter loads.
 *
 * `tabindex="-1"`, not Shiki's default `0`: a scrollable `<pre>` is natively
 * tabbable and would be a second Tab stop inside the grid, which is exactly what
 * the one-Tab-stop contract forbids. It stays reachable — and keyboard
 * scrollable — through F2 interaction mode, which promotes it to `0`. Shiki's
 * own output is brought in line by a transformer at install time.
 */
function plainHighlight(code: string, lang: string) {
	const language = lang ? ` data-language="${escape(lang)}"` : ''
	return `<pre class="shiki-plain" tabindex="-1"${language}><code>${escape(
		code.replace(/\n$/, ''),
	)}</code></pre>`
}

function tooLargeToHighlight(code: string) {
	return (
		code.length > MAX_HIGHLIGHT_CHARS ||
		code.split('\n').some((line) => line.length > MAX_HIGHLIGHT_LINE)
	)
}

const { rules } = md.renderer

// User headings become non-heading elements, so a note beginning `# Notes`
// cannot inject an <h1> into the panel's own document outline. The panel owns
// exactly one h1 and one h2 per section.
rules.heading_open = (tokens, index) => `<div class="note-h${tokens[index]?.tag.slice(1) ?? '1'}">`
rules.heading_close = () => '</div>'

// A wide table would otherwise widen the whole document rather than scrolling
// inside its own box — horizontal overflow is a separate failure mode from
// vertical and needs its own containment.
rules.table_open = () => '<div class="table-scroll"><table>'
rules.table_close = () => '</table></div>'

/** Nesting depth of the author's own links, tracked so the image rule below can
 *  tell whether it is already inside one. `md.render` builds a fresh `env` per
 *  call, so nothing leaks between notes. markdown-it types `env` as an open
 *  record, so the shape is asserted at each use rather than in the signature. */
type RenderEnv = { linkDepth?: number }

rules.link_open = (tokens, index, options, rawEnv, self) => {
	const env = rawEnv as RenderEnv
	const token = tokens[index]
	if (!token) return ''

	env.linkDepth = (env.linkDepth ?? 0) + 1

	const hrefIndex = token.attrIndex('href')
	const href = hrefIndex === -1 ? null : String(token.attrs?.[hrefIndex]?.[1] ?? '')

	if (href !== null && !isSafeHref(href)) {
		// Not `href="#"`, not `aria-disabled`: the attribute is removed outright,
		// so the context menu and middle-click have nothing to offer.
		token.attrs?.splice(hrefIndex, 1)
	} else if (href !== null) {
		token.attrSet('rel', 'noreferrer')
	}

	// Rendered anchors are natively tabbable. Left alone they would let Tab fall
	// into the middle of a note body and break the grid's one-Tab-stop contract.
	token.attrSet('tabindex', '-1')
	return self.renderToken(tokens, index, options)
}

rules.link_close = (tokens, index, options, rawEnv, self) => {
	const env = rawEnv as RenderEnv
	env.linkDepth = Math.max(0, (env.linkDepth ?? 0) - 1)
	return self.renderToken(tokens, index, options)
}

// `![alt](url)` renders as a link to the image, never as an <img>. The alt text
// becomes the link text; the file is reachable but never prefetched.
rules.image = (tokens, index, options, rawEnv, self) => {
	const env = rawEnv as RenderEnv
	const token = tokens[index]
	if (!token) return ''

	const src = token.attrGet('src')
	// Rendered, not `token.content`: the raw content of `![**bold**](url)` is the
	// literal source, asterisks and all. Rendered *as text* rather than as HTML,
	// because the alt of an image may itself contain a link and nesting one
	// anchor inside another is invalid markup the browser will take apart.
	const rendered = self.renderInlineAsText(token.children ?? [], options, rawEnv).trim()
	const alt = rendered || String(src ?? '') || 'image'

	// `[![alt](img)](link)` — inside the author's own link, the visible text has
	// to belong to *their* link. Emitting an anchor here would nest one inside
	// the other and the browser would break both apart.
	if (env.linkDepth) return escape(alt)

	// The same allowlist as links, deliberately: an image-derived anchor is
	// handed to the OS opener by exactly the same click path, so a `file:` src
	// would reopen the hole the link rule closes. Local attachments get their own
	// designed treatment in task-011.
	if (src === null || !isSafeHref(String(src))) return escape(alt)
	return `<a href="${escape(String(src))}" rel="noreferrer" tabindex="-1">${escape(alt)}</a>`
}

/** The plain path, installed from the start so it is what runs until — and if —
 *  the highlighter resolves. */
md.options.highlight = plainHighlight

// --- Shiki -------------------------------------------------------------------

type Highlighter = Awaited<ReturnType<typeof createHighlighterCore>>

let highlighterPromise: Promise<Highlighter | null> | null = null

/**
 * Created once, behind a single module-scoped promise. Until it resolves — and
 * permanently, if it rejects — code fences render through the plain path and the
 * panel is fully usable.
 */
function ensureHighlighter(): Promise<Highlighter | null> {
	highlighterPromise ??= createHighlighterCore({
		themes: [import('@shikijs/themes/vitesse-light'), import('@shikijs/themes/vitesse-dark')],
		langs: [
			import('@shikijs/langs/bash'),
			import('@shikijs/langs/css'),
			import('@shikijs/langs/diff'),
			import('@shikijs/langs/html'),
			import('@shikijs/langs/javascript'),
			import('@shikijs/langs/json'),
			import('@shikijs/langs/markdown'),
			import('@shikijs/langs/python'),
			import('@shikijs/langs/rust'),
			import('@shikijs/langs/sql'),
			import('@shikijs/langs/toml'),
			import('@shikijs/langs/typescript'),
			import('@shikijs/langs/vue'),
			import('@shikijs/langs/yaml'),
		],
		engine: createOnigurumaEngine(import('shiki/wasm')),
	})
		.then((highlighter) => {
			installHighlighter(highlighter)
			return highlighter
		})
		.catch((error: unknown) => {
			// Logged once and never retried. The plain renderer stays usable.
			console.error('[copper] syntax highlighting unavailable', error)
			return null
		})

	return highlighterPromise
}

function installHighlighter(highlighter: Highlighter) {
	md.use(
		fromHighlighter(highlighter, {
			// Two themes rather than one: Shiki emits the light colour inline and the
			// dark one as `--shiki-dark`, so switching theme is pure CSS and the
			// render cache stays valid across it.
			themes: { light: 'vitesse-light', dark: 'vitesse-dark' },
			// Required. Without it an unknown language throws rather than degrading,
			// and note bodies name whatever language they like.
			fallbackLanguage: PLAIN_TEXT,
			defaultLanguage: PLAIN_TEXT,
			transformers: [
				{
					// Shiki emits `<pre tabindex="0">`. Inside the grid that is a second
					// Tab stop, which breaks the one-Tab-stop contract; F2 interaction
					// mode promotes it back to 0 when the user asks for it.
					pre(node) {
						node.properties.tabindex = '-1'
					},
				},
			],
		}),
	)

	const highlight = md.options.highlight
	md.options.highlight = (code, lang, attrs) => {
		if (tooLargeToHighlight(code) || code.length > highlightBudget) {
			return plainHighlight(code, lang)
		}
		highlightBudget -= code.length
		try {
			return highlight?.(code, lang, attrs) ?? plainHighlight(code, lang)
		} catch {
			return plainHighlight(code, lang)
		}
	}

	// The cached HTML was produced by the plain path, so it has to go — and the
	// counter is what actually makes components re-read it.
	cache.clear()
	revision.value++
}

// --- rendering ---------------------------------------------------------------

/**
 * Cached on the body *string*, never on `note.updated`. A `git checkout` can
 * restore a historical body and a hand-edited file can change a body without
 * touching its timestamp; either would serve stale HTML from a timestamp-keyed
 * cache.
 */
function renderNote(note: Pick<Note, 'id' | 'body'>): string {
	// Read reactively, so a component's computed re-runs when the highlighter
	// swaps in. This is the whole mechanism — the cache clear alone is invisible.
	void revision.value

	const hit = cache.get(note.id)
	if (hit && hit.body === note.body) return hit.html

	highlightBudget = MAX_HIGHLIGHT_CHARS_PER_NOTE
	const html = md.render(note.body)
	cache.set(note.id, { body: note.body, html })
	return html
}

function pruneCache(liveIds: Iterable<string>) {
	const live = new Set(liveIds)
	for (const id of cache.keys()) {
		if (!live.has(id)) cache.delete(id)
	}
}

function clearCache() {
	cache.clear()
}

export function useMarkdown() {
	return {
		renderNote,
		pruneCache,
		clearCache,
		/** Resolves when the highlighter has installed, or to `null` if it could
		 *  not be created. Tests await it; the app fires and forgets. */
		ensureHighlighter,
		/** The instance itself, for tests that need to spy on `render`. */
		md,
	}
}
