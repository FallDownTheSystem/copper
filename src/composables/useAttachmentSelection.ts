/**
 * Attachment selection is separate from note-row selection. A selected file must
 * never turn a copy or delete gesture into an operation on its containing note.
 * The document arrives from useSpace, without a runtime import back into it.
 */

import { noteRow, useSelection } from './useSelection'
import type { SpaceView } from './useSpace'
import type { AttachmentTarget } from './useSystemClipboard'

export function attachmentKey(target: AttachmentTarget) {
	return JSON.stringify([target.note, target.attachment])
}

const notes = useSelection()
const entries = shallowRef<readonly AttachmentTarget[]>([])
const selectedKeys = shallowRef<readonly string[]>([])
const focused = shallowRef<AttachmentTarget | null>(null)
let anchor: string | null = null
let spaceId: string | null = null

const available = computed(() => {
	const byNote = new Map<string, AttachmentTarget[]>()
	for (const entry of entries.value) {
		const group = byNote.get(entry.note) ?? []
		group.push(entry)
		byNote.set(entry.note, group)
	}
	return notes.visibleNoteIds.value.flatMap((note) => byNote.get(note) ?? [])
})
const selectedSet = computed(() => new Set(selectedKeys.value))
const selected = computed(() =>
	available.value.filter((target) => selectedSet.value.has(attachmentKey(target))),
)
const count = computed(() => selected.value.length)
const active = computed(() => count.value > 0 || focused.value !== null)

function clear() {
	selectedKeys.value = []
	focused.value = null
	anchor = null
}

function syncDocument(space: SpaceView | null) {
	if (spaceId !== (space?.id ?? null)) clear()
	spaceId = space?.id ?? null
	entries.value =
		space?.notes.flatMap((note) =>
			(note.attachments ?? []).map((attachment) => ({
				note: note.id,
				attachment: attachment.id,
			})),
		) ?? []
}

function isSelected(target: AttachmentTarget) {
	return selectedSet.value.has(attachmentKey(target))
}

function focus(target: AttachmentTarget) {
	notes.focusRow(noteRow(target.note))
	focused.value = target
}

function select(
	target: AttachmentTarget,
	modifiers: { ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean } = {},
) {
	notes.clear()
	focus(target)
	const key = attachmentKey(target)
	if (modifiers.shiftKey && anchor) {
		const order = available.value.map(attachmentKey)
		const from = order.indexOf(anchor)
		const to = order.indexOf(key)
		if (from !== -1 && to !== -1) {
			selectedKeys.value = order.slice(Math.min(from, to), Math.max(from, to) + 1)
			return
		}
	}
	if (modifiers.ctrlKey || modifiers.metaKey) {
		selectedKeys.value = isSelected(target)
			? selectedKeys.value.filter((entry) => entry !== key)
			: [...selectedKeys.value, key]
	} else {
		selectedKeys.value = [key]
	}
	anchor = key
}

function toggle(target: AttachmentTarget) {
	select(target, { ctrlKey: true })
}

function ensure(target: AttachmentTarget) {
	if (isSelected(target)) focus(target)
	else select(target)
}

function selectAllInNote(note: string) {
	notes.clear()
	selectedKeys.value = available.value.filter((entry) => entry.note === note).map(attachmentKey)
	anchor = selectedKeys.value[0] ?? null
}

function copyTargets(preferred = focused.value): AttachmentTarget[] {
	if (
		preferred &&
		available.value.some((entry) => attachmentKey(entry) === attachmentKey(preferred)) &&
		!isSelected(preferred)
	) {
		return [{ ...preferred }]
	}
	return selected.value.map((target) => ({ ...target }))
}

function move(
	target: AttachmentTarget,
	step: number,
	modifiers: { ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean },
) {
	const index = available.value.findIndex((entry) => attachmentKey(entry) === attachmentKey(target))
	const next = available.value[index + step]
	if (index === -1 || !next) return null
	if (modifiers.ctrlKey || modifiers.metaKey) focus(next)
	else select(next, modifiers)
	return next
}

watch(available, (targets) => {
	const keys = new Set(targets.map(attachmentKey))
	selectedKeys.value = selectedKeys.value.filter((key) => keys.has(key))
	if (focused.value && !keys.has(attachmentKey(focused.value))) focused.value = null
	if (anchor && !keys.has(anchor)) anchor = null
})

watch(notes.selectionIntent, clear, { flush: 'sync' })

export function useAttachmentSelection() {
	return {
		selected,
		count,
		active,
		focused: readonly(focused),
		syncDocument,
		isSelected,
		focus,
		select,
		toggle,
		ensure,
		selectAllInNote,
		copyTargets,
		move,
		clear,
	}
}
