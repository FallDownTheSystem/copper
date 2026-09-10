/**
 * Clipboard gestures never wait in a queue: a delayed gesture could overwrite a
 * newer copy from another application before its native sequence guard starts.
 * One native copy runs at a time; callers must retry after it finishes.
 */

import { invoke } from '@tauri-apps/api/core'

import type { DocumentSource, NoteSelection, RenderedNotes } from './useSpace'

export type AttachmentTarget = { note: string; attachment: string }
export type AttachmentCopyFormat = 'auto' | 'image' | 'files' | 'paths'
export type CopiedAttachments = { count: number; format: 'image' | 'files' | 'paths' }

const copying = ref(false)

async function exclusive<T>(write: () => Promise<T>): Promise<T> {
	if (copying.value) throw new Error('A copy is in progress. Try again when it finishes.')
	copying.value = true
	try {
		return await write()
	} finally {
		copying.value = false
	}
}

async function writeText(text: string): Promise<boolean> {
	try {
		await exclusive(() => invoke('clipboard_write_text', { text }))
		return true
	} catch (error) {
		console.error('[copper] clipboard write failed', error)
		return false
	}
}

function copyAttachments(
	targets: AttachmentTarget[],
	format: AttachmentCopyFormat,
	source: DocumentSource,
) {
	return exclusive(() =>
		invoke<CopiedAttachments>('clipboard_copy_attachments', { targets, format, source }),
	)
}

function copyNotesWithAttachments(selection: NoteSelection, source: DocumentSource) {
	return exclusive(() => invoke<RenderedNotes>('clipboard_copy_notes', { selection, source }))
}

export function useSystemClipboard() {
	return { writeText, copyAttachments, copyNotesWithAttachments, copying: readonly(copying) }
}
