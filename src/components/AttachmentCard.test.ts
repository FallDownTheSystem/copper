import { mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import AttachmentCard from './AttachmentCard.vue'
import { useAttachments, type Attachment } from '@/composables/useAttachments'
import { useOverlayHost } from '@/composables/useOverlayHost'

/**
 * The card's own context menu, and the one entry on it.
 *
 * Two things here are worth pinning and the rest is not. The entry has to reach
 * `attachment_reveal` rather than `attachment_open`, because the two differ
 * exactly in that the second one may *launch* what it is given. And it has to
 * carry the content-addressed `file` — the argument Rust rebuilds the path
 * from — where the attachment's `id` would be the plausible mistake: both fields
 * are strings, so nothing but a test can tell them apart.
 *
 * Statically imported, like the component: the composables behind it hold
 * module-scoped state by design, and a dynamic import would hand this file a
 * second instance of it.
 */

const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))

const attachments = useAttachments()
const { setOverlayHost } = useOverlayHost()

/** A `.pdf`, so the card is available without being viewable — the plain case.
 *  Its `id` and its `file` are deliberately nothing like each other. */
const PDF: Attachment = {
	id: 'att_1',
	file: 'b1946ac92492d2347c6235b4d2611184.pdf',
	name: 'brief.pdf',
	mime: 'application/pdf',
	bytes: 2048,
}

let wrapper: ReturnType<typeof mount> | null = null
let host: HTMLElement | null = null

async function settle(turns = 3) {
	for (let i = 0; i < turns; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

beforeEach(() => {
	// The preview cache is module state and outlives a mount, so without this the
	// second case's request is deduped against the first case's answer.
	attachments.clearPreviews()
	mocks.invoke.mockReset()
	// An empty answer is Rust's "the file is there and has nothing to show",
	// which is what a `.pdf` gets.
	mocks.invoke.mockImplementation(async (command) =>
		command === 'attachment_thumb' ? new ArrayBuffer(0) : null,
	)

	// The menu is portalled, and it renders only once a host exists — `PanelShell`
	// publishes one in the running app, and there is no shell here.
	host = document.createElement('div')
	document.body.append(host)
	setOverlayHost(host, host)
})

afterEach(() => {
	wrapper?.unmount()
	wrapper = null
	setOverlayHost(null, null)
	host?.remove()
	host = null
})

async function openCardMenu() {
	wrapper = mount(AttachmentCard, {
		attachTo: document.body,
		props: { attachment: PDF, tabIndex: 0 },
	})
	await settle()

	await wrapper.get('button').trigger('contextmenu')
	await settle()

	const content = document.querySelector<HTMLElement>('[data-slot="context-menu-content"]')
	expect(content, 'the attachment context menu did not open').not.toBeNull()
	return content!
}

describe('the attachment context menu', () => {
	it('offers the one entry, named for the location rather than the file', async () => {
		const content = await openCardMenu()

		const items = [...content.querySelectorAll<HTMLElement>('[role="menuitem"]')]
		expect(items).toHaveLength(1)
		expect(items[0]?.textContent).toContain('Open attachment location')
	})

	it('reveals through attachment_reveal, carrying the stored name and not the id', async () => {
		const content = await openCardMenu()

		content.querySelector<HTMLElement>('[role="menuitem"]')?.click()
		await settle()

		expect(mocks.invoke).toHaveBeenCalledWith('attachment_reveal', { file: PDF.file })
		// The other half of the pair stays out of it. It is the arm that launches.
		expect(mocks.invoke).not.toHaveBeenCalledWith('attachment_open', expect.anything())
	})
})
