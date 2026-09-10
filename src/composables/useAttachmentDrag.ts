import { invoke } from '@tauri-apps/api/core'
import { errorMessage } from '@/lib/rustError'
import { useAttachmentSelection } from './useAttachmentSelection'
import { useSpace } from './useSpace'
import { useStatusMessage } from './useStatusMessage'
import type { AttachmentTarget } from './useSystemClipboard'

const dragging = ref(false)

/** The press owns its ticket until the native event-loop thread starts the drag. */
async function start(target: AttachmentTarget, signal: AbortSignal) {
	if (dragging.value || signal.aborted) return
	const space = useSpace()
	const source = space.source.value
	if (!source) return
	const epoch = space.epoch.value
	const selection = useAttachmentSelection()
	selection.ensure(target)
	const targets = selection.copyTargets(target)
	const current = () => space.epoch.value === epoch && space.isSource(source)
	dragging.value = true
	let ticket: string | null = null
	let cancelled = false
	let discard: Promise<unknown> | null = null
	function cancel() {
		cancelled = true
		if (ticket === null) return
		const id = ticket
		ticket = null
		discard = invoke('attachment_discard_drag', { id }).catch((error) => {
			console.error('[copper] could not discard attachment drag', error)
		})
	}
	signal.addEventListener('abort', cancel, { once: true })
	const stopSourceWatch = watch(
		() => [space.epoch.value, space.source.value],
		() => {
			if (!current()) cancel()
		},
		{ flush: 'sync' },
	)
	try {
		ticket = await invoke<string>('attachment_prepare_drag', { targets, source })
		if (cancelled || signal.aborted || !current()) return
		await invoke<boolean>('attachment_start_drag', { id: ticket })
		ticket = null
	} catch (error) {
		if (!cancelled && current()) useStatusMessage().setError(errorMessage(error))
	} finally {
		stopSourceWatch()
		signal.removeEventListener('abort', cancel)
		if (ticket !== null) cancel()
		await discard
		dragging.value = false
	}
}

export function useAttachmentDrag() {
	return { start, dragging: readonly(dragging) }
}
