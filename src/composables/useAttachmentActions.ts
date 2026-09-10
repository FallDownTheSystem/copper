import { errorMessage } from '@/lib/rustError'

import { useAttachmentSelection } from './useAttachmentSelection'
import { useSpace } from './useSpace'
import { focusRowSoon, noteRow } from './useSelection'
import { useInteractionMode } from './useInteractionMode'
import { countMessage, useStatusMessage } from './useStatusMessage'
import { useSystemClipboard, type AttachmentCopyFormat } from './useSystemClipboard'

const selection = useAttachmentSelection()
const clipboard = useSystemClipboard()
const space = useSpace()
const status = useStatusMessage()

async function copy(format: AttachmentCopyFormat = 'auto', targets = selection.copyTargets()) {
	if (targets.length === 0) return false
	const source = space.source.value
	if (!source) {
		status.setError('Open a space before copying attachments.')
		return false
	}
	const epoch = space.epoch.value
	try {
		const result = await clipboard.copyAttachments(targets, format, source)
		if (space.epoch.value === epoch && space.isSource(source)) {
			status.setMessage(
				result.format === 'image'
					? 'Image copied'
					: countMessage(
							result.count,
							result.format === 'paths'
								? { one: 'Copied 1 attachment path', many: (n) => `Copied ${n} attachment paths` }
								: { one: 'Copied 1 attachment', many: (n) => `Copied ${n} attachments` },
						),
			)
		}
		return true
	} catch (error) {
		if (space.epoch.value === epoch && space.isSource(source)) status.setError(errorMessage(error))
		return false
	}
}

function dismiss() {
	const note = selection.focused.value?.note ?? selection.selected.value[0]?.note
	selection.clear()
	const interaction = useInteractionMode()
	if (interaction.interactionRowId.value) interaction.exit()
	else if (note) focusRowSoon(noteRow(note))
}

export function useAttachmentActions() {
	return { copy, dismiss, copying: clipboard.copying }
}
