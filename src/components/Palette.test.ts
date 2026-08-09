import { mount } from '@vue/test-utils'
import axe from 'axe-core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

// The palette is mounted by the shell and takes a chord the shell dispatches, so
// the unit under test is only meaningful inside it. Statically imported for the
// reason `PanelShell.test.ts` records: a dynamic import after `vi.resetModules()`
// resolves a second copy of modules whose state is module-scoped by design.
import PanelShell from './PanelShell.vue'
import { useInteractionMode } from '@/composables/useInteractionMode'
import { useNoteEditor } from '@/composables/useNoteEditor'
import { useNoteSearch } from '@/composables/useNoteSearch'
import { usePalette } from '@/composables/usePalette'
import { useSections } from '@/composables/useSections'
import { useSelection } from '@/composables/useSelection'
import { useSettings } from '@/composables/useSettings'
import { useSpace } from '@/composables/useSpace'
import { useSpaces } from '@/composables/useSpaces'
import { useView } from '@/composables/useView'
import type { Space, StoreStatus } from '@/composables/useSpace'
import type { RecentEntry } from '@/composables/useSpaces'

const editor = useNoteEditor()
const palette = usePalette()
const search = useNoteSearch()
const sections = useSections()
const selection = useSelection()
const settings = useSettings()
const space = useSpace()
const spaces = useSpaces()
const view = useView()

// happy-dom implements no Web Animations API and auto-animate calls `el.animate`
// out of band, so a filtered row throws rather than failing an assertion. Same
// stub, and the same reason, as the shell's own suite.
const elementPrototype = Element.prototype as unknown as Record<string, unknown>
if (elementPrototype.animate === undefined) {
	elementPrototype.animate = () => ({
		playState: 'finished',
		finished: Promise.resolve(),
		cancel: () => {},
		removeEventListener: () => {},
		addEventListener: (name: string, handler: () => void) => {
			if (name === 'finish') queueMicrotask(handler)
		},
	})
}

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {}, emit: async () => {} }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))
vi.mock('@tauri-apps/api/webview', () => ({
	getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}))

/** Task-026's shipped default: off, unconfigured, nothing to report. The Share
 *  section renders from this, and the note context menu's **Send to my other
 *  device** stays disabled under it. */
const SHARE_CONFIG = {
	enabled: false,
	relayUrl: '',
	role: 'first',
	tokenSet: false,
	secretSet: false,
	configured: false,
	lastError: null,
}

const SPACE: Space = {
	id: 'spc_1',
	name: 'development',
	activeSection: 'sec_a',
	sections: [
		{ id: 'sec_a', name: 'Research', order: 0 },
		{ id: 'sec_b', name: 'Inbox', order: 1 },
	],
	notes: [
		{
			id: 'nte_1',
			section: 'sec_a',
			order: 0,
			done: false,
			body: 'first note',
			created: '2026-08-05T00:00:00Z',
			updated: '2026-08-05T00:00:00Z',
		},
	],
}

const STATUS: StoreStatus = {
	path: 'C:\\notes.copper',
	errored: false,
	watching: true,
	canUndo: false,
	canRedo: false,
	startupNotice: null,
}

/** Two entries, recency-ordered as Rust hands them over — the palette adds no
 *  ordering of its own, so the second one being the *active* space is what proves
 *  it is not sorting them. */
const RECENTS: RecentEntry[] = [
	{
		path: 'C:\\archive.copper',
		displayPath: 'C:\\archive.copper',
		key: 'c:\\archive.copper',
		name: 'archive',
		active: false,
		availability: { state: 'available' },
	},
	{
		path: 'C:\\notes.copper',
		displayPath: 'C:\\notes.copper',
		key: 'c:\\notes.copper',
		name: 'development',
		active: true,
		availability: { state: 'available' },
	},
]

const SHORTCUTS = {
	capture: 'Shift Shift',
	summon: 'Ctrl+Shift+Space',
	defaults: { capture: 'Shift Shift', summon: 'Ctrl+Shift+Space' },
	summonRegistered: true,
	summonError: null,
	captureRegistered: true,
	captureError: null,
	captureFallback: null,
	summonFallback: null,
}

let settingsPayload: Record<string, unknown> = {}

function defaultSettings() {
	return {
		recents: [],
		activeSpace: 0,
		panelPosition: null,
		shortcuts: {},
		theme: 'system',
		sounds: false,
		motion: 'auto',
		insertionPoint: 'bottom',
		doubleClick: 'copy',
		alwaysOnTop: true,
		showCreated: false,
		captureNotifications: true,
		linkPreviews: false,
	}
}

async function baseInvoke(command: string) {
	if (command === 'get_active_space') return SPACE
	if (command === 'get_status') return STATUS
	if (command === 'get_settings') return settingsPayload
	if (command === 'get_shortcut_state') return SHORTCUTS
	if (command === 'get_autostart_enabled') return false
	if (command === 'get_share_config') return SHARE_CONFIG
	if (command === 'editor_handoffs') return []
	if (command === 'list_recents') return RECENTS
	if (command === 'refresh_recents') return null
	if (command === 'set_active_section') return SPACE
	if (command === 'activate_space') return { changed: false, space: null }
	if (command === 'update_settings' || command === 'set_always_on_top') return settingsPayload
	if (command === 'set_theme_preference') return settingsPayload
	if (command === 'set_autostart_enabled') return true
	if (command === 'delete_notes') return SPACE
	if (command === 'hide_panel') return null
	throw { kind: 'invalid', message: command }
}

beforeEach(() => {
	vi.resetModules()
	mocks.invoke.mockReset()
	settingsPayload = defaultSettings()
	mocks.invoke.mockImplementation(baseInvoke)
})

let panel: ReturnType<typeof mount> | null = null

afterEach(async () => {
	panel?.unmount()
	panel = null

	// Module-scoped by design, so it outlives the component tree exactly as it
	// does in the app — and a palette left open declines every chord in the next
	// test through the shell's overlay guard, which is the worst of the states to
	// inherit silently.
	palette.close()
	editor.cancel()
	search.clearQuery()
	sections.reset()
	selection.clear()
	useInteractionMode().exit()
	space.clearActionError('list')
	view.showList()

	settingsPayload = defaultSettings()
	mocks.invoke.mockImplementation(baseInvoke)
	await space.refresh()
	await settings.refresh()
	await spaces.refresh()

	document.body.innerHTML = ''
})

async function settle(turns = 4) {
	for (let i = 0; i < turns; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

/** `useSettings` is initialised by `App`, not by the shell, so the action labels
 *  have nothing to read until this runs. */
async function mountPanel() {
	panel = mount(PanelShell, { attachTo: document.body })
	await settle(6)
	await settings.refresh()
	await settle(2)
	return panel as ReturnType<typeof mount<typeof PanelShell>>
}

function overlay() {
	return document.querySelector<HTMLElement>('[data-slot="command-overlay"]')
}

function filter() {
	return document.querySelector<HTMLInputElement>('#command-filter')
}

function rows() {
	return [...(overlay()?.querySelectorAll<HTMLElement>('[role="option"]') ?? [])]
}

function headings() {
	return [...(overlay()?.querySelectorAll('[data-slot="command-group-label"]') ?? [])].map((node) =>
		node.textContent?.trim(),
	)
}

function rowNamed(text: string) {
	const row = rows().find((item) => item.textContent?.includes(text))
	expect(row, `no palette row containing “${text}”`).toBeTruthy()
	return row!
}

/** Typed the way a person types: reka reads the value off the event target, so
 *  assigning without dispatching filters nothing. */
async function type(text: string) {
	const field = filter()!
	field.value = text
	field.dispatchEvent(new Event('input', { bubbles: true }))
	await settle(2)
}

async function press(key: string) {
	filter()!.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }))
	await settle(3)
}

async function openWithChord(wrapper: Awaited<ReturnType<typeof mountPanel>>, from = '#composer') {
	const target = wrapper.find(from)
	;(target.element as HTMLElement).focus()
	await target.trigger('keydown', { key: 'k', ctrlKey: true })
	await settle(3)
}

describe('opening', () => {
	/**
	 * AC-1. The chord sits above the shell's text-surface guard and, unlike the
	 * section switcher's version of it, carries no condition at all — so the three
	 * surfaces that used to swallow every chord are three more places it works.
	 */
	it('opens from the composer, the search field and the inline editor alike', async () => {
		const wrapper = await mountPanel()

		await openWithChord(wrapper, '#composer')
		expect(palette.isOpen.value).toBe(true)
		palette.close()
		await settle(2)

		await openWithChord(wrapper, '#panel-search')
		expect(palette.isOpen.value).toBe(true)
		palette.close()
		await settle(2)

		// The editor's textarea is the third text surface, and the one the switcher
		// stayed suppressed in on the argument that it is not where "where does the
		// next capture land" is asked. "Open the command palette" is asked anywhere.
		editor.beginEdit(SPACE, SPACE.notes[0]!)
		await settle(2)
		await openWithChord(wrapper, 'textarea[aria-label="Edit note"]')
		expect(palette.isOpen.value).toBe(true)
	})

	it('opens with the panel itself focused, and takes focus off it', async () => {
		const wrapper = await mountPanel()

		await wrapper.trigger('keydown', { key: 'k', ctrlKey: true })
		await settle(4)

		expect(overlay(), 'the palette did not open').not.toBeNull()
		// The field is the only focusable control in there: `ListboxFilter` takes the
		// list out of the tab order for as long as it is mounted.
		expect(document.activeElement?.id).toBe('command-filter')
	})

	it('probes availability on open, the way the overflow menu does', async () => {
		// Listing recents is a pure read of cached state, so opening is what makes
		// the answers current. It is also the only trigger allowed to start probes —
		// from a store event the results would ask for another listing and the two
		// would drive each other.
		const wrapper = await mountPanel()
		mocks.invoke.mockClear()

		await openWithChord(wrapper)

		expect(mocks.invoke).toHaveBeenCalledWith('refresh_recents')
	})

	it('survives being asked twice, and clears the query between openings', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)
		await type('inbox')
		expect(filter()!.value).toBe('inbox')

		// A second press while it is already up must not re-record the focus to
		// return to — the field it would record is the palette's own.
		await openWithChord(wrapper)
		expect(overlay()).not.toBeNull()

		palette.close()
		await settle(3)
		await openWithChord(wrapper)

		// A filter that survived a dismissal brings the next opening up
		// pre-filtered, which is the failure the switcher's lifecycle records.
		expect(filter()!.value).toBe('')
	})
})

describe('closing', () => {
	/** AC-2. */
	it('returns focus to whatever had it, from anywhere', async () => {
		const wrapper = await mountPanel()
		const composer = wrapper.find('#composer').element as HTMLTextAreaElement
		await wrapper.find('#composer').setValue('half a thought')
		composer.focus()

		await openWithChord(wrapper)
		expect(document.activeElement).not.toBe(composer)

		await press('Escape')

		expect(palette.isOpen.value).toBe(false)
		expect(document.activeElement).toBe(composer)
		// Opening a palette must cost nothing that was already typed.
		expect(composer.value).toBe('half a thought')
	})

	/**
	 * The palette is hand-rolled like the image viewer and still has no rung on the
	 * shell's Escape ladder, because it traps focus like the section switcher does:
	 * the press resolves at the `inOverlay` guard above the ladder and never
	 * reaches it.
	 */
	it('closes on Escape without taking a rung of the ladder with it', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await wrapper.find('#panel-search').setValue('first')
		await settle(3)

		await openWithChord(wrapper)
		await press('Escape')

		expect(palette.isOpen.value).toBe(false)
		expect(search.query.value).toBe('first')
		expect(selection.selectedIds.value).toEqual(['nte_1'])
	})

	it('closes on a click outside the card', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		overlay()!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
		await settle(3)

		expect(palette.isOpen.value).toBe(false)
	})

	/**
	 * **The single highest-risk integration point.** `inOverlay` is a hard-coded
	 * allowlist, and the palette is not a reka menu — so without its entry every
	 * in-panel chord would keep firing underneath the open overlay, and `Delete`
	 * would take the selected notes while the palette stood there filtering.
	 */
	it('owns the keyboard while it is up, so Delete does not reach the notes', async () => {
		const wrapper = await mountPanel()
		selection.select('nte_1')
		await openWithChord(wrapper)
		mocks.invoke.mockClear()

		filter()!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }))
		await settle(3)

		expect(mocks.invoke).not.toHaveBeenCalledWith('delete_notes', expect.anything())
		expect(palette.isOpen.value).toBe(true)
	})
})

describe('the three groups', () => {
	/** AC-8: the order is Rust's — `touch_recent` moves an entry to the front — so
	 *  the palette renders `recents` as it arrives. */
	it('lists recent spaces in the order they were handed over', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		expect(headings()).toEqual(['Spaces', 'Sections', 'Actions'])
		const spaceRows = rows().filter((row) => row.textContent?.match(/archive|development/))
		expect(spaceRows[0]?.textContent).toContain('archive')
		expect(spaceRows[1]?.textContent).toContain('development')
		// Marked with colour *and* a non-colour cue, as every active row in the
		// panel is.
		expect(spaceRows[1]?.textContent).toContain('(active space)')
	})

	/** AC-9. */
	it('lists the active space’s sections with what each one holds', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		const research = rowNamed('Research')
		expect(research.textContent).toContain('1 note')
		// An empty section still says so rather than showing nothing.
		expect(rowNamed('Inbox').textContent).toContain('0 notes')
		expect(research.textContent).toContain('(active section)')
	})

	/** AC-10, including both toggles that landed after this task was written. */
	it('offers every settings-surface action, the two newest included', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		const labels = rows().map((row) => row.textContent ?? '')
		for (const label of [
			'Theme: System',
			'Theme: Light',
			'Theme: Dark',
			'Keep on top',
			'New notes go',
			'Double-click a note',
			'Date added',
			'Sound',
			'Animate controls',
			'Capture notifications',
			'Link previews',
			'Launch Copper at login',
			'Open Settings',
			'Check for updates',
		]) {
			expect(
				labels.some((text) => text.includes(label)),
				`${label} is unreachable`,
			).toBe(true)
		}
	})

	it('says what each toggle is set to right now', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		// Read live rather than snapshotted: a list built at import time would have
		// captured a `settings.value` of null and said `Off` about everything.
		expect(rowNamed('Keep on top').textContent).toContain('On')
		expect(rowNamed('Link previews').textContent).toContain('Off')
		expect(rowNamed('New notes go').textContent).toContain('Bottom')
	})
})

describe('filtering', () => {
	/** AC-4: the fzf-style matcher, not a substring test and not a second matcher.
	 *  The needle is a folded character *sequence*, so the spaces in a query are
	 *  stripped rather than split on. */
	it('matches a subsequence across the label, whitespace and case ignored', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		await type('kot')
		expect(rows().map((row) => row.textContent)).toContainEqual(
			expect.stringContaining('Keep on top'),
		)

		await type('K E E P')
		expect(rowNamed('Keep on top')).toBeTruthy()
	})

	it('ranks by score rather than listing in declaration order', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		// `dat` is contiguous and word-initial in `Date added` and scattered across
		// `Double-click a note`; both match, and the ranking is the whole reason for
		// scoring rather than filtering.
		await type('dat')
		expect(rows()[0]?.textContent).toContain('Date added')
	})

	/** AC-5. */
	it('hides a group with no matches', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)
		expect(headings()).toEqual(['Spaces', 'Sections', 'Actions'])

		await type('theme')
		expect(headings()).toEqual(['Actions'])

		await type('inbox')
		expect(headings()).toEqual(['Sections'])
	})

	it('shows everything again for an empty query, rather than nothing', async () => {
		// An empty needle matches *nothing* in `fuzzyMatch` — "no query" is a
		// separate state the caller has to branch on, and handing the matcher an
		// empty needle would empty the palette the moment a query was cleared.
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		await type('theme')
		const filtered = rows().length
		await type('')

		expect(rows().length).toBeGreaterThan(filtered)
		expect(headings()).toEqual(['Spaces', 'Sections', 'Actions'])
	})

	it('says so when nothing matches at all', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		await type('zzzzz')

		expect(rows()).toHaveLength(0)
		expect(overlay()?.textContent).toContain('Nothing matches')
	})
})

describe('selecting', () => {
	/** AC-3, over the group that is furthest from the field. */
	it('walks the rows with the arrow keys and runs the one Enter lands on', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		await type('inbox')
		expect(rows()).toHaveLength(1)
		// The highlight follows a narrowing query, so Enter resolves the first
		// result without an arrow key being pressed at all.
		await press('Enter')

		expect(mocks.invoke).toHaveBeenCalledWith('set_active_section', { id: 'sec_b' })
		expect(palette.isOpen.value).toBe(false)
	})

	function highlightedIndex() {
		return rows().findIndex((row) => row.hasAttribute('data-highlighted'))
	}

	it('opens with the first row highlighted and walks from there', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		// Reka highlights the first row on every keystroke and never before the
		// first one, so the palette asks for it once on open — a palette whose Enter
		// does nothing until a character has been typed looks broken on arrival.
		expect(highlightedIndex()).toBe(0)

		await press('ArrowDown')
		expect(highlightedIndex()).toBe(1)

		await press('ArrowUp')
		expect(highlightedIndex()).toBe(0)
	})

	it('walks straight across a group boundary, because the collection spans them', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		// Two spaces, then the first section: grouping is presentational, and the
		// highlight is reka's one collection over every row in the list.
		await press('ArrowDown')
		await press('ArrowDown')
		expect(rows()[highlightedIndex()]?.textContent).toContain('Research')
	})

	it('switches space on select, and closes', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		rowNamed('archive').click()
		await settle(4)

		expect(mocks.invoke).toHaveBeenCalledWith('activate_space', { path: 'C:\\archive.copper' })
		expect(palette.isOpen.value).toBe(false)
	})

	it('switches section on select, through the store rather than the switcher', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		rowNamed('Inbox').click()
		await settle(4)

		expect(mocks.invoke).toHaveBeenCalledWith('set_active_section', { id: 'sec_b' })
		// The switcher is a separate surface with its own lifecycle; the palette
		// calls `useSpace` directly, exactly as the switcher's own rows do.
		expect(sections.switcherOpen.value).toBe(false)
		expect(palette.isOpen.value).toBe(false)
	})

	it('executes a toggle and closes', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		rowNamed('Capture notifications').click()
		await settle(4)

		expect(mocks.invoke).toHaveBeenCalledWith('update_settings', {
			patch: { captureNotifications: false },
		})
		expect(palette.isOpen.value).toBe(false)
	})

	it('reaches the settings view, which is what covers the two recorder rows', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		rowNamed('Open Settings').click()
		await settle(3)

		expect(view.view.value).toBe('settings')
	})

	/**
	 * A refused write has one surface here, and it is the panel's error band —
	 * the position `PanelHeader`'s pin is in, for the same reason: a palette row
	 * has no inline slot of its own, and it has closed by the time the answer
	 * arrives.
	 */
	it('reports a refused write into the band behind the closed palette', async () => {
		const wrapper = await mountPanel()
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'set_always_on_top') throw { kind: 'io', message: 'The file is read-only.' }
			return baseInvoke(command)
		})
		await openWithChord(wrapper)

		rowNamed('Keep on top').click()
		await settle(4)

		expect(palette.isOpen.value).toBe(false)
		expect(space.actionError.value?.message).toContain('read-only')
	})
})

describe('presentation', () => {
	/**
	 * AC-7 needs no code of its own, and this is the assertion that says why — but
	 * the answer changed. The palette had an `animate-in` fade, which was reachable
	 * by `main.css`'s `.reduce-motion` block and therefore correct as far as it
	 * went; it should not have been there at all.
	 *
	 * **A command palette has no entrance.** It is reached by a chord and by nothing
	 * else, which means it is only ever opened by someone who already knew they
	 * wanted it and is already typing. Every millisecond of fade is time the field
	 * is on screen and not yet worth looking at, and it lands squarely on the one
	 * path in the app where the user is fastest. Nothing here to reduce, for anyone.
	 */
	it('opens with no animation at all, because a chord is not an arrival', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		expect(overlay()?.className).not.toContain('animate-in')
		expect(overlay()?.querySelector('[data-slot="command"]')?.className ?? '').not.toContain(
			'animate-in',
		)
	})

	/**
	 * The hairline between two groups is drawn by `CommandGroup`'s own
	 * `[data-slot='command-group'] + [data-slot='command-group']` rule, so the
	 * groups being *adjacent siblings* is not an implementation detail — it is the
	 * whole mechanism. A wrapper around one of them, or any node slipped between
	 * two, would take every divider away and nothing would look broken enough to
	 * notice. This is the half a test can hold; the drawing itself is CSS.
	 *
	 * The second half is why it is CSS at all: which group renders first changes
	 * with every keystroke, and a lone group must never carry a rule above it.
	 */
	it('leaves the groups adjacent, and never gives a lone group a rule above it', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)

		const list = overlay()?.querySelector('[data-slot="command-list"]')
		const groups = [...(list?.querySelectorAll('[data-slot="command-group"]') ?? [])]
		expect(groups).toHaveLength(3)
		expect(groups[1]?.previousElementSibling).toBe(groups[0])
		expect(groups[2]?.previousElementSibling).toBe(groups[1])

		// Whichever group survives the filter is the first one, and the rule is a
		// question about what precedes it rather than about which group it is.
		await type('theme')
		const alone = [...(list?.querySelectorAll('[data-slot="command-group"]') ?? [])]
		expect(alone).toHaveLength(1)
		expect(alone[0]?.previousElementSibling).toBeNull()
	})

	/**
	 * The palette is the ARIA-correct shape for "filter a list", which is the whole
	 * reason `Listbox` is the primitive: a textbox may not be a child of
	 * `role="menu"`, and `SectionSwitcher` documents that knowing violation at
	 * length. Here there is nothing to exclude.
	 */
	it('reports no axe violations while open', async () => {
		const wrapper = await mountPanel()
		await openWithChord(wrapper)
		expect(overlay(), 'the palette did not open').not.toBeNull()

		// Scoped to the overlay, as the switcher's run is: a whole-document run
		// reports the panel content behind an `aria-modal` dialog, which is not this
		// component's finding and not actionable here.
		const results = await axe.run(overlay()!, {
			rules: {
				// Needs a real layout and paint; verified by hand, as the shell's
				// whole-panel run records.
				'color-contrast': { enabled: false },
			},
		})

		expect(
			results.violations.map((violation) => `${violation.id}: ${violation.nodes.length} node(s)`),
		).toEqual([])
	}, 30_000)
})
