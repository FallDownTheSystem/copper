<script setup lang="ts">
import { useAutoSize } from '@/composables/useAutoSize'
import { focusRowSoon } from '@/composables/useSelection'
import { isComposing } from '@/lib/chords'
import { insertNewline } from '@/lib/insertNewline'

const props = defineProps<{ rowId: string }>()

const {
	session,
	canCommit,
	setDraft,
	setComposing,
	cancel,
	beginCommit,
	finishCommit,
	resolveUseExternal,
	resolveKeepMine,
} = useNoteEditor()
const { updateNoteBody, errorFor } = useSpace()
const { enterKeyAction } = useSettings()

const editorError = errorFor('editor')

const textarea = useTemplateRef<HTMLTextAreaElement>('textarea')

const draft = computed(() => session.value?.draft ?? '')
const conflict = computed(() => session.value?.conflict ?? null)
const pending = computed(() => session.value?.pending ?? false)

/** Uncapped: a note body is a document, and the list scrolls around it. */
const { supportsFieldSizing, scheduleAutoSize } = useAutoSize(textarea)

onMounted(() => {
	const element = textarea.value
	if (!element) return
	element.focus()
	element.setSelectionRange(element.value.length, element.value.length)
	scheduleAutoSize()
})

function onInput(event: Event) {
	setDraft((event.target as HTMLTextAreaElement).value)
	scheduleAutoSize()
}

function returnFocusToRow() {
	focusRowSoon(props.rowId)
}

/** The one write path. Both the ordinary commit and `Keep your version` report
 *  back through `finishCommit`, which is what closes the editor only when the
 *  field is unchanged since the request went out. */
async function write(submission: { body: string; revision: number } | null) {
	const id = session.value?.noteId
	if (!submission || !id) return false

	const result = await updateNoteBody(id, submission.body)
	finishCommit(submission.revision, result !== null)
	return true
}

async function commit() {
	if (!canCommit.value) return
	await write(beginCommit())
}

function onKeydown(event: KeyboardEvent) {
	// `stopPropagation` even while composing: the press has to be *withheld* from
	// the shell's Escape ladder, not merely ignored here. Escape closes an IME
	// candidate window, and letting that press continue up would take the ladder's
	// first rung — cancelling the edit — and destroy the draft the user was
	// midway through composing into.
	if (isComposing(event)) {
		if (event.key === 'Escape') event.stopPropagation()
		return
	}

	if (event.key === 'Escape') {
		event.preventDefault()
		event.stopPropagation()
		cancel()
		returnFocusToRow()
		return
	}

	if (event.key === 'Enter') {
		// The same matrix as the composer, chosen by the `Enter key` setting
		// (user ruling 2026-08-11: one rule for both multi-line fields — the old
		// inverted matrix here read as a bug). Shift+Enter is a newline in both
		// modes and is *declined* so the field's own undo and IME behaviour
		// handle it; Ctrl+Enter's newline has to be inserted by hand, Chromium
		// mapping that chord to nothing.
		if (event.shiftKey) return
		const submitOnEnter = enterKeyAction.value === 'submit'
		if (event.ctrlKey || event.metaKey) {
			// `stopPropagation` in both modes, for the same reason `Escape` above
			// has it: the press has to be *withheld* from the shell, not merely
			// handled here. Ctrl+Enter is two things by context —
			// `CHORDS.openInEditor` starts the `$EDITOR` handoff from a focused
			// card — and the shell resolves that today by declining every chord
			// from a text surface. That guard is one reordering away from not
			// covering this: `Ctrl+K` already sits above it as a documented
			// exception, so a second exception would fork a handoff off a note
			// whose inline draft is mid-commit.
			event.preventDefault()
			event.stopPropagation()
			if (submitOnEnter) {
				const field = textarea.value
				if (field) insertNewline(field)
			} else {
				void commit().then(returnFocusToRow)
			}
			return
		}
		// A bare Enter: the browser's newline under `newline`, the save under
		// `submit`. Withheld from the shell either way it is consumed.
		if (!submitOnEnter) return
		event.preventDefault()
		event.stopPropagation()
		void commit().then(returnFocusToRow)
	}
}

/**
 * Commits but leaves focus wherever the user just put it. Committing and then
 * unconditionally refocusing the row fights the user — clicking the composer,
 * the search field or the `...` menu would yank focus straight back.
 */
function onBlur() {
	// A FocusEvent carries neither `isComposing` nor `keyCode`, which is why the
	// flag is tracked explicitly on the session.
	if (session.value?.composing) return
	if (!canCommit.value) return
	void commit()
}

function useExternal() {
	resolveUseExternal()
	returnFocusToRow()
}

async function keepMine() {
	// Gated on the write actually happening: a second click after the conflict is
	// resolved gets null back, and must not move focus off whatever now has it.
	if (await write(resolveKeepMine())) returnFocusToRow()
}
</script>

<template>
	<div class="min-w-0">
		<p v-if="conflict !== null" class="text-text-secondary mb-1 text-meta font-semibold uppercase">
			Your version
		</p>
		<textarea
			ref="textarea"
			:value="draft"
			:aria-busy="pending"
			aria-label="Edit note"
			class="panel-field w-full min-w-0 resize-none px-2 py-1.5"
			:class="supportsFieldSizing ? 'field-sizing-content' : ''"
			@input="onInput"
			@keydown="onKeydown"
			@blur="onBlur"
			@contextmenu.stop
			@compositionstart="setComposing(true)"
			@compositionend="setComposing(false)"
		/>

		<!-- Stacked and labelled, never side by side: the panel is ~440px wide and
		     two columns of Markdown at that width are unreadable. -->
		<div
			v-if="conflict !== null"
			class="squircle border-separator mt-2 rounded-lg border p-2"
			role="group"
			aria-label="This note changed on disk"
		>
			<p class="text-text-primary text-meta font-semibold">This note changed on disk.</p>
			<p class="text-text-secondary mt-0.5 text-meta">
				Nothing is written until you choose. Saving is paused meanwhile.
			</p>

			<p class="text-text-secondary mt-2 text-meta font-semibold uppercase">On disk</p>
			<pre
				class="text-text-secondary bg-surface-hover mt-1 max-h-32 overflow-auto rounded-sm p-1.5 text-meta whitespace-pre-wrap"
				>{{ conflict }}</pre>

			<div class="mt-2 flex flex-wrap gap-2">
				<button type="button" class="panel-button" @click="keepMine">Keep your version</button>
				<button type="button" class="panel-button" @click="useExternal">
					Use the version on disk
				</button>
			</div>
		</div>

		<p v-if="editorError" class="text-text-primary mt-1 flex items-start gap-1.5 text-meta">
			<IconLucideAlertCircle
				class="mt-0.5 size-3.5 shrink-0"
				aria-hidden="true"
				focusable="false"
			/>
			<span>{{ editorError }}</span>
		</p>
	</div>
</template>
