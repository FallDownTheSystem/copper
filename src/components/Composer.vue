<script setup lang="ts">
import { useAutoSize } from '@/composables/useAutoSize'
import { noteRow, takeRow } from '@/composables/useSelection'

const { spaceName, submitEntry, errorFor, clearActionError, reportActionError } = useSpace()

const composerError = errorFor('composer')
const { visibleNoteIds } = useSelection()
const { switcherOpen } = useSections()
const { pending, pasteAttachment, pickAttachments, removePending } = useAttachments()

const textarea = useTemplateRef<HTMLTextAreaElement>('textarea')

const value = ref('')
/** Named for the request rather than for the tray beside it: `pending` is the
 *  pending-attachment list now, and one word meaning two things in one file is
 *  how a submit guard silently stops guarding. */
const submitting = ref(false)
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

/**
 * Where the caret was when the section switcher opened, or null if the composer
 * did not have focus at the time.
 *
 * Switching a destination must cost nothing: the half-typed line stays, and so
 * does the position in it. Recorded rather than trusted to survive the blur,
 * because "focus returns to the composer with its caret preserved" is a promise
 * worth holding explicitly rather than one that happens to be true of Chromium.
 *
 * Null is also the signal that the switcher was opened by clicking the chip, in
 * which case reka's own close-focus — back to the chip — is the right answer and
 * is left alone.
 */
let caret: { start: number; end: number } | null = null

watch(switcherOpen, (open) => {
	if (!open) return
	const field = textarea.value
	caret =
		field && document.activeElement === field
			? { start: field.selectionStart, end: field.selectionEnd }
			: null
})

function onSwitcherClosed(event: Event) {
	if (!caret) return
	// Declines reka's return-to-trigger only when there is somewhere better to go.
	event.preventDefault()
	const { start, end } = caret
	caret = null
	const field = textarea.value
	if (!field) return
	field.focus()
	field.setSelectionRange(start, end)
}

function onInput(event: Event) {
	value.value = (event.target as HTMLTextAreaElement).value
	revision++
	if (composerError.value) clearActionError('composer')
	scheduleAutoSize()
}

/** Reported on the composer's own surface, next to the field and the tray the
 *  message is about. */
function report(message: string | null) {
	if (message) reportActionError('composer', message)
}

/**
 * `Ctrl+V`, which may or may not be an attachment.
 *
 * **The native paste is deliberately not prevented.** Deciding needs a round
 * trip to Rust and a `paste` handler is synchronous, so preventing first would
 * mean re-implementing text insertion here — losing the field's native undo
 * stack and its IME behaviour — for a keystroke that is text the overwhelming
 * majority of the time.
 *
 * Letting it run is safe rather than merely convenient: Rust reports an
 * attachment only when the clipboard carries **no** `CF_UNICODETEXT`, because
 * text always wins. So in exactly the cases this handles, the native paste had
 * nothing to insert and inserted nothing.
 */
async function onPaste() {
	const outcome = await pasteAttachment()
	if (!outcome.handled) return
	report(outcome.message)
	focus()
}

async function pick() {
	report(await pickAttachments())
	focus()
}

async function submit() {
	// Emptiness is tested on the trimmed value, but the *untrimmed* value is what
	// is submitted: leading whitespace is significant Markdown — indented code
	// blocks and nested list continuations both depend on it.
	if (value.value.trim().length === 0) return
	// A submit already in flight blocks a second, so holding Enter cannot create
	// duplicates.
	if (submitting.value) return

	const submitted = value.value
	const attachments = [...pending.value]
	const submittedRevision = revision
	submitting.value = true

	const result = await submitEntry(submitted, attachments)
	submitting.value = false

	// Nothing is cleared optimistically, and a success must not destroy newer
	// input: the field is only cleared if it is unchanged since the request went
	// out.
	if (result && submittedRevision === revision) {
		value.value = ''
		scheduleAutoSize()
	}
	// The tray is cleared on success alone, and unconditionally on the revision
	// check — unlike the text, an attachment cannot be "newer input": adding one
	// during a submit would have to go through `pending`, which was copied above,
	// so anything added since is still in the tray and must survive.
	if (result) {
		for (const attachment of attachments) removePending(attachment.id)
	}
	// Focus stays here either way, so consecutive captures need no mouse.
	focus()
}

/** The keyboard route out of the composer and back into the list. Returns false
 *  when there is no list to return to. */
function focusLastNote() {
	const last = visibleNoteIds.value.at(-1)
	if (!last) return false
	takeRow(noteRow(last))
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
		<!-- Above the field, in a row it never shares, so activating a section
		     shifts nothing. The placeholder below still names the *space*, which is
		     task-004's rule and is upheld rather than amended. -->
		<div class="mb-1.5 flex min-w-0">
			<ActiveSectionChip @closed="onSwitcherClosed" />
		</div>

		<AttachmentTray />

		<label for="composer" class="sr-only">New note</label>
		<!-- The field and the paperclip share a row so the button does not cost a
		     line of a panel that cannot grow, and `items-end` keeps it on the last
		     line as the field grows to its five-line cap. -->
		<div class="flex min-w-0 items-end gap-1.5">
			<textarea
				id="composer"
				ref="textarea"
				name="note"
				data-composer
				rows="1"
				autocomplete="off"
				:value="value"
				:placeholder="placeholder"
				:aria-busy="submitting"
				class="border-separator bg-surface-hover text-text-primary placeholder:text-text-disabled outline-focus-ring max-h-[5lh] min-h-8 w-full min-w-0 flex-1 resize-none select-text rounded-md border px-2 py-1.5 text-body focus-visible:outline-2 focus-visible:-outline-offset-1"
				:class="supportsFieldSizing ? 'field-sizing-content' : ''"
				@input="onInput"
				@keydown="onKeydown"
				@paste="onPaste"
				@compositionstart="composing = true"
				@compositionend="composing = false"
			/>
			<button
				type="button"
				aria-label="Attach files"
				class="text-text-secondary hover:text-text-primary hover:bg-surface-hover outline-focus-ring hit-44 relative mb-0.5 grid size-7 shrink-0 place-items-center rounded-md transition-colors duration-fast focus-visible:outline-2"
				@click="pick"
			>
				<IconLucidePaperclip class="size-4" aria-hidden="true" focusable="false" />
			</button>
		</div>

		<p v-if="composerError" class="text-destructive mt-1 text-meta">{{ composerError }}</p>
	</form>
</template>
