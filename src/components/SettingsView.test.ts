import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import SettingsView from './SettingsView.vue'
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
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
	emit: vi.fn(),
	listen: async () => () => {},
}))

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

async function openSettings(stored: Partial<Settings> = {}) {
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
		expect(segment(wrapper, 'Where new notes go', 'Top').find('[aria-hidden="true"]').exists()).toBe(
			true,
		)
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

		for (const promised of ['fetch', 'IP address', 'when you read']) {
			expect(text).toContain(promised)
		}
	})

	/** Its own section, and not a row under one of the behavioural ones: it is
	 *  the only setting here that is not about how the panel behaves. */
	it('lives in a section of its own called Privacy', async () => {
		const wrapper = await openSettings()
		expect(wrapper.text()).toContain('Privacy')
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
