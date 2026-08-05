/**
 * The only caller of `clipboard_write_text`.
 *
 * One adapter per Rust surface, the same rule `useSpace` holds for the store: no
 * component invokes a command directly, so the invoke string exists in exactly
 * one place and a rename touches one file.
 *
 * Named `useSystemClipboard` rather than `useClipboard` because VueUse ships a
 * `useClipboard` of its own and both are auto-imported. Two different clipboards
 * under one name is not a cosmetic clash: VueUse's goes through the browser's
 * async Clipboard API, while this one reaches Win32 directly and sets the three
 * privacy formats — so a caller reaching for the familiar name and silently
 * getting the other would change what lands in `Win+V` history.
 */

import { invoke } from '@tauri-apps/api/core'

async function writeText(text: string): Promise<boolean> {
	try {
		await invoke('clipboard_write_text', { text })
		return true
	} catch (error) {
		console.error('[copper] clipboard write failed', error)
		return false
	}
}

export function useSystemClipboard() {
	return { writeText }
}
