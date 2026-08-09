import { mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import SettingsSlider from './SettingsSlider.vue'
import SettingsView from './SettingsView.vue'
import { useAttachments } from '@/composables/useAttachments'
import { useView } from '@/composables/useView'
import { SHARE_SETUP_PROMPT } from '@/lib/shareSetupPrompt'
import type { Settings } from '@/composables/useSpace'

/**
 * The two rows task-012 adds. The `sounds` row is an ordinary boolean; the
 * `motion` row is the one worth a test, because the control and the stored value
 * do not have the same shape — the switch is "animate", the setting is
 * `auto | off`, and an inverted mapping would look completely plausible in
 * review.
 */

const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
	openUrl: vi.fn<(url: string) => Promise<void>>(),
	/** `DropTarget`'s listener — the settings view mounts one too, so a test can
	 *  hand it an OS drag event. Boxed for the reason `PanelShell.test` gives: the
	 *  `vi.mock` factory closes over this object. */
	dragDrop: { deliver: null as ((payload: unknown) => unknown) | null },
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
// The Share setup guide's two links leave for the user's browser rather than
// navigating the WebView. The plugin reaches Rust, which does not exist here.
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: mocks.openUrl }))
vi.mock('@tauri-apps/api/event', () => ({
	emit: vi.fn(),
	listen: async () => () => {},
}))
// `DropTarget` subscribes on mount, and `getCurrentWebview` reaches into
// `window.__TAURI_INTERNALS__`, which does not exist outside the real webview —
// the same stub `PanelShell.test` carries, for the same reason.
vi.mock('@tauri-apps/api/webview', () => ({
	getCurrentWebview: () => ({
		onDragDropEvent: async (handler: (payload: unknown) => unknown) => {
			mocks.dragDrop.deliver = handler
			return () => {
				mocks.dragDrop.deliver = null
			}
		},
	}),
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

function makeSettings(over: Partial<Settings> = {}): Settings {
	return {
		recents: ['C:\\notes.copper'],
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
		translucent: false,
		neutral: 'warm',
		accent: 'copper',
		vibrancy: 1,
		resizable: false,
		panelWidth: 440,
		panelHeight: 760,
		...over,
	}
}

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

async function flush(times = 4) {
	for (let i = 0; i < times; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

/** The last mount, retired by the next one. The view now owns a `document`-level
 *  paste listener, so leaving every test's mount attached — which was merely
 *  untidy before — would make one dispatched paste fan out to every stale
 *  instance and ingest a file per leak. */
let mounted: ReturnType<typeof mount<typeof SettingsView>> | null = null

afterEach(() => {
	mounted?.unmount()
	mounted = null
})

async function openSettings(stored: Partial<Settings> = {}) {
	// Before the mock is touched, so anything an unmount hook happens to invoke
	// lands in the outgoing test's call log rather than the new one's.
	mounted?.unmount()

	// No module reset here, unlike the suites that use one: `SettingsView` and the
	// composables behind it are static imports, and `vi.resetModules()` cannot
	// re-evaluate a module that has already been imported. What separates the cases
	// is that every mount pulls `get_settings` again and overwrites the
	// module-scoped value.
	//
	// Cleared per mount, not per test: a case that opens the view twice is
	// comparing what the *second* one wrote.
	mocks.invoke.mockClear()
	mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
		switch (command) {
			case 'get_settings':
				return makeSettings(stored)
			case 'get_shortcut_state':
				return SHORTCUTS
			case 'get_autostart_enabled':
				return false
			case 'get_app_version':
				return '0.1.0'
			// Task-026. The Share section is always mounted; its rows below the
			// enable switch are not, because the switch ships off.
			case 'get_share_config':
				return SHARE_CONFIG
			// The setup guide's **Copy prompt** button, which goes through
			// `useSystemClipboard` and so through Rust, like every other clipboard
			// write in the app.
			case 'clipboard_write_text':
				return null
			case 'update_settings':
				return makeSettings({ ...stored, ...(args?.patch as Partial<Settings>) })
			// Its own command rather than a patch, because it has a native side to
			// apply — so the responder has to answer with the whole settings object
			// the same way `set_theme_preference` does.
			case 'set_always_on_top':
				return makeSettings({ ...stored, alwaysOnTop: args?.enabled as boolean })
			default:
				throw { kind: 'invalid', message: `no responder: ${command}` }
		}
	})

	const wrapper = mount(SettingsView, { attachTo: document.body })
	mounted = wrapper
	await flush()
	return wrapper
}

function patchesSent() {
	return mocks.invoke.mock.calls
		.filter((call) => call[0] === 'update_settings')
		.map((call) => (call[1] as { patch: unknown }).patch)
}

beforeEach(() => {
	mocks.invoke.mockReset()
	mocks.openUrl.mockReset()
})

describe('the sound and motion rows', () => {
	it('renders both as switches the keyboard can reach and name', async () => {
		const wrapper = await openSettings()

		for (const id of ['sounds', 'motion']) {
			const control = wrapper.get(`#${id}`)
			// reka renders a real `<button role="switch">`, which Space and Enter both
			// operate — the same control task-008 shipped for autostart.
			expect(control.element.tagName).toBe('BUTTON')
			expect(control.attributes('role')).toBe('switch')
			expect(control.attributes('aria-checked')).toBeDefined()
			// A switch has no text of its own, so the row's visible label is what
			// names it.
			expect(wrapper.find(`label[for="${id}"]`).exists()).toBe(true)
		}
	})

	it('shows sound off by default and turns it on with a patch of one key', async () => {
		const wrapper = await openSettings()
		expect(wrapper.get('#sounds').attributes('aria-checked')).toBe('false')

		await wrapper.get('#sounds').trigger('click')
		await flush()

		expect(patchesSent()).toEqual([{ sounds: true }])
	})

	/** `auto` is the animating value, so the switch reads *on* — the row that
	 *  would be easiest to wire backwards. */
	it('reads the animate switch as on for auto and off for off', async () => {
		const auto = await openSettings({ motion: 'auto' })
		expect(auto.get('#motion').attributes('aria-checked')).toBe('true')

		const off = await openSettings({ motion: 'off' })
		expect(off.get('#motion').attributes('aria-checked')).toBe('false')
	})

	it('stores off when animation is switched off, and auto when it is switched back', async () => {
		const on = await openSettings({ motion: 'auto' })
		await on.get('#motion').trigger('click')
		await flush()
		expect(patchesSent()).toEqual([{ motion: 'off' }])

		const off = await openSettings({ motion: 'off' })
		await off.get('#motion').trigger('click')
		await flush()
		expect(patchesSent()).toEqual([{ motion: 'auto' }])
	})

	/** Each key is written on its own, so turning sound on cannot silently rewrite
	 *  a motion preference read at mount — the failure mode a whole-object save
	 *  would have. */
	it('never writes the two keys together', async () => {
		const wrapper = await openSettings({ motion: 'off' })

		await wrapper.get('#sounds').trigger('click')
		await flush()

		expect(patchesSent()).toEqual([{ sounds: true }])
	})
})

/**
 * Task-013's two rows. Both are two-value *choices* rather than booleans, which
 * is why they are radio groups and not switches — and the ARIA is the whole
 * point of that decision, so it is what this asserts.
 */
describe('the always-on-top row', () => {
	it('renders on by default and turns off through its own Rust command', async () => {
		const wrapper = await openSettings()

		const control = wrapper.get('#always-on-top')
		expect(control.attributes('role')).toBe('switch')
		expect(control.attributes('aria-checked')).toBe('true')
		expect(wrapper.find('label[for="always-on-top"]').exists()).toBe(true)

		await control.trigger('click')
		await flush()

		// Not `update_settings`. Unlike `sounds` and `motion` this preference has a
		// native side — the window's z-order band — so it goes through the command
		// that applies it before persisting it, exactly as the theme does.
		expect(patchesSent()).toEqual([])
		expect(mocks.invoke).toHaveBeenCalledWith('set_always_on_top', { enabled: false })
		expect(wrapper.get('#always-on-top').attributes('aria-checked')).toBe('false')
	})

	it('reads a stored false as off', async () => {
		const wrapper = await openSettings({ alwaysOnTop: false })

		expect(wrapper.get('#always-on-top').attributes('aria-checked')).toBe('false')
	})

	/** The row's own error slot, not the panel's band: a failure here has to
	 *  render next to the control that produced it. */
	it('reports a refused write on its own row and leaves the switch where it was', async () => {
		const wrapper = await openSettings()
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'set_always_on_top') {
				throw { kind: 'persist', message: "Copper couldn't save the always-on-top setting" }
			}
			throw { kind: 'invalid', message: `no responder: ${command}` }
		})

		await wrapper.get('#always-on-top').trigger('click')
		await flush()

		expect(wrapper.text()).toContain("Copper couldn't save the always-on-top setting")
		expect(wrapper.get('#always-on-top').attributes('aria-checked')).toBe('true')
	})
})

/**
 * A row's failure has to be *announced* and it has to be *reachable from the
 * control*, and those are two different mechanisms with two different failure
 * modes. A `v-if`'d `role="alert"` satisfies neither reliably: the region is
 * injected together with its text, which screen readers generally do not read,
 * and it is tied to nothing, so a user who tabs back to the switch is told
 * nothing about why it did not move.
 */
describe('a row that fails', () => {
	/** The row element around a trailing control: the control's wrapper is the
	 *  `shrink-0` box, and the row is its parent. Scoped rather than looked up by
	 *  id in the document, because every mount here stays attached and `useId`
	 *  restarts its counter per app. */
	function rowOf(control: Element) {
		return control.closest('div')?.parentElement ?? null
	}

	async function refuseAlwaysOnTop() {
		const wrapper = await openSettings()
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'set_always_on_top') {
				throw { kind: 'persist', message: "Copper couldn't save the always-on-top setting" }
			}
			throw { kind: 'invalid', message: `no responder: ${command}` }
		})
		await wrapper.get('#always-on-top').trigger('click')
		await flush()
		return wrapper
	}

	/** Mounted from the start and empty, so the announcement is a text change
	 *  inside a region already in the accessibility tree. Asserted on a row this
	 *  suite never fails — the settings errors are module-scoped and outlive a
	 *  remount, so a row another case has broken is not a clean "before". */
	it('keeps its alert region mounted before anything goes wrong', async () => {
		const wrapper = await openSettings()
		const control = wrapper.get('#sounds')

		const region = rowOf(control.element)?.querySelector('[role="alert"]')
		expect(region).not.toBeNull()
		expect(region?.textContent).toBe('')
		// Described by nothing while there is nothing to describe.
		expect(control.attributes('aria-describedby')).toBeUndefined()
		expect(control.attributes('aria-invalid')).toBeUndefined()
	})

	it('fills that same region rather than injecting a second one', async () => {
		const wrapper = await refuseAlwaysOnTop()

		const regions = rowOf(wrapper.get('#always-on-top').element)?.querySelectorAll('[role="alert"]')
		expect(regions).toHaveLength(1)
		expect(regions?.[0]?.textContent).toContain("Copper couldn't save the always-on-top setting")
	})

	it('points the control at the message that explains why it did not move', async () => {
		const wrapper = await refuseAlwaysOnTop()
		const control = wrapper.get('#always-on-top')
		expect(control.attributes('aria-invalid')).toBe('true')

		const described = control.attributes('aria-describedby')
		expect(described).toBeDefined()
		const target = rowOf(control.element)?.querySelector(`#${described}`)
		expect(target?.textContent).toContain("Copper couldn't save the always-on-top setting")
	})
})

describe('the notes rows', () => {
	function group(wrapper: ReturnType<typeof mount>, label: string) {
		return wrapper.get(`[role="radiogroup"][aria-label="${label}"]`)
	}

	function segment(wrapper: ReturnType<typeof mount>, label: string, text: string) {
		const found = group(wrapper, label)
			.findAll('[role="radio"]')
			.find((item) => item.text() === text)
		if (!found) throw new Error(`no ${text} segment in ${label}`)
		return found
	}

	it('renders each as one radiogroup rather than a row of independent toggles', async () => {
		const wrapper = await openSettings()

		for (const label of ['Where new notes go', 'What double-clicking a note does']) {
			const radios = group(wrapper, label).findAll('[role="radio"]')
			expect(radios).toHaveLength(2)
			// `aria-pressed` here would mean two independent toggle buttons, which is
			// exactly what `ToggleGroup` would have produced and why it was declined.
			for (const radio of radios) expect(radio.attributes('aria-checked')).toBeDefined()
		}
	})

	it('shows the shipped defaults, which are what every earlier build did', async () => {
		const wrapper = await openSettings()

		expect(segment(wrapper, 'Where new notes go', 'Bottom').attributes('aria-checked')).toBe('true')
		expect(
			segment(wrapper, 'What double-clicking a note does', 'Copy').attributes('aria-checked'),
		).toBe('true')
	})

	it('reflects a stored top and edit', async () => {
		const wrapper = await openSettings({ insertionPoint: 'top', doubleClick: 'edit' })

		expect(segment(wrapper, 'Where new notes go', 'Top').attributes('aria-checked')).toBe('true')
		expect(
			segment(wrapper, 'What double-clicking a note does', 'Edit').attributes('aria-checked'),
		).toBe('true')
	})

	it('writes one key per choice, so neither row can clear the other', async () => {
		const wrapper = await openSettings()

		await segment(wrapper, 'Where new notes go', 'Top').trigger('click')
		await flush()
		expect(patchesSent()).toEqual([{ insertionPoint: 'top' }])

		await segment(wrapper, 'What double-clicking a note does', 'Edit').trigger('click')
		await flush()
		expect(patchesSent()).toEqual([{ insertionPoint: 'top' }, { doubleClick: 'edit' }])
	})

	/** The moving pill is one element handed between segments, not a fill each
	 *  segment paints on itself — so exactly one exists per group, it sits on the
	 *  checked one, and it says nothing to a screen reader that `aria-checked` has
	 *  not already said. */
	it('marks the chosen segment with a single decorative pill', async () => {
		const wrapper = await openSettings({ insertionPoint: 'top' })

		const pills = group(wrapper, 'Where new notes go').findAll('[aria-hidden="true"]')
		expect(pills).toHaveLength(1)
		expect(
			segment(wrapper, 'Where new notes go', 'Top').find('[aria-hidden="true"]').exists(),
		).toBe(true)
		expect(
			segment(wrapper, 'Where new notes go', 'Bottom').find('[aria-hidden="true"]').exists(),
		).toBe(false)
	})

	/** A hand-edited value nothing recognises collapses to the default on read
	 *  rather than leaving the control showing nothing at all. */
	it('falls back to the default for a name it does not recognise', async () => {
		const wrapper = await openSettings({ insertionPoint: 'sideways', doubleClick: 'launch' })

		expect(segment(wrapper, 'Where new notes go', 'Bottom').attributes('aria-checked')).toBe('true')
		expect(
			segment(wrapper, 'What double-clicking a note does', 'Copy').attributes('aria-checked'),
		).toBe('true')
	})
})

/**
 * Task-016's one settings key. It is a *display* switch and nothing else: the
 * `created` it reveals has been recorded on every note since task-003, so
 * turning it on shows history that already exists rather than starting to
 * collect any.
 */
describe('the date-added switch', () => {
	it('ships off, so an upgrade shows the cards it showed before', async () => {
		const wrapper = await openSettings()
		expect(wrapper.get('#show-created').attributes('aria-checked')).toBe('false')
	})

	it('reflects a stored true', async () => {
		const wrapper = await openSettings({ showCreated: true })
		expect(wrapper.get('#show-created').attributes('aria-checked')).toBe('true')
	})

	/** One key wide, like every other row here, so writing this one cannot clear
	 *  the preference beside it. */
	it('writes only its own key', async () => {
		const wrapper = await openSettings()

		await wrapper.get('#show-created').trigger('click')
		await flush()

		expect(patchesSent()).toEqual([{ showCreated: true }])
	})
})

/**
 * Task-018's one settings key, and the one row in this view whose default is
 * *on*. A capture that lands while the panel is hidden produces nothing the user
 * can see, so shipping this off would ship a gesture with no confirmation and a
 * feature nobody discovers.
 */
describe('the capture-notifications switch', () => {
	it('ships on, so a hidden capture confirms itself out of the box', async () => {
		const wrapper = await openSettings()
		expect(wrapper.get('#capture-notifications').attributes('aria-checked')).toBe('true')
	})

	/** The half a default-off row cannot get wrong: an older `settings.json` has
	 *  no such key at all, and reading its absence as "off" would silence the
	 *  feature for every existing install. */
	it('reads an absent key as on', async () => {
		const wrapper = await openSettings({ captureNotifications: undefined })
		expect(wrapper.get('#capture-notifications').attributes('aria-checked')).toBe('true')
	})

	it('reflects a stored false', async () => {
		const wrapper = await openSettings({ captureNotifications: false })
		expect(wrapper.get('#capture-notifications').attributes('aria-checked')).toBe('false')
	})

	it('writes only its own key', async () => {
		const wrapper = await openSettings()

		await wrapper.get('#capture-notifications').trigger('click')
		await flush()

		expect(patchesSent()).toEqual([{ captureNotifications: false }])
	})
})

/**
 * Task-020's one settings key, and the only switch in this view whose "on"
 * position makes Copper contact anybody. The tests below are about the *default*
 * and about what the row says, because those are the two halves of consent — a
 * switch that shipped on, or one whose description only mentioned the visible
 * benefit, would both be consent nobody gave.
 */
describe('the link-previews switch', () => {
	it('ships off, so an upgrade fetches nothing until the user asks', async () => {
		const wrapper = await openSettings()
		expect(wrapper.get('#link-previews').attributes('aria-checked')).toBe('false')
	})

	/** The half this row cannot get wrong. Every `settings.json` written by an
	 *  earlier build has no such key, and reading its absence as "on" would mean
	 *  the upgrade itself was the moment Copper started disclosing which pages a
	 *  user's notes mention. */
	it('reads an absent key as off', async () => {
		const wrapper = await openSettings({ linkPreviews: undefined })
		expect(wrapper.get('#link-previews').attributes('aria-checked')).toBe('false')
	})

	it('reflects a stored true', async () => {
		const wrapper = await openSettings({ linkPreviews: true })
		expect(wrapper.get('#link-previews').attributes('aria-checked')).toBe('true')
	})

	it('writes only its own key', async () => {
		const wrapper = await openSettings()

		await wrapper.get('#link-previews').trigger('click')
		await flush()

		expect(patchesSent()).toEqual([{ linkPreviews: true }])
	})

	/** The description has to state what enabling *sends*, not only what it
	 *  shows. "Show cached page details below links" describes the visible half
	 *  and leaves the half a person would want to decide on to be discovered
	 *  later, which is not a description of a privacy setting. */
	it('says in the row what turning it on discloses', async () => {
		const wrapper = await openSettings()
		const row = wrapper.get('#link-previews').element.closest('div')?.parentElement
		const text = (row?.textContent ?? '') + wrapper.text()

		for (const promised of ['Fetching', 'IP address', 'when you read']) {
			expect(text).toContain(promised)
		}
	})

	/** The dedicated Privacy section is gone (2026-08-08, by request): the row
	 *  lives under Notes now, and the description carries the disclosure weight
	 *  the heading used to — which the test above already pins. What must not
	 *  come back is a heading with one row under it. */
	it('lives under Notes, with no Privacy section left behind', async () => {
		const wrapper = await openSettings()
		expect(wrapper.text()).not.toContain('Privacy')
		expect(wrapper.text()).not.toContain('Notifications')
		expect(wrapper.find('#link-previews').exists()).toBe(true)
		expect(wrapper.find('#capture-notifications').exists()).toBe(true)
	})
})

describe('the appearance rows', () => {
	// The responder answers `set_translucency` the way it answers
	// `set_always_on_top`, and for the same reason: a native side means the
	// command returns the whole settings object itself.
	async function openWithTranslucency(stored: Partial<Settings> = {}) {
		const wrapper = await openSettings(stored)
		const base = mocks.invoke.getMockImplementation()!
		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
			if (command === 'set_translucency') {
				return makeSettings({ ...stored, translucent: args?.enabled as boolean })
			}
			return base(command, args)
		})
		return wrapper
	}

	it('ships the translucency switch off and writes through its own command', async () => {
		const wrapper = await openWithTranslucency()
		const control = wrapper.get('#translucent')
		expect(control.attributes('role')).toBe('switch')
		expect(control.attributes('aria-checked')).toBe('false')

		await control.trigger('click')
		await flush()

		// Not `update_settings`: the material is native state the patch path could
		// neither apply nor undo.
		expect(mocks.invoke).toHaveBeenCalledWith('set_translucency', { enabled: true })
		expect(patchesSent()).toEqual([])
	})

	it('renders the two pickers as radiogroups sized to their palettes', async () => {
		const wrapper = await openSettings()

		const tones = wrapper.get('[aria-label="Grey tone"]')
		const accents = wrapper.get('[aria-label="Accent color"]')
		expect(tones.attributes('role')).toBe('radiogroup')
		expect(accents.attributes('role')).toBe('radiogroup')
		expect(tones.findAll('[role="radio"]')).toHaveLength(6)
		expect(accents.findAll('[role="radio"]')).toHaveLength(18)
	})

	it('marks the shipped palette, and names the choice in the row label', async () => {
		const wrapper = await openSettings()

		expect(wrapper.get('[aria-label="Warm"]').attributes('aria-checked')).toBe('true')
		expect(wrapper.get('[aria-label="Copper"]').attributes('aria-checked')).toBe('true')
		expect(wrapper.text()).toContain('Grey tone: Warm')
		expect(wrapper.text()).toContain('Accent color: Copper')
	})

	it('writes one key per picker, so neither can clear the other', async () => {
		const wrapper = await openSettings()

		await wrapper.get('[aria-label="Slate"]').trigger('click')
		await flush()
		await wrapper.get('[aria-label="Blue"]').trigger('click')
		await flush()

		expect(patchesSent()).toEqual([{ neutral: 'slate' }, { accent: 'blue' }])
	})

	/** The same rule `theme` and the notes rows follow: the store repairs a wrong
	 *  *type*, and a stored name nothing recognises collapses to the shipped
	 *  palette on read rather than rendering a picker with nothing selected. */
	it('falls back to the shipped palette for a name it does not recognise', async () => {
		const wrapper = await openSettings({ neutral: 'chartreuse', accent: 'gold' })

		expect(wrapper.get('[aria-label="Warm"]').attributes('aria-checked')).toBe('true')
		expect(wrapper.get('[aria-label="Copper"]').attributes('aria-checked')).toBe('true')
	})
})

describe('the vibrancy, resizable and size rows', () => {
	// Both window commands answer with the whole settings object, exactly as
	// `set_always_on_top` and `set_translucency` do and for the same reason: a
	// native side means the command is the writer, not the patch.
	async function openWithWindowCommands(stored: Partial<Settings> = {}) {
		const wrapper = await openSettings(stored)
		const base = mocks.invoke.getMockImplementation()!
		mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
			if (command === 'set_resizable') {
				return makeSettings({ ...stored, resizable: args?.enabled as boolean })
			}
			if (command === 'set_panel_size') {
				return makeSettings({
					...stored,
					panelWidth: args?.width as number,
					panelHeight: args?.height as number,
				})
			}
			return base(command, args)
		})
		return wrapper
	}

	/** The reorganisation, pinned by where the two moved rows now sit: the section
	 *  that grouped by mechanism is gone, and the rows kept their ids across the
	 *  move so nothing that points at them had to learn a new name. */
	it('renames Sound and motion to Behavior and keeps both moved rows reachable', async () => {
		const wrapper = await openSettings()

		expect(wrapper.text()).toContain('Behavior')
		expect(wrapper.text()).not.toContain('Sound and motion')
		expect(wrapper.find('#translucent').exists()).toBe(true)
		expect(wrapper.find('#always-on-top').exists()).toBe(true)
	})

	/** The slider's two events are the whole design: a drag repaints the document
	 *  through a preview and writes nothing, and only the released value becomes a
	 *  patch — thirty `settings.json` rewrites per drag is the failure the split
	 *  exists to prevent. Driven through the component's own events because reka's
	 *  pointer machinery needs real geometry happy-dom does not lay out.
	 *
	 *  The emitted values are *dial* units — the slider speaks 0–100 — and the
	 *  patch is the stored multiplier: dial 50 of a 0–3 scale is 1.5, and that
	 *  conversion happening at the slider's edge is exactly what this asserts. */
	it('previews a vibrancy drag without writing, and patches the multiplier on commit', async () => {
		const wrapper = await openSettings()
		const slider = wrapper.findComponent(SettingsSlider)

		slider.vm.$emit('update:modelValue', 50)
		await flush()
		expect(patchesSent()).toEqual([])

		slider.vm.$emit('commit', 50)
		await flush()
		expect(patchesSent()).toEqual([{ vibrancy: 1.5 }])
	})

	/** `aria-valuetext` because a chroma multiplier is not a number a person has
	 *  words for: the thumb announces the dial's percentage — a stored 1.5 on the
	 *  0–3 scale is half the dial — and the shipped 1 must land on 33, not on a
	 *  "100%" that would misread the scale as percent-of-default. */
	it('announces the vibrancy value as the dial percentage', async () => {
		const wrapper = await openSettings({ vibrancy: 1.5 })
		const thumb = wrapper.get('[role="slider"][aria-label="Vibrancy"]')
		expect(thumb.attributes('aria-valuetext')).toBe('50%')

		const shipped = await openSettings()
		const defaultThumb = shipped.get('[role="slider"][aria-label="Vibrancy"]')
		expect(defaultThumb.attributes('aria-valuetext')).toBe('33%')
		expect(defaultThumb.attributes('aria-valuenow')).toBe('33')
	})

	it('ships the resizable switch off and writes through its own command', async () => {
		const wrapper = await openWithWindowCommands()
		const control = wrapper.get('#resizable')
		expect(control.attributes('role')).toBe('switch')
		expect(control.attributes('aria-checked')).toBe('false')

		await control.trigger('click')
		await flush()

		// Not `update_settings`: the drag handles are native state the patch path
		// could neither apply nor undo.
		expect(mocks.invoke).toHaveBeenCalledWith('set_resizable', { enabled: true })
		expect(patchesSent()).toEqual([])
	})

	/** The clamp is client-side as well as Rust-side, so the field never shows a
	 *  number the store would have silently rewritten: a typed 9999 goes out as
	 *  the band's maximum, and what comes back is what the field then reads. */
	it('commits the size fields through set_panel_size, clamped to the band', async () => {
		const wrapper = await openWithWindowCommands()
		const width = wrapper.get('input[aria-label="Panel width in pixels"]')

		await width.setValue('9999')
		await width.trigger('blur')
		await flush()

		expect(mocks.invoke).toHaveBeenCalledWith('set_panel_size', { width: 1200, height: 760 })
		expect((width.element as HTMLInputElement).value).toBe('1200')
		expect(patchesSent()).toEqual([])
	})
})

/**
 * A file arriving while the settings are open. Both ingest surfaces the list
 * view carries reach into this view too — the OS drop through the `DropTarget`
 * it now mounts, the paste through its own file-only listener — and both hand
 * the user back to the list, because the tray they filled lives in the
 * composer.
 */
describe('attaching from the settings view', () => {
	const PDF = {
		id: 'att_1',
		file: 'abcdef0123456789.pdf',
		name: 'report.pdf',
		mime: 'application/pdf',
		bytes: 1000,
	}

	/** Adds attachment responders on top of whatever `openSettings` installed. */
	function answerAttach(command: string, result: unknown) {
		const prior = mocks.invoke.getMockImplementation()
		mocks.invoke.mockImplementation(async (name, args) => {
			if (name === command) return result
			return prior?.(name, args)
		})
	}

	beforeEach(() => {
		useView().showSettings()
	})

	afterEach(() => {
		// Both are module-scope and would otherwise leak into the next test: the
		// view ref stays wherever the last test drove it, and the pending tray
		// keeps what these ingests added.
		useView().showList()
		useAttachments().clearPending()
	})

	it('accepts an OS file drop and returns to the list', async () => {
		await openSettings()
		answerAttach('attach_paths', [PDF])
		expect(mocks.dragDrop.deliver, 'DropTarget registered no drag listener').not.toBeNull()

		await mocks.dragDrop.deliver?.({
			payload: { type: 'drop', paths: ['C:\\reports\\report.pdf'] },
		})
		await flush()

		expect(mocks.invoke).toHaveBeenCalledWith('attach_paths', {
			paths: ['C:\\reports\\report.pdf'],
		})
		expect(useView().view.value).toBe('list')
		expect(useAttachments().pending.value).toHaveLength(1)
	})

	it('accepts a pasted file and returns to the list', async () => {
		await openSettings()
		answerAttach('attach_paste', [PDF])

		document.dispatchEvent(new Event('paste', { bubbles: true }))
		await flush()

		expect(mocks.invoke).toHaveBeenCalledWith('attach_paste')
		expect(useView().view.value).toBe('list')
		expect(useAttachments().pending.value).toHaveLength(1)
	})

	/** Rust answers an empty list for a clipboard carrying text or nothing, and
	 *  that is the paste this view must leave alone — there is no composer here
	 *  for a capture to land in, so nothing may change. */
	it('leaves a text paste alone and stays put', async () => {
		await openSettings()
		answerAttach('attach_paste', [])

		document.dispatchEvent(new Event('paste', { bubbles: true }))
		await flush()

		expect(useView().view.value).toBe('settings')
		expect(useAttachments().pending.value).toHaveLength(0)
	})
})

/**
 * The Share setup guide: a disclosure under the enable switch, and the one
 * clipboard write in this view.
 *
 * The load-bearing facts are that it is reachable **before** Share is turned on —
 * it explains the values the switch reveals rows for — and that the prompt it
 * copies still carries the four things an assistant cannot be allowed to guess:
 * where the Worker lives, which runner to use, that the token is a quota guard,
 * and that the two machines take different roles.
 */
describe('the share setup guide', () => {
	/** The last text written through `clipboard_write_text`, or null. */
	function copiedText() {
		const written = mocks.invoke.mock.calls.filter((call) => call[0] === 'clipboard_write_text')
		const last = written.at(-1)
		return last ? (last[1] as { text: string }).text : null
	}

	/** Available with Share switched off, which is where a reader who has not set
	 *  anything up yet actually stands. Everything else in the section is behind
	 *  the switch. */
	it('offers the guide while share is off, and opens and closes it', async () => {
		const wrapper = await openSettings()
		const toggle = wrapper.get('[data-testid="share-guide-toggle"]')

		expect(wrapper.get('#share-enabled').attributes('aria-checked')).toBe('false')
		expect(toggle.element.tagName).toBe('BUTTON')
		expect(toggle.attributes('aria-expanded')).toBe('false')
		expect(wrapper.find('[data-testid="share-setup-guide"]').exists()).toBe(false)

		await toggle.trigger('click')
		expect(wrapper.get('[data-testid="share-guide-toggle"]').attributes('aria-expanded')).toBe(
			'true',
		)
		expect(wrapper.find('[data-testid="share-setup-guide"]').exists()).toBe(true)

		await wrapper.get('[data-testid="share-guide-toggle"]').trigger('click')
		expect(wrapper.get('[data-testid="share-guide-toggle"]').attributes('aria-expanded')).toBe(
			'false',
		)
		expect(wrapper.find('[data-testid="share-setup-guide"]').exists()).toBe(false)
	})

	/** The guide's own copy, not the prompt's: a reader who never presses **Copy
	 *  prompt** still has to be told which runner to use inside a checkout, and
	 *  that the two machines differ. */
	it('names the wrangler commands, the npx caveat and the two roles', async () => {
		const wrapper = await openSettings()
		await wrapper.get('[data-testid="share-guide-toggle"]').trigger('click')

		const guide = wrapper.get('[data-testid="share-setup-guide"]').text()
		expect(guide).toContain('pnpm dlx wrangler@4 login')
		expect(guide).toContain('pnpm dlx wrangler@4 kv namespace create MAILBOX')
		expect(guide).toContain('pnpm dlx wrangler@4 secret put RELAY_TOKEN')
		expect(guide).toContain('pnpm dlx wrangler@4 deploy')
		expect(guide).toContain('npx')
		expect(guide).toContain('First')
		expect(guide).toContain('Second')
		// The generic form, never a real subdomain: the guide is read by people who
		// would otherwise paste someone else's relay address into their own panel.
		expect(guide).toContain('https://copper-relay.<your-subdomain>.workers.dev')
	})

	/** Both links leave for the user's browser. A real anchor would navigate the
	 *  WebView and take the panel with it. */
	it('opens the sign-up page and the repository in the browser', async () => {
		const wrapper = await openSettings()
		await wrapper.get('[data-testid="share-guide-toggle"]').trigger('click')

		const links = wrapper.get('[data-testid="share-setup-guide"]').findAll('button')
		const signUp = links.find((link) => link.text().includes('Cloudflare sign-up'))
		const repository = links.find((link) => link.text().includes('Copper repository'))

		await signUp?.trigger('click')
		await repository?.trigger('click')

		expect(mocks.openUrl).toHaveBeenCalledWith('https://dash.cloudflare.com/sign-up')
		expect(mocks.openUrl).toHaveBeenCalledWith('https://github.com/FallDownTheSystem/copper')
	})

	it('puts the hand-off prompt on the clipboard and says so', async () => {
		const wrapper = await openSettings()
		await wrapper.get('[data-testid="share-guide-toggle"]').trigger('click')

		await wrapper.get('[data-testid="share-copy-prompt"]').trigger('click')
		await flush()

		// The real constant reached the real clipboard adapter, unaltered.
		expect(copiedText()).toBe(SHARE_SETUP_PROMPT)

		const guide = wrapper.get('[data-testid="share-setup-guide"]').text()
		expect(guide).toContain('The prompt is on your clipboard.')
	})

	/**
	 * The prompt's own content, asserted against the constant rather than against
	 * the rendered guide: it is the half that leaves the app, and an assistant
	 * reading it has none of the surrounding UI to fall back on.
	 */
	it('carries the facts the assistant cannot infer', () => {
		expect(SHARE_SETUP_PROMPT).toContain('https://github.com/FallDownTheSystem/copper')
		expect(SHARE_SETUP_PROMPT).toContain('pnpm dlx wrangler@4 login')
		expect(SHARE_SETUP_PROMPT).toContain('pnpm dlx wrangler@4 kv namespace create MAILBOX')
		expect(SHARE_SETUP_PROMPT).toContain('pnpm dlx wrangler@4 secret put RELAY_TOKEN')
		expect(SHARE_SETUP_PROMPT).toContain('pnpm dlx wrangler@4 deploy')
		// The runner caveat, both halves: pnpm dlx inside a checkout, npx outside.
		expect(SHARE_SETUP_PROMPT).toContain('EBADDEVENGINES')
		expect(SHARE_SETUP_PROMPT).toContain('npx wrangler@4 works too')
		// What the token is and is not.
		expect(SHARE_SETUP_PROMPT).toContain('quota guard, not a confidentiality control')
		// The boundary the assistant must not cross, and the field it must warn about.
		expect(SHARE_SETUP_PROMPT).toContain('Never invent one')
		expect(SHARE_SETUP_PROMPT).toContain('First on one machine and Second on the other')
	})

	/** A refused clipboard write is the one message here that must not be
	 *  mistaken for a success: the reader is about to paste nothing into an
	 *  assistant. */
	it('reports a refused clipboard write instead of claiming a copy', async () => {
		const wrapper = await openSettings()
		await wrapper.get('[data-testid="share-guide-toggle"]').trigger('click')

		// `useSystemClipboard` logs the rejection it swallows; the suite's output is
		// not the place to read it.
		vi.spyOn(console, 'error').mockImplementation(() => {})
		const prior = mocks.invoke.getMockImplementation()
		mocks.invoke.mockImplementation(async (name: string, args?: Record<string, unknown>) => {
			if (name === 'clipboard_write_text') throw { kind: 'invalid', message: 'refused' }
			return prior?.(name, args)
		})

		await wrapper.get('[data-testid="share-copy-prompt"]').trigger('click')
		await flush()

		const guide = wrapper.get('[data-testid="share-setup-guide"]').text()
		expect(guide).toContain("Couldn't write to the clipboard.")
		expect(guide).not.toContain('The prompt is on your clipboard.')
	})
})
