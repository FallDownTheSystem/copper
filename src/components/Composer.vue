<script setup lang="ts">
import { useAutoSize } from '@/composables/useAutoSize'
import { noteRow, takeRow } from '@/composables/useSelection'
import { isComposing } from '@/lib/chords'

const { spaceName, submitEntry, errorFor, clearActionError, reportActionError } = useSpace()

const composerError = errorFor('composer')
const { visibleNoteIds } = useSelection()
const { switcherOpen } = useSections()
const { pending, pasteAttachment, pickAttachments, removePending } = useAttachments()
const { entrySubmitted } = useSounds()

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
 * Every attach attempt starts by retiring the previous one's message — the same
 * discipline `useSettings`'s `attempt` applies to a settings row, and the reason
 * both need it is that `report` can only ever *add*.
 *
 * Without this a refusal outlived every later attach that succeeded: the new
 * file landed in the tray with the old failure still sitting under it, until a
 * keystroke happened to clear it through `onInput`. On the way in rather than on
 * the way out, so the stale message is not on screen for the length of the round
 * trip either. `DropTarget` is the fourth ingest path and clears the same scope
 * for the same reason.
 */
function beginAttach() {
	clearActionError('composer')
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
	// Ahead of the round trip that decides whether this was an attachment at all,
	// which costs nothing when it turns out to have been text: the native paste
	// that ran alongside it fires `input`, and that clears this scope too.
	beginAttach()
	const outcome = await pasteAttachment()
	if (!outcome.handled) return
	report(outcome.message)
	focus()
}

async function pick() {
	beginAttach()
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

	// The store accepted it, independently of the revision race below — that guard
	// is about whose text is in the field, not about whether the note was written.
	// A `# Name` directive sounds the same: it is still a composer submit, and the
	// section it activates does not go through `setActiveSection`.
	if (result) entrySubmitted()

	// Nothing is cleared optimistically, and a success must not destroy newer
	// input: the field is only cleared if it is unchanged since the request went
	// out.
	if (result && submittedRevision === revision) {
		value.value = ''
		scheduleAutoSize()
	}
	// The tray is cleared on success alone, and independently of the revision
	// check that guards the text — unlike the text, an attachment cannot be
	// "newer input": adding one during a submit would have to go through
	// `pending`, which was copied above, so anything added since is still in the
	// tray and must survive.
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
	// text, not submit the note. The local flag is the third term because the
	// composition events reach this field directly and outlive an individual
	// keypress.
	if (isComposing(event) || composing.value) return

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
				class="panel-field max-h-[5lh] min-h-8 w-full min-w-0 flex-1 resize-none px-2 py-1.5"
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
				class="icon-button hover:text-text-primary hit-44 relative shrink-0"
				@click="pick"
			>
				<IconLucidePaperclip class="size-4" aria-hidden="true" focusable="false" />
			</button>
		</div>

		<p v-if="composerError" class="text-text-primary mt-1 flex items-start gap-1.5 text-meta">
			<IconLucideAlertCircle
				class="mt-0.5 size-3.5 shrink-0"
				aria-hidden="true"
				focusable="false"
			/>
			<span>{{ composerError }}</span>
		</p>
	</form>
</template>
