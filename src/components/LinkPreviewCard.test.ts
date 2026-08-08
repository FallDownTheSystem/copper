import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import NoteBody from './NoteBody.vue'
import LinkPreviewCard from './LinkPreviewCard.vue'
import type { Note, Settings } from '@/composables/useSpace'
import { useSettings } from '@/composables/useSettings'
import { usePreviews } from '@/composables/usePreviews'

/**
 * Task-020, asserted through `NoteBody` rather than through the card alone,
 * because the two halves that matter are both about the *mount*: which links are
 * asked about, and whether anything is asked about at all.
 *
 * The assertions here are mostly negative — no request, no card, no second
 * fetch — and that is the shape of the feature. A link preview is a disclosure
 * to a third party, so almost everything worth testing is a case where Copper
 * must not make one.
 */

const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
	openUrl: vi.fn<(url: string) => Promise<void>>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
	emit: vi.fn(),
	listen: async () => () => {},
}))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: mocks.openUrl }))

const CARD = {
	url: 'https://a.example/one',
	siteName: 'Example',
	title: 'A title',
	description: 'A description',
	image: null,
}

function makeSettings(linkPreviews: boolean): Settings {
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
		linkPreviews,
	}
}

function makeNote(body: string, id = 'nte_1'): Note {
	return {
		id,
		section: 'sec_1',
		order: 0,
		done: false,
		body,
		attachments: [],
		created: '2026-08-08T09:00:00Z',
		updated: '2026-08-08T09:00:00Z',
	}
}

async function flush(times = 6) {
	for (let i = 0; i < times; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

/** The composables here are module-scoped singletons and `vi.resetModules()`
 *  cannot re-evaluate a module already imported, so each case re-pulls settings
 *  through the same path the app does and lets the pull overwrite the value. */
async function withPreviews(enabled: boolean) {
	mocks.invoke.mockImplementation(async (command: string) => {
		switch (command) {
			case 'get_settings':
				return makeSettings(enabled)
			case 'get_shortcut_state':
				return {
					capture: 'Shift Shift',
					summon: 'Ctrl+Shift+Space',
					defaults: { capture: 'Shift Shift', summon: 'Ctrl+Shift+Space' },
					summonRegistered: true,
					summonError: null,
					captureRegistered: true,
					captureError: null,
					captureFallback: null,
				}
			case 'get_autostart_enabled':
				return false
			case 'link_preview':
				return CARD
			case 'preview_image':
				return new ArrayBuffer(0)
			default:
				throw { kind: 'invalid', message: `no responder: ${command}` }
		}
	})
	await useSettings().refresh()
	// The other precondition of a fetch, and it is a separate one: a request may
	// not be issued while the panel window is not on screen, which is how it starts
	// life. `NoteList` answers that question in the app and is not mounted here, so
	// the cases below stand the panel up themselves — except the one asserting the
	// gate, which puts it back down.
	usePreviews().setPanelVisible(true)
	await flush()
}

function previewCalls() {
	return mocks.invoke.mock.calls.filter((call) => call[0] === 'link_preview')
}

beforeEach(() => {
	mocks.invoke.mockReset()
	mocks.openUrl.mockReset()
})

describe('when link previews are switched off', () => {
	/** AC-7's frontend half. Rust refuses independently and that is the guarantee
	 *  — but a command that is never issued is one fewer thing depending on the
	 *  guarantee holding. */
	it('asks for nothing at all', async () => {
		await withPreviews(false)

		mount(NoteBody, { props: { note: makeNote('see https://a.example/one') } })
		await flush()

		expect(previewCalls()).toEqual([])
	})

	it('renders no card', async () => {
		await withPreviews(false)

		const wrapper = mount(NoteBody, { props: { note: makeNote('see https://a.example/one') } })
		await flush()

		expect(wrapper.findComponent(LinkPreviewCard).exists()).toBe(false)
	})
})

describe('when link previews are switched on', () => {
	it('asks once per distinct link and shows what came back', async () => {
		await withPreviews(true)

		const wrapper = mount(NoteBody, {
			props: { note: makeNote('see https://a.example/one and https://a.example/one again') },
		})
		await flush()

		// Twice in the body, one request: each fetch is a separate disclosure, so
		// de-duplication is a privacy property rather than an optimisation.
		expect(previewCalls()).toEqual([['link_preview', { url: 'https://a.example/one' }]])
		expect(wrapper.text()).toContain('A title')
		expect(wrapper.text()).toContain('A description')
		expect(wrapper.text()).toContain('Example')
	})

	/** The same URL in a second note is the same page. Asking again would double
	 *  the disclosure for a reader who happened to file one link twice. */
	it('does not ask again for a link another note already resolved', async () => {
		await withPreviews(true)

		mount(NoteBody, { props: { note: makeNote('https://a.example/one', 'nte_a') } })
		await flush()
		const first = previewCalls().length

		mount(NoteBody, { props: { note: makeNote('https://a.example/one', 'nte_b') } })
		await flush()

		expect(previewCalls().length).toBe(first)
	})

	/** AC-6. Every failure is the same outcome as a page with no metadata: the
	 *  link renders exactly as it did, and nothing is said about it anywhere. */
	it('renders the plain link and no message when the fetch answers with nothing', async () => {
		await withPreviews(true)
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'link_preview') return null
			throw { kind: 'invalid', message: `no responder: ${command}` }
		})

		const wrapper = mount(NoteBody, { props: { note: makeNote('https://b.example/none') } })
		await flush()

		expect(wrapper.findComponent(LinkPreviewCard).exists()).toBe(false)
		expect(wrapper.find('a[href="https://b.example/none"]').exists()).toBe(true)
	})

	it('says nothing when the command itself rejects', async () => {
		await withPreviews(true)
		mocks.invoke.mockImplementation(async (command: string) => {
			if (command === 'link_preview') throw { kind: 'io', message: 'the socket closed' }
			throw { kind: 'invalid', message: `no responder: ${command}` }
		})

		const wrapper = mount(NoteBody, { props: { note: makeNote('https://c.example/boom') } })
		await flush()

		expect(wrapper.findComponent(LinkPreviewCard).exists()).toBe(false)
		expect(wrapper.text()).not.toContain('socket')
	})

	/** AC-8. The body is rendered and interactive before any card exists — a card
	 *  is never in the first paint, so nothing about a fetch can delay the note. */
	it('renders the body before the card arrives', async () => {
		await withPreviews(true)

		// A URL no earlier case resolved. One already in the cache renders its card
		// on the first tick, which is correct behaviour and would make this assert
		// nothing.
		const wrapper = mount(NoteBody, { props: { note: makeNote('see https://fresh.example/x') } })

		expect(wrapper.find('.note-prose').text()).toContain('see')
		expect(wrapper.findComponent(LinkPreviewCard).exists()).toBe(false)
		await flush()
		expect(wrapper.findComponent(LinkPreviewCard).exists()).toBe(true)
	})

	/** AC-10. The card is a gateway to the page, not a picture to inspect: it
	 *  opens the URL through the same `openUrl` the prose link uses. */
	it('opens the page rather than a viewer when the card is clicked', async () => {
		await withPreviews(true)

		const wrapper = mount(NoteBody, { props: { note: makeNote('https://a.example/one') } })
		await flush()

		await wrapper.findComponent(LinkPreviewCard).get('button').trigger('click')

		expect(mocks.openUrl).toHaveBeenCalledWith('https://a.example/one')
	})

	/** The card is a second route to a link that is already on screen, so it must
	 *  not add a Tab stop — the grid's one-Tab-stop contract is what every anchor
	 *  in the prose above is held to as well. */
	it('stays out of the tab order, like every anchor in the prose', async () => {
		await withPreviews(true)

		const wrapper = mount(NoteBody, { props: { note: makeNote('https://a.example/one') } })
		await flush()

		expect(wrapper.findComponent(LinkPreviewCard).get('button').attributes('tabindex')).toBe('-1')
	})

	/**
	 * The panel window is mounted hidden at launch and stays that way until the
	 * user summons it. Fetching then would contact every host named anywhere in the
	 * space at the one moment nobody could have asked for it — so the request is
	 * held, and released when the panel appears.
	 */
	it('issues nothing while the panel is hidden and flushes when it is shown', async () => {
		await withPreviews(true)
		const { setPanelVisible } = usePreviews()
		setPanelVisible(false)

		mount(NoteBody, { props: { note: makeNote('https://hidden.example/x', 'nte_hidden') } })
		await flush()
		expect(previewCalls()).toEqual([])

		setPanelVisible(true)
		await flush()

		// Held, not dropped: nothing re-renders the note, so a dropped request would
		// be a link that never resolves for the life of the session.
		expect(previewCalls()).toEqual([['link_preview', { url: 'https://hidden.example/x' }]])
	})

	/**
	 * Rust keys its cache on the URL with the fragment dropped, so two links
	 * differing only by `#section` are one entry there. Dedup on the raw href made
	 * them two requests here — two disclosures, racing to write the same file.
	 */
	it('treats two fragment variants of one URL as one page', async () => {
		await withPreviews(true)

		const wrapper = mount(NoteBody, {
			props: {
				note: makeNote('https://frag.example/p#one and https://frag.example/p#two', 'nte_fragment'),
			},
		})
		await flush()

		expect(previewCalls()).toEqual([['link_preview', { url: 'https://frag.example/p' }]])
		// And both links read the one answer, rather than one card and one link that
		// never resolves.
		expect(wrapper.findAllComponents(LinkPreviewCard).length).toBe(2)
	})

	/** A URL the renderer refuses to make a link of is not a link, so no request
	 *  is made for it — the gate is the token stream, not the text. */
	it('asks for nothing when the only URL in the note is inside code', async () => {
		await withPreviews(true)

		mount(NoteBody, { props: { note: makeNote('`https://d.example/fenced`', 'nte_code') } })
		await flush()

		expect(previewCalls()).toEqual([])
	})
})
