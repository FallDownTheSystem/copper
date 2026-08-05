import { beforeEach, describe, expect, it } from 'vite-plus/test'

import { useNoteEditor } from './useNoteEditor'
import type { Note, Space } from './useSpace'

function makeSpace(bodies: Record<string, string>, id = 'spc_1'): Space {
	return {
		id,
		name: 'development',
		activeSection: 'sec_a',
		sections: [{ id: 'sec_a', name: 'Research', order: 0 }],
		notes: Object.entries(bodies).map(([noteId, body], order) => ({
			id: noteId,
			section: 'sec_a',
			order,
			done: false,
			body,
			created: '2026-08-05T00:00:00Z',
			updated: '2026-08-05T00:00:00Z',
		})),
	}
}

function noteOf(space: Space, id: string): Note {
	const note = space.notes.find((candidate) => candidate.id === id)
	if (!note) throw new Error(`no note ${id}`)
	return note
}

const editor = useNoteEditor()

const original = makeSpace({ n1: 'original body', n2: 'other' })

beforeEach(() => {
	editor.cancel()
	editor.beginEdit(original, noteOf(original, 'n1'))
	editor.setDraft('my edit')
})

describe('an external reload that did not touch the edited note', () => {
	it('leaves the draft untouched', () => {
		editor.reconcile(makeSpace({ n1: 'original body', n2: 'changed elsewhere' }), false)

		expect(editor.session.value?.draft).toBe('my edit')
		expect(editor.session.value?.conflict).toBeNull()
		expect(editor.canCommit.value).toBe(true)
	})
})

describe('an external change to the edited note', () => {
	it('keeps the row in editing mode so the resolution controls are reachable', () => {
		editor.reconcile(makeSpace({ n1: 'someone else wrote this', n2: 'other' }), false)

		// Regression: `isEditing` used to require `conflict === null`, which
		// unmounted NoteEditor the instant a conflict was raised — taking the draft
		// off screen and leaving `Keep my version` / `Use the external version`
		// unreachable, so the conflict state had no exit at all.
		expect(editor.isEditing('n1')).toBe(true)
		expect(editor.editingNoteId.value).toBe('n1')
		expect(editor.session.value?.draft).toBe('my edit')
	})

	it('raises the conflict state and blocks committing', () => {
		editor.reconcile(makeSpace({ n1: 'someone else wrote this', n2: 'other' }), false)

		expect(editor.session.value?.conflict).toBe('someone else wrote this')
		// Without this, a later blur or Ctrl+Enter calls edit_note and overwrites
		// the external body — exactly the data loss the conflict state exists for.
		expect(editor.canCommit.value).toBe(false)
		expect(editor.conflicted.value).not.toBeNull()
	})

	it('does not conflict when the file caught up with the draft', () => {
		editor.reconcile(makeSpace({ n1: 'my edit', n2: 'other' }), false)

		expect(editor.session.value?.conflict).toBeNull()
		expect(editor.session.value?.baseBody).toBe('my edit')
	})

	it('discards the draft on "use the external version"', () => {
		editor.reconcile(makeSpace({ n1: 'theirs', n2: 'other' }), false)
		editor.resolveUseExternal()

		expect(editor.session.value).toBeNull()
	})

	it('writes exactly once on "keep my version" and then closes', () => {
		editor.reconcile(makeSpace({ n1: 'theirs', n2: 'other' }), false)

		const submission = editor.resolveKeepMine()
		expect(submission).toEqual({ body: 'my edit', revision: 1 })
		// A second call must not queue a second write.
		expect(editor.resolveKeepMine()).toBeNull()

		editor.finishCommit(submission!.revision, true)
		expect(editor.session.value).toBeNull()
	})
})

describe('an external deletion of the edited note', () => {
	it('preserves the draft as a recovery surface and never re-creates the note', () => {
		editor.reconcile(makeSpace({ n2: 'other' }), false)

		expect(editor.recovery.value?.draft).toBe('my edit')
		expect(editor.editingNoteId.value).toBeNull()
		expect(editor.canCommit.value).toBe(false)

		editor.dismissRecovery()
		expect(editor.session.value).toBeNull()
	})
})

describe('space identity replacement', () => {
	it('detaches the draft rather than re-associating it with a matching id', () => {
		// A different document in which the same note id coincidentally exists.
		editor.reconcile(makeSpace({ n1: 'a wholly different note' }, 'spc_2'), true)

		expect(editor.recovery.value?.draft).toBe('my edit')
		expect(editor.session.value?.conflict).toBeNull()
	})
})

describe('an in-flight commit of our own', () => {
	it('is not mistaken for an external conflict when the user typed during it', () => {
		const submission = editor.beginCommit()!
		editor.setDraft('typed while pending')

		// The coordinator applies the document `edit_note` returned before
		// `finishCommit` runs. That body matches neither `baseBody` nor the current
		// draft, so without the pending-body check it reads as somebody else's
		// change — a conflict against our own write.
		editor.reconcile(makeSpace({ n1: submission.body, n2: 'other' }), false)

		expect(editor.session.value?.conflict).toBeNull()
		expect(editor.canCommit.value).toBe(true)
		expect(editor.session.value?.draft).toBe('typed while pending')
	})

	it('still detects a genuine external change landing during the same window', () => {
		editor.beginCommit()
		editor.setDraft('typed while pending')

		editor.reconcile(makeSpace({ n1: 'somebody else entirely', n2: 'other' }), false)

		expect(editor.session.value?.conflict).toBe('somebody else entirely')
		expect(editor.canCommit.value).toBe(false)
	})
})

describe('commit revision guarding', () => {
	it('closes the editor when the field is unchanged since submit', () => {
		const submission = editor.beginCommit()!
		editor.finishCommit(submission.revision, true)

		expect(editor.session.value).toBeNull()
	})

	it('keeps the editor open when the user typed after submitting', () => {
		const submission = editor.beginCommit()!
		editor.setDraft('typed while pending')
		editor.finishCommit(submission.revision, true)

		// A success must not destroy newer input.
		expect(editor.session.value?.draft).toBe('typed while pending')
		expect(editor.session.value?.baseBody).toBe('typed while pending')
	})

	it('leaves everything in place when the command failed', () => {
		const submission = editor.beginCommit()!
		editor.finishCommit(submission.revision, false)

		expect(editor.session.value?.draft).toBe('my edit')
		expect(editor.session.value?.pending).toBe(false)
	})
})

describe('composition tracking', () => {
	it('is held on the session, where a blur can consult it', () => {
		// `event.isComposing` and `keyCode` exist only on keyboard events; a
		// FocusEvent and an unmount hook carry neither.
		editor.setComposing(true)
		expect(editor.session.value?.composing).toBe(true)

		editor.setComposing(false)
		expect(editor.session.value?.composing).toBe(false)
	})
})
