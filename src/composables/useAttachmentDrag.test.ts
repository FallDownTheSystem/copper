import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import { useAttachmentDrag } from './useAttachmentDrag'
import { useAttachmentSelection } from './useAttachmentSelection'
import { useNoteList } from './useNoteList'
import { useNoteSearch } from './useNoteSearch'
import { useSections } from './useSections'
import { useSelection } from './useSelection'
import { useSpace, type Space } from './useSpace'
import { useStatusMessage } from './useStatusMessage'

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))

const space = useSpace()
const selection = useAttachmentSelection()
const drag = useAttachmentDrag()
const first = { note: 'note-a', attachment: 'a-1' }
const second = { note: 'note-a', attachment: 'a-2' }
const third = { note: 'note-b', attachment: 'b-1' }
const SOURCE = { path: 'C:\\synthetic-fixtures\\drag.copper', id: 'drag-space' }

function fixture(): Space {
	return {
		id: SOURCE.id,
		name: 'Synthetic drag',
		activeSection: 'section',
		sections: [{ id: 'section', name: 'Notes', order: 0 }],
		notes: ['note-a', 'note-b'].map((id, order) => ({
			id,
			section: 'section',
			order,
			body: id,
			done: false,
			created: '',
			updated: '',
			attachments: (order === 0 ? [first, second] : [third]).map((target) => ({
				id: target.attachment,
				file: `${target.attachment}.pdf`,
				name: `${target.attachment}.pdf`,
				mime: 'application/pdf',
				bytes: 1,
			})),
		})),
	}
}

let currentDocument: Space
let currentPath: string | null
let prepare: () => Promise<string>
let nativeDrag: () => Promise<boolean>
let discard: () => Promise<void>
const cleanups: (() => void)[] = []
const jobs: Promise<unknown>[] = []

function deferred<T>(fallback: T) {
	let resolve!: (value: T) => void
	let reject!: (error: Error) => void
	const promise = new Promise<T>((resolvePromise, rejectPromise) => {
		resolve = resolvePromise
		reject = rejectPromise
	})
	cleanups.push(() => resolve(fallback))
	return { promise, resolve, reject }
}

function start(target = first, signal = new AbortController().signal) {
	const job = drag.start(target, signal)
	jobs.push(job)
	return job
}

function dragCalls() {
	return mocks.invoke.mock.calls.filter(([command]) => command.startsWith('attachment_'))
}

beforeEach(async () => {
	mocks.invoke.mockReset()
	currentDocument = fixture()
	currentPath = SOURCE.path
	prepare = async () => 'drag-ticket'
	nativeDrag = async () => true
	discard = async () => {}
	mocks.invoke.mockImplementation(async (command: string) => {
		if (command === 'get_active_space') return structuredClone(currentDocument)
		if (command === 'get_status') {
			return {
				path: currentPath,
				errored: false,
				watching: true,
				canUndo: false,
				canRedo: false,
				startupNotice: null,
			}
		}
		if (command === 'get_settings') {
			return { doneFilter: 'all', sortMode: 'manual', doneOnCopy: true }
		}
		if (command === 'attachment_prepare_drag') return prepare()
		if (command === 'attachment_start_drag') return nativeDrag()
		if (command === 'attachment_discard_drag') return discard()
		throw new Error(`Unexpected native command: ${command}`)
	})
	selection.syncDocument(null)
	useSelection().resetForNewSpace()
	useSections().reset()
	useNoteSearch().clearQuery()
	useNoteList().hydrate(null)
	await space.load()
	mocks.invoke.mockClear()
})

afterEach(async () => {
	for (const cleanup of cleanups.splice(0)) cleanup()
	await Promise.all(jobs.splice(0))
	selection.clear()
	useStatusMessage().clear()
	expect(drag.dragging.value).toBe(false)
	expect(space.space.value?.notes.map((note) => note.done)).toEqual([false, false])
	expect(
		mocks.invoke.mock.calls.filter(([command]) => /clipboard|copy|done/.test(command)),
	).toEqual([])
})

describe('native attachment drag adapter', () => {
	it.each([true, false])(
		'prepares targets and source before starting a ticket with native result %s',
		async (result) => {
			const pending = deferred('drag-ticket')
			prepare = () => pending.promise
			nativeDrag = async () => result
			selection.select(third)
			selection.toggle(first)
			const controller = new AbortController()
			const job = start(first, controller.signal)
			expect(drag.dragging.value).toBe(true)
			expect(dragCalls()).toEqual([
				['attachment_prepare_drag', { targets: [first, third], source: SOURCE }],
			])
			pending.resolve('prepared-id')
			await job
			controller.abort()
			expect(dragCalls()).toEqual([
				['attachment_prepare_drag', { targets: [first, third], source: SOURCE }],
				['attachment_start_drag', { id: 'prepared-id' }],
			])
			expect(selection.selected.value).toEqual([first, third])
			expect(drag.dragging.value).toBe(false)
		},
	)

	it('chooses an unselected target without dragging a stale selection', async () => {
		selection.select(first)
		selection.toggle(second)
		await start(third)
		expect(selection.selected.value).toEqual([third])
		expect(dragCalls()).toEqual([
			['attachment_prepare_drag', { targets: [third], source: SOURCE }],
			['attachment_start_drag', { id: 'drag-ticket' }],
		])
	})

	it('does not prepare an already released gesture or change its selection', async () => {
		selection.select(third)
		const controller = new AbortController()
		controller.abort()
		await start(first, controller.signal)
		expect(mocks.invoke).not.toHaveBeenCalled()
		expect(selection.selected.value).toEqual([third])
	})

	it('does not prepare without an originating document source', async () => {
		currentPath = null
		await space.load()
		mocks.invoke.mockClear()
		await start()
		expect(mocks.invoke).not.toHaveBeenCalled()
	})

	it('discards a ticket if the gesture is released while preparation is pending', async () => {
		const pending = deferred('drag-ticket')
		prepare = () => pending.promise
		const controller = new AbortController()
		const job = start(first, controller.signal)
		controller.abort()
		pending.resolve('released-id')
		await job
		expect(dragCalls()).toEqual([
			['attachment_prepare_drag', { targets: [first], source: SOURCE }],
			['attachment_discard_drag', { id: 'released-id' }],
		])
	})

	it('discards after leaving and returning to the same source because its epoch changed', async () => {
		const pending = deferred('drag-ticket')
		prepare = () => pending.promise
		const epoch = space.epoch.value
		const job = start()
		currentDocument = { ...fixture(), id: 'another-space' }
		await space.adopt(currentDocument)
		currentDocument = fixture()
		await space.adopt(currentDocument)
		expect(space.source.value).toEqual(SOURCE)
		expect(space.epoch.value).toBeGreaterThan(epoch)
		pending.resolve('stale-epoch-id')
		await job
		expect(dragCalls()).toEqual([
			['attachment_prepare_drag', { targets: [first], source: SOURCE }],
			['attachment_discard_drag', { id: 'stale-epoch-id' }],
		])
	})

	it('discards when the file path changes even if the document id and epoch are unchanged', async () => {
		const pending = deferred('drag-ticket')
		prepare = () => pending.promise
		const epoch = space.epoch.value
		const job = start()
		currentPath = 'C:\\synthetic-fixtures\\other-drag.copper'
		await space.adopt(currentDocument)
		expect(space.epoch.value).toBe(epoch)
		pending.resolve('stale-source-id')
		await job
		expect(dragCalls()).toEqual([
			['attachment_prepare_drag', { targets: [first], source: SOURCE }],
			['attachment_discard_drag', { id: 'stale-source-id' }],
		])
	})

	it('rejects concurrent gestures across adapter instances without queuing a replay', async () => {
		const pendingPreparation = deferred('drag-ticket')
		const pendingNative = deferred(true)
		prepare = () => pendingPreparation.promise
		nativeDrag = () => pendingNative.promise
		const job = start()
		await useAttachmentDrag().start(second, new AbortController().signal)
		expect(selection.selected.value).toEqual([first])
		expect(dragCalls()).toHaveLength(1)
		pendingPreparation.resolve('in-flight-id')
		await vi.waitFor(() => expect(dragCalls()).toHaveLength(2))
		await useAttachmentDrag().start(third, new AbortController().signal)
		expect(drag.dragging.value).toBe(true)
		expect(selection.selected.value).toEqual([first])
		pendingNative.resolve(true)
		await job
		expect(dragCalls()).toEqual([
			['attachment_prepare_drag', { targets: [first], source: SOURCE }],
			['attachment_start_drag', { id: 'in-flight-id' }],
		])
		await start(second)
		expect(dragCalls()).toHaveLength(4)
		expect(dragCalls()[2]).toEqual([
			'attachment_prepare_drag',
			{ targets: [second], source: SOURCE },
		])
	})

	it.each(['native', 'discard'])(
		'aborts a dispatched native drag immediately and stays busy when %s settles first',
		async (settlesFirst) => {
			const pendingNative = deferred(false)
			const pendingDiscard = deferred<void>(undefined)
			let nativeSettled = false
			let discardSettled = false
			nativeDrag = async () => {
				const result = await pendingNative.promise
				nativeSettled = true
				return result
			}
			discard = async () => {
				await pendingDiscard.promise
				discardSettled = true
			}
			const controller = new AbortController()
			const job = start(first, controller.signal)
			await vi.waitFor(() => expect(dragCalls()).toHaveLength(2))
			controller.abort()
			expect(dragCalls()).toEqual([
				['attachment_prepare_drag', { targets: [first], source: SOURCE }],
				['attachment_start_drag', { id: 'drag-ticket' }],
				['attachment_discard_drag', { id: 'drag-ticket' }],
			])
			await start(second)
			if (settlesFirst === 'native') {
				pendingNative.resolve(false)
				await vi.waitFor(() => expect(nativeSettled).toBe(true))
			} else {
				pendingDiscard.resolve()
				await vi.waitFor(() => expect(discardSettled).toBe(true))
			}
			expect(drag.dragging.value).toBe(true)
			await start(third)
			expect(dragCalls()).toHaveLength(3)
			pendingNative.resolve(false)
			pendingDiscard.resolve()
			await job
			expect(drag.dragging.value).toBe(false)
			expect(dragCalls()).toHaveLength(3)
			expect(selection.selected.value).toEqual([first])
			await start(second)
			expect(dragCalls().slice(3)).toEqual([
				['attachment_prepare_drag', { targets: [second], source: SOURCE }],
				['attachment_start_drag', { id: 'drag-ticket' }],
			])
		},
	)

	it.each(['epoch', 'source'])(
		'discards a queued native start as soon as the document %s changes',
		async (change) => {
			const pendingNative = deferred(false)
			nativeDrag = () => pendingNative.promise
			const epoch = space.epoch.value
			const job = start()
			await vi.waitFor(() => expect(dragCalls()).toHaveLength(2))
			if (change === 'epoch') currentDocument = { ...fixture(), id: 'other-space' }
			else currentPath = 'C:\\synthetic-fixtures\\queued-other.copper'
			await space.adopt(currentDocument)
			if (change === 'source') expect(space.epoch.value).toBe(epoch)
			else expect(space.epoch.value).toBeGreaterThan(epoch)
			expect(dragCalls()).toEqual([
				['attachment_prepare_drag', { targets: [first], source: SOURCE }],
				['attachment_start_drag', { id: 'drag-ticket' }],
				['attachment_discard_drag', { id: 'drag-ticket' }],
			])
			expect(drag.dragging.value).toBe(true)
			pendingNative.resolve(false)
			await job
			expect(drag.dragging.value).toBe(false)
		},
	)

	it.each(['preparation', 'native start'])(
		'clears global dragging after a %s error and allows a retry',
		async (phase) => {
			if (phase === 'preparation')
				prepare = async () => {
					throw new Error('Preparation failed')
				}
			else
				nativeDrag = async () => {
					throw new Error('Native drag failed')
				}
			await start()
			expect(drag.dragging.value).toBe(false)
			expect(dragCalls().map(([command]) => command)).toEqual(
				phase === 'preparation'
					? ['attachment_prepare_drag']
					: ['attachment_prepare_drag', 'attachment_start_drag', 'attachment_discard_drag'],
			)
			prepare = async () => 'retry-ticket'
			nativeDrag = async () => true
			await start(second)
			expect(dragCalls().slice(-2)).toEqual([
				['attachment_prepare_drag', { targets: [second], source: SOURCE }],
				['attachment_start_drag', { id: 'retry-ticket' }],
			])
		},
	)

	it('keeps the global guard through pending discard and releases it even when discard fails', async () => {
		const pendingPreparation = deferred('drag-ticket')
		const pendingDiscard = deferred<void>(undefined)
		prepare = () => pendingPreparation.promise
		discard = () => pendingDiscard.promise
		const logged = vi.spyOn(console, 'error').mockImplementation(() => {})
		const controller = new AbortController()
		const job = start(first, controller.signal)
		controller.abort()
		pendingPreparation.resolve('discard-id')
		await vi.waitFor(() => expect(dragCalls()).toHaveLength(2))
		expect(drag.dragging.value).toBe(true)
		await start(second)
		pendingDiscard.reject(new Error('Discard failed'))
		await job
		expect(drag.dragging.value).toBe(false)
		expect(logged).toHaveBeenCalled()
		expect(dragCalls()).toEqual([
			['attachment_prepare_drag', { targets: [first], source: SOURCE }],
			['attachment_discard_drag', { id: 'discard-id' }],
		])
		logged.mockRestore()
		prepare = async () => 'drag-ticket'
		await start(second)
		expect(dragCalls().slice(-1)).toEqual([['attachment_start_drag', { id: 'drag-ticket' }]])
	})
})
