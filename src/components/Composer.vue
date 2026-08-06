<script setup lang="ts">
import { useAutoSize } from '@/composables/useAutoSize'
import { focusRowSoon, noteRow } from '@/composables/useSelection'

const { spaceName, addNote, errorFor, clearActionError } = useSpace()

const composerError = errorFor('composer')
const { visibleNoteIds, focusRow } = useSelection()

const textarea = useTemplateRef<HTMLTextAreaElement>('textarea')

const value = ref('')
const pending = ref(false)
const composing = ref(false)
/** Bumped on every keystroke and captured at submit, so a result that lands
 *  after the user typed more cannot clear newer input. */
let revision = 0

const placeholder = computed(() => `Add a note or a prompt (${spaceName.value})`)

/** Capped at five lines, tracking the `max-h-[5lh]` on the field itself. */
const { supportsFieldSizing, scheduleAutoSize } = useAutoSize(textarea, { maxLines: 5 })

function focus() {
	textarea.value?.focus()
}

defineExpose({ focus })

function onInput(event: Event) {
	value.value = (event.target as HTMLTextAreaElement).value
	revision++
	if (composerError.value) clearActionError('composer')
	scheduleAutoSize()
}

async function submit() {
	// Emptiness is tested on the trimmed value, but the *untrimmed* value is what
	// is submitted: leading whitespace is significant Markdown — indented code
	// blocks and nested list continuations both depend on it.
	if (value.value.trim().length === 0) return
	// A submit already in flight blocks a second, so holding Enter cannot create
	// duplicates.
	if (pending.value) return

	const submitted = value.value
	const submittedRevision = revision
	pending.value = true

	const result = await addNote(submitted)
	pending.value = false

	// Nothing is cleared optimistically, and a success must not destroy newer
	// input: the field is only cleared if it is unchanged since the request went
	// out.
	if (result && submittedRevision === revision) {
		value.value = ''
		scheduleAutoSize()
	}
	// Focus stays here either way, so consecutive captures need no mouse.
	focus()
}

/** The keyboard route out of the composer and back into the list. Returns false
 *  when there is no list to return to. */
function focusLastNote() {
	const last = visibleNoteIds.value.at(-1)
	if (!last) return false
	focusRow(noteRow(last))
	focusRowSoon(noteRow(last))
	return true
}

function onKeydown(event: KeyboardEvent) {
	// Before anything else: an IME candidate confirmed with Enter must insert
	// text, not submit the note. WebView2 still reports keyCode 229 while
	// composing.
	if (event.isComposing || event.keyCode === 229 || composing.value) return

	if (event.key === 'Enter') {
		// Deviation from the form convention, and a deliberate one: the composer is
		// a capture line in a keyboard-first tool, so the most frequent action in
		// the app must not require a chord. Both modifiers give a newline.
		if (event.shiftKey || event.ctrlKey || event.metaKey) return
		event.preventDefault()
		void submit()
		return
	}

	if (event.key === 'Escape') {
		// Escape is always consumed here; it just stays put when the list is empty.
		event.preventDefault()
		focusLastNote()
		return
	}

	if (event.key === 'ArrowUp' && value.value.length === 0) {
		// Let the key through untouched when there are no notes, rather than
		// silently swallowing the keystroke.
		if (focusLastNote()) event.preventDefault()
	}
}
</script>

<template>
	<!-- There is no submit button, so every other button inside the form needs an
	     explicit type: the default is `submit`. -->
	<form
		aria-label="Add a note"
		class="border-separator border-t px-3 py-2"
		@submit.prevent="submit"
	>
		<label for="composer" class="sr-only">New note</label>
		<textarea
			id="composer"
			ref="textarea"
			name="note"
			data-composer
			rows="1"
			autocomplete="off"
			:value="value"
			:placeholder="placeholder"
			:aria-busy="pending"
			class="border-separator bg-surface-hover text-text-primary placeholder:text-text-disabled outline-focus-ring max-h-[5lh] min-h-8 w-full min-w-0 resize-none select-text rounded-md border px-2 py-1.5 text-body focus-visible:outline-2 focus-visible:-outline-offset-1"
			:class="supportsFieldSizing ? 'field-sizing-content' : ''"
			@input="onInput"
			@keydown="onKeydown"
			@compositionstart="composing = true"
			@compositionend="composing = false"
		/>

		<p v-if="composerError" class="text-destructive mt-1 text-meta">{{ composerError }}</p>
	</form>
</template>
