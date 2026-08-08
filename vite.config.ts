import { fileURLToPath, URL } from 'node:url'
import { defineConfig, lazyPlugins } from 'vite-plus'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'
import vueDevTools from 'vite-plugin-vue-devtools'
import AutoImport from 'unplugin-auto-import/vite'
import Components from 'unplugin-vue-components/vite'
import Icons from 'unplugin-icons/vite'
import IconsResolver from 'unplugin-icons/resolver'
import Unfonts from 'unplugin-fonts/vite'

export default defineConfig({
	fmt: {
		semi: false,
		singleQuote: true,
		useTabs: true,
		// unplugin regenerates the d.ts files on every dev/build run in its own
		// style, so formatting them just makes `vp check` fail again after the next
		// build. `.claude/` is tool-owned config that any Claude Code user in this
		// repo will have; without it `vp check` fails on a file nobody edited.
		ignorePatterns: ['src/auto-imports.d.ts', 'src/components.d.ts', '.claude/'],
	},
	lint: {
		jsPlugins: [{ name: 'vite-plus', specifier: 'vite-plus/oxlint-plugin' }],
		rules: { 'vite-plus/prefer-vite-plus-imports': 'error' },
		options: { typeAware: true, typeCheck: true },
	},
	plugins: lazyPlugins(() => [
		vue(),
		tailwindcss(),
		vueDevTools(),
		AutoImport({
			imports: ['vue', '@vueuse/core', { from: '@/lib/utils', imports: ['cn'] }],
			dirs: ['src/composables'],
			dts: 'src/auto-imports.d.ts',
			vueTemplate: true,
		}),
		Components({
			dts: 'src/components.d.ts',
			resolvers: [IconsResolver({ prefix: 'Icon' })],
		}),
		Icons({ compiler: 'vue3', autoInstall: true }),
		Unfonts({
			fontsource: {
				families: [{ name: 'Inter Variable', variable: true }],
			},
		}),
	]),
	resolve: {
		alias: {
			// Must be absolute — Vite cannot use a bare './src' as a filesystem
			// alias replacement. Mirrored in tsconfig.app.json's paths.
			'@': fileURLToPath(new URL('./src', import.meta.url)),
		},
	},

	// --- Options Tauri requires of its frontend dev server and build ---
	// Keep Tauri's Rust compiler output from being wiped by the frontend server.
	clearScreen: false,
	// Vite matches env prefixes with a literal startsWith, so Tauri's documented
	// 'TAURI_ENV_*' would match nothing at all — the trailing * is not a wildcard.
	envPrefix: ['VITE_', 'TAURI_ENV_'],
	server: {
		port: 1420,
		// A silent fallback to 1421 would break Tauri's devUrl.
		strictPort: true,
		host: process.env.TAURI_DEV_HOST || false,
		// Without this, Tauri's own rebuilds churn src-tauri/target/ and the dev
		// server ends up in a reload loop.
		watch: { ignored: ['**/src-tauri/**'] },
	},
	// `vp test` bundles Vitest, so nothing extra is installed for the runner
	// itself. happy-dom rather than jsdom: the suites here need a DOM for focus,
	// datasets and ResizeObserver-free measurement, not a browser.
	test: {
		environment: 'happy-dom',
		// happy-dom stubs `Element.animate` but ships neither `KeyframeEffect` nor
		// `Animation`, and auto-animate's plugin path constructs both by name.
		setupFiles: ['./src/testing/waapi.ts'],
		// `tests/` is for suites that assert the *build*, not the app: they read
		// node builtins, which tsconfig.app.json deliberately gives no types for so
		// that a component importing `node:fs` stays a type error.
		include: ['src/**/*.test.ts', 'tests/**/*.test.ts'],
		restoreMocks: true,
	},

	build: {
		target: 'chrome105',
		// Boolean rather than a named minifier: Vite+ builds on Rolldown, where
		// Vite's 'esbuild' value is not a safe assumption.
		minify: !process.env.TAURI_ENV_DEBUG,
		sourcemap: !!process.env.TAURI_ENV_DEBUG,
	},
})
