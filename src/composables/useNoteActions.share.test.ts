import { beforeEach, describe, expect, it, vi } from 'vite-plus/test'

/**
 * `sendToOtherDevice`'s outcome switch — all six branches.
 *
 * Every one of them is a sentence a person reads at the moment a note either did
 * or did not leave their machine, and three of them are easy to get subtly wrong
 * in a way no other test would catch:
 *
 * - `delayed` is a **success**. The relay stored the note and only failed to
 *   announce it; reporting that as a failure would send the user into a resend
 *   that duplicates the note.
 * - `unknown` is neither success nor failure, and its message has to say so —
 *   the note may have arrived, so "try again" would be exactly the wrong advice.
 * - `too-large` has to name the **attachment** budget, not the ciphertext sizes
 *   Rust measured. "Too large" alone leaves the reader with nothing to act on,
 *   and the raw numbers leave them with arithmetic instead.
 *
 * A separate file from a hypothetical `useNoteActions.test.ts` because the
 * composables under it hold module-scoped state by design: mocking `invoke` for
 * this one switch in the same worker as a broader suite would leak.
 */

const mocks = vi.hoisted(() => ({
	invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
	listen: vi.fn(async () => () => {}),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen, emit: vi.fn() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))
vi.mock('@tauri-apps/api/webview', () => ({
	getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}))

import { toast as sonner } from 'vue-sonner'

import { useNoteActions } from './useNoteActions'
import { noteRow, useSelection } from './useSelection'
import { type Space } from './useSpace'
import { useStatusMessage } from './useStatusMessage'

const SPACE: Space = {
	id: 'spc_1',
	name: 'development',
	activeSection: 'sec_a',
	sections: [{ id: 'sec_a', name: 'Research', order: 0 }],
	notes: [
		{
			id: 'n1',
			section: 'sec_a',
			order: 0,
			done: false,
			body: 'first',
			created: '2026-08-09T00:00:00Z',
			updated: '2026-08-09T00:00:00Z',
		},
		{
			id: 'n2',
			section: 'sec_a',
			order: 1,
			done: false,
			body: 'second',
			created: '2026-08-09T00:00:00Z',
			updated: '2026-08-09T00:00:00Z',
		},
	],
}

const actions = useNoteActions()
const selection = useSelection()
const status = useStatusMessage()

/** What `share_send_notes` answers this case. */
let outcome: unknown = { kind: 'sent', notes: 1 }

beforeEach(() => {
	mocks.invoke.mockReset()
	mocks.invoke.mockImplementation(async (command) => {
		if (command === 'share_send_notes') return outcome
		return null
	})

	status.clear()
	selection.resetForNewSpace()
	// `targetIds()` is resolved entirely out of `useSelection` — the row order and
	// the focused row — so the document only has to be known here, not loaded into
	// `useSpace`.
	selection.syncDocument(SPACE)
	// One note focused and nothing selected, so `targetIds()` is exactly it.
	selection.focusRow(noteRow('n1'))
})

/** Reads the outcome back off Sonner's own state rather than a DOM: the toast
 *  store is a module singleton `useStatusMessage` writes through, and no
 *  `<Toaster>` is mounted in a composable suite. The last active toast is the
 *  one the send just wrote, reshaped to the seam's vocabulary. */
async function send(): Promise<{ text: string; severity: 'info' | 'error' } | null> {
	await actions.sendToOtherDevice()
	const latest = sonner.getToasts().at(-1)
	if (!latest || !('title' in latest)) return null
	return {
		text: typeof latest.title === 'string' ? latest.title : '',
		severity: latest.type === 'error' ? 'error' : 'info',
	}
}

describe('sendToOtherDevice reports every outcome', () => {
	it('names the count on success', async () => {
		outcome = { kind: 'sent', notes: 1 }
		let toast = await send()
		expect(toast?.severity).toBe('info')
		expect(toast?.text).toBe('Sent 1 note to your other device')

		outcome = { kind: 'sent', notes: 3 }
		toast = await send()
		expect(toast?.text).toBe('Sent 3 notes to your other device')
	})

	/**
	 * A success, not a failure: the relay has the note. But it is **not** "on its
	 * way" — the head pointer never moved, and the reader only walks up to that
	 * pointer, so this note is collected when the next send moves it past. The
	 * message has to say that rather than promise a time.
	 */
	it('treats delayed as a success and says what actually collects it', async () => {
		outcome = { kind: 'delayed', notes: 1 }
		const toast = await send()

		expect(toast?.severity).toBe('info')
		expect(toast?.text).toContain('The relay has it')
		expect(toast?.text).toContain('a later send')
		// Neither wording that promises a time this protocol cannot keep: the
		// announcing send can fail to announce itself as well.
		expect(toast?.text).not.toContain('shortly')
		expect(toast?.text).not.toContain('the next one')
	})

	/** The one message that must not invite a retry. */
	it('warns that an unknown outcome may already have been delivered', async () => {
		outcome = { kind: 'unknown', message: 'the relay did not answer in time' }
		const toast = await send()

		expect(toast?.severity).toBe('error')
		expect(toast?.text).toContain('may have arrived')
		expect(toast?.text).toContain('twice')
		expect(toast?.text).toContain('the relay did not answer in time')
	})

	it('gives one actionable number when the payload is over the cap', async () => {
		outcome = { kind: 'too-large', bytes: 22 * 1024 * 1024, limit: 20 * 1024 * 1024 }
		const toast = await send()

		expect(toast?.severity).toBe('error')
		expect(toast?.text).toContain('14 MB')
		// Neither of Rust's two numbers reaches the reader, and neither does the
		// conversion between them. Both are measured after encryption, so neither is
		// a size the reader has ever seen on disk — quoting them asks for a
		// multiplication and answers nothing.
		expect(toast?.text).not.toContain('22.0 MB')
		expect(toast?.text).not.toContain('20.0 MB')
		expect(toast?.text).not.toContain('a third')
	})

	it('names the missing field and points at Settings', async () => {
		outcome = { kind: 'unconfigured', missing: 'pairing secret' }
		const toast = await send()

		expect(toast?.severity).toBe('error')
		expect(toast?.text).toContain('pairing secret')
		expect(toast?.text).toContain('Settings')
	})

	it("passes Rust's own sentence through on a plain failure", async () => {
		outcome = { kind: 'failed', message: 'no space is open' }
		const toast = await send()

		expect(toast?.severity).toBe('error')
		expect(toast?.text).toBe('no space is open')
	})

	/** A rejected invoke is normalised into `failed` by `useDeviceShare`, so this
	 *  switch never has a branch with no message. */
	it('reports a rejected command rather than throwing', async () => {
		mocks.invoke.mockImplementation(async (command) => {
			if (command === 'share_send_notes') throw { kind: 'invalid', message: 'no notes' }
			return null
		})

		const toast = await send()
		expect(toast?.severity).toBe('error')
		expect(toast?.text).toContain('no notes')
	})

	it('sends the focused note and does nothing with no target', async () => {
		outcome = { kind: 'sent', notes: 1 }
		await send()
		expect(mocks.invoke).toHaveBeenCalledWith('share_send_notes', { ids: ['n1'] })

		mocks.invoke.mockClear()
		selection.resetForNewSpace()
		await actions.sendToOtherDevice()
		expect(mocks.invoke).not.toHaveBeenCalledWith('share_send_notes', expect.anything())
	})
})
