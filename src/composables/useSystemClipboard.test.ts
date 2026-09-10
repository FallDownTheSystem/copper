import { beforeEach, expect, it, vi } from 'vite-plus/test'

import { useSystemClipboard } from './useSystemClipboard'

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))

const clipboard = useSystemClipboard()
const targets = [{ note: 'note', attachment: 'image' }]
const source = { path: 'C:\\notes.copper', id: 'space' }

beforeEach(() => mocks.invoke.mockReset())

it('refuses concurrent copies instead of saving gestures for later replay', async () => {
	let finish!: (value: unknown) => void
	mocks.invoke.mockImplementation((command: string) =>
		command === 'clipboard_copy_attachments'
			? new Promise((resolve) => {
					finish = resolve
				})
			: Promise.resolve(),
	)
	const image = clipboard.copyAttachments(targets, 'auto', source)
	const text = clipboard.writeText('the newest copy')
	await Promise.resolve()
	expect(clipboard.copying.value).toBe(true)
	expect(mocks.invoke.mock.calls).toEqual([
		['clipboard_copy_attachments', { targets, format: 'auto', source }],
	])
	expect(await text).toBe(false)
	finish({ count: 1, format: 'image' })
	await image
	expect(await clipboard.writeText('retry after completion')).toBe(true)
	expect(mocks.invoke.mock.calls[1]).toEqual([
		'clipboard_write_text',
		{ text: 'retry after completion' },
	])
	expect(clipboard.copying.value).toBe(false)
})

it('permits a retry after a failed copy and never adds text to a native attachment copy', async () => {
	mocks.invoke.mockRejectedValueOnce(new Error('missing image')).mockResolvedValueOnce(undefined)
	const image = clipboard.copyAttachments(targets, 'image', source)
	const failed = expect(image).rejects.toThrow('missing image')
	await failed
	expect(await clipboard.writeText('retry as text')).toBe(true)
	expect(mocks.invoke).toHaveBeenCalledTimes(2)
	expect(clipboard.copying.value).toBe(false)
})

it('does not replay an older copy after another application changes the clipboard', async () => {
	let sequence = 1
	let contents = 'previous clipboard'
	let finish!: () => void
	let calls = 0
	mocks.invoke.mockImplementation(() => {
		const expected = sequence
		if (++calls === 1) {
			return new Promise((_resolve, reject) => {
				finish = () => {
					if (sequence !== expected) reject(new Error('clipboard changed'))
				}
			})
		}
		contents = 'older attachment copy'
		return Promise.resolve({ count: 1, format: 'image' })
	})
	const first = clipboard.copyAttachments(targets, 'image', source).catch((error) => error)
	await Promise.resolve()
	const repeated = clipboard.copyAttachments(targets, 'image', source).catch((error) => error)
	sequence++
	contents = 'newer external text'
	finish()
	await first
	await repeated
	expect(contents).toBe('newer external text')
	expect(mocks.invoke).toHaveBeenCalledTimes(1)
})

it('sends the originating space with note-reference copies', async () => {
	mocks.invoke.mockResolvedValue({ text: 'note and references', count: 1, ids: ['note'] })
	await clipboard.copyNotesWithAttachments({ kind: 'ids', ids: ['note'] }, source)
	expect(mocks.invoke).toHaveBeenCalledWith('clipboard_copy_notes', {
		selection: { kind: 'ids', ids: ['note'] },
		source,
	})
})
