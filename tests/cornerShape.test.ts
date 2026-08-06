import { readFileSync, readdirSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vite-plus/test'

/**
 * Task-012 AC1, AC2 and AC4, asserted against the **production** artefacts.
 *
 * The hazard `corner-shape` carries here was never the browser — it shipped
 * unflagged in Chromium 139 and WebView2 Evergreen is far past that. It is the
 * build: Tailwind 4 runs this stylesheet through Lightning CSS, and Vite's build
 * target is `chrome105` because Tauri requires it of the *JS*. A minifier
 * configured against a target that predates a property is entitled to drop the
 * declaration, and it would do so silently — in the production build only, with
 * `vp dev` looking perfectly fine.
 *
 * So this reads the built CSS rather than the source. It is why the gate order
 * runs `vp build` before `vp test`.
 */

// The runner's working directory is the project root; `import.meta.url` is not a
// file URL under it.
const root = process.cwd()

const REBUILD =
	'This suite asserts the production CSS pipeline, so run `pnpm exec vp build` before ' +
	'`pnpm exec vp test` — the order the gates use.'

/**
 * The built stylesheet, or a readable failure.
 *
 * **Staleness is checked, not just presence**, and that is the whole point of the
 * mtime comparison: a `dist/` left over from before an edit to `main.css` would
 * let this suite go green while asserting nothing about the code under review,
 * which is a worse failure than not running at all. Comparing against
 * `main.css` alone is enough — it is the only source file that can change what
 * these tests read.
 */
function builtCss() {
	const directory = resolve(root, 'dist/assets')
	let names: string[]
	try {
		names = readdirSync(directory).filter((name) => name.endsWith('.css'))
	} catch {
		throw new Error(`dist/assets is missing. ${REBUILD}`)
	}
	if (names.length === 0) throw new Error(`dist/assets holds no stylesheet. ${REBUILD}`)

	const built = Math.max(...names.map((name) => statSync(resolve(directory, name)).mtimeMs))
	const source = statSync(resolve(root, 'src/assets/main.css')).mtimeMs
	if (built < source) {
		throw new Error(
			`dist/assets is older than src/assets/main.css, so this would be checking a ` +
				`stale build. ${REBUILD}`,
		)
	}

	return names.map((name) => readFileSync(resolve(directory, name), 'utf8')).join('\n')
}

/**
 * The inverse of `withoutSupportsBlocks`: the *contents* of every corner-shape
 * support query, and nothing else.
 *
 * Not `css.replace(withoutSupportsBlocks(css), '')` — that returns a
 * concatenation of fragments rather than one contiguous substring, so it would
 * match nothing and silently hand back the whole stylesheet.
 */
function supportsBlocksOnly(css: string) {
	const prelude = /@supports\s*\(corner-shape:\s*squircle\)\s*\{/g
	let out = ''

	for (let match = prelude.exec(css); match; match = prelude.exec(css)) {
		let depth = 1
		const start = match.index + match[0].length
		let index = start
		while (index < css.length && depth > 0) {
			if (css[index] === '{') depth++
			else if (css[index] === '}') depth--
			index++
		}
		// `index` sits one past the closing brace.
		out += `${css.slice(start, index - 1)}\n`
		prelude.lastIndex = index
	}

	return out
}

/**
 * Cuts out every `@supports (corner-shape: squircle)` block, prelude and all, so
 * that what remains is exactly the CSS that would still apply on a runtime
 * without the property. Brace-matched rather than regexed: these blocks hold
 * whole rules, and a non-greedy `[^}]*` would stop at the first inner `}`.
 */
function withoutSupportsBlocks(css: string) {
	const prelude = /@supports\s*\(corner-shape:\s*squircle\)\s*\{/g
	let out = ''
	let cursor = 0

	for (let match = prelude.exec(css); match; match = prelude.exec(css)) {
		out += css.slice(cursor, match.index)
		let depth = 1
		let index = match.index + match[0].length
		while (index < css.length && depth > 0) {
			if (css[index] === '{') depth++
			else if (css[index] === '}') depth--
			index++
		}
		cursor = index
		prelude.lastIndex = index
	}

	return out + css.slice(cursor)
}

describe('the squircle survives the production pipeline', () => {
	/** AC1. Minified output has no spaces after the colon, so both forms are
	 *  accepted rather than pinning the minifier's formatting. */
	it('emits the corner-shape declaration into the built stylesheet', () => {
		const css = builtCss()

		expect(css).toMatch(/corner-shape:\s*squircle/)
	})

	/**
	 * AC2, expressed as a property of the emitted rule rather than as a rendering
	 * test: the utility carries **only** the corner shape. Every surface's radius
	 * comes from its own `rounded-*`, so a runtime that ignores `corner-shape`
	 * renders exactly what shipped before this task, and deleting the utility is a
	 * complete rollback.
	 */
	it('keeps the utility to the corner shape alone, so the radius is the fallback', () => {
		const css = builtCss()
		const rule = /\.squircle\{([^}]*)\}/.exec(css)

		expect(rule, 'the .squircle utility was not emitted at all').not.toBeNull()
		expect(rule?.[1].trim().replace(/;$/, '')).toMatch(/^corner-shape:\s*squircle$/)
	})

	/** Fallback-first: nothing may assert the property outside a support query
	 *  except the panel root's explicit `round`, which is the initial value and is
	 *  a no-op wherever the property is unknown. */
	it('guards every squircle behind a support query', () => {
		const guarded = withoutSupportsBlocks(builtCss())

		// Whatever is left asserts the property unconditionally, which on a runtime
		// that does not understand it is a declaration the parser discards — the
		// same outcome, but arrived at by luck rather than by the fallback the
		// design calls for.
		expect(guarded).not.toMatch(/corner-shape:\s*squircle/)
	})

	/**
	 * The `@apply squircle` composition, which is the one piece of this work with
	 * no other assertion behind it: `panel-button` gets its corner shape by
	 * applying the utility rather than by carrying the declaration, and Tailwind
	 * composing an `@utility` whose whole body is a support query is not obviously
	 * guaranteed to survive.
	 *
	 * Both halves matter and they live in different rules. The radius must be
	 * **unconditional**, because it is the fallback; the corner shape must be
	 * **guarded**, because it is the enhancement.
	 */
	it('composes the utility into panel-button without losing either half', () => {
		const css = builtCss()

		const plain = /\.panel-button\{([^}]*)\}/.exec(withoutSupportsBlocks(css))
		expect(plain, 'panel-button emitted no unguarded rule at all').not.toBeNull()
		const radius = /border-radius:\s*([^;}]+)/.exec(plain?.[1] ?? '')
		expect(radius, 'panel-button lost its fallback radius').not.toBeNull()
		expect(radius?.[1].trim()).not.toMatch(/^0(px|%|em|rem)?$/)

		expect(supportsBlocksOnly(css)).toMatch(/\.panel-button\{corner-shape:\s*squircle/)
	})

	/**
	 * AC3's structural half. `DWMWCP_ROUND` rounds the *window* on a circular arc
	 * and `--panel-radius` exists to match it; a superellipse here would diverge
	 * from that arc exactly at the corner, which is where task-004's acceptance
	 * criterion 13 looks for a seam. Stated rather than inherited, so a utility
	 * landing on the root by accident is a no-op.
	 */
	it('leaves the panel root explicitly round', () => {
		const css = builtCss()
		const rule = /\.panel-surface\{([^}]*)\}/.exec(css)

		expect(rule, 'the .panel-surface rule was not emitted').not.toBeNull()
		expect(rule?.[1]).toMatch(/corner-shape:\s*round/)
		expect(rule?.[1]).not.toMatch(/corner-shape:\s*squircle/)
	})

	/**
	 * The gap the test above cannot see. It reads the *first* `.panel-surface`
	 * rule, so adding `squircle` to the class in a template would emit a second,
	 * guarded rule that overrides it at every runtime that supports the property —
	 * and all the other assertions here would stay green while the panel root
	 * seamed against the DWM corner, which is the one outcome AC3 forbids.
	 */
	it('never squircles the panel root from inside a support query either', () => {
		expect(supportsBlocksOnly(builtCss())).not.toMatch(/\.panel-surface/)
	})
})

describe('the webview keeps wry defaults', () => {
	/**
	 * AC4. `corner-shape` needs no flag on any runtime this app can run on, and
	 * reaching for one would be actively harmful: Tauri's own documentation warns
	 * that wry passes `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`
	 * by default and that setting `additionalBrowserArgs` means re-supplying them
	 * yourself. Setting it would silently re-enable the WebView2 out-of-process UI
	 * and SmartScreen components wry deliberately disables.
	 */
	it('sets no additionalBrowserArgs anywhere in tauri.conf.json', () => {
		const config = readFileSync(resolve(root, 'src-tauri/tauri.conf.json'), 'utf8')

		expect(config).not.toContain('additionalBrowserArgs')
		expect(config).not.toContain('additional_browser_args')
	})
})
