<script setup lang="ts">
import { rowElement } from '@/composables/useSelection'

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
const { updateNoteBody, actionError } = useSpace()

const textarea = useTemplateRef<HTMLTextAreaElement>('textarea')

const draft = computed(() => session.value?.draft ?? '')
const conflict = computed(() => session.value?.conflict ?? null)
const pending = computed(() => session.value?.pending ?? false)

/**
 * `field-sizing: content` is the clean answer but landed in Chromium 123, while
 * the build targets chrome105 and Evergreen WebView2 cannot be pinned to one
 * runtime version. The fallback must not reset height and read `scrollHeight`
 * per keystroke — that forces synchronous layout on every character.
 */
const supportsFieldSizing =
	typeof CSS !== 'undefined' && CSS.supports?.('field-sizing', 'content') === true

let sizingFrame = 0

function scheduleAutoSize() {
	if (supportsFieldSizing) return
	cancelAnimationFrame(sizingFrame)
	sizingFrame = requestAnimationFrame(() => {
		const element = textarea.value
		if (!element) return
		element.style.height = 'auto'
		element.style.height = `${element.scrollHeight}px`
	})
}

onMounted(() => {
	const element = textarea.value
	if (!element) return
	element.focus()
	element.setSelectionRange(element.value.length, element.value.length)
	scheduleAutoSize()
})

onBeforeUnmount(() => cancelAnimationFrame(sizingFrame))

function onInput(event: Event) {
	setDraft((event.target as HTMLTextAreaElement).value)
	scheduleAutoSize()
}

function returnFocusToRow() {
	// nextTick: focusing before Vue has patched the DOM lands on an element that
	// is about to be replaced.
	void nextTick(() => {
		rowElement(props.rowId)?.focus()
	})
}

async function commit() {
	if (!canCommit.value) return
	const submission = beginCommit()
	if (!submission) return

	const id = session.value?.noteId
	if (!id) return

	const result = await updateNoteBody(id, submission.body)
	finishCommit(submission.revision, result !== null)
}

function onKeydown(event: KeyboardEvent) {
	// WebView2 still reports keyCode 229 during composition, and a Japanese,
	// Chinese or Korean user accepting a candidate would otherwise commit.
	if (event.isComposing || event.keyCode === 229) return

	if (event.key === 'Escape') {
		event.preventDefault()
		event.stopPropagation()
		cancel()
		returnFocusToRow()
		return
	}

	if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
		event.preventDefault()
		void commit().then(returnFocusToRow)
	}
	// Bare Enter inserts a newline: a note body is a document, unlike the
	// composer's capture line.
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
	const submission = resolveKeepMine()
	const id = session.value?.noteId
	if (!submission || !id) return

	const result = await updateNoteBody(id, submission.body)
	finishCommit(submission.revision, result !== null)
	returnFocusToRow()
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
			class="border-separator bg-surface-hover text-text-primary outline-focus-ring w-full min-w-0 select-text resize-none rounded-md border px-2 py-1.5 text-body focus-visible:outline-2 focus-visible:-outline-offset-1"
			:class="supportsFieldSizing ? 'field-sizing-content' : ''"
			@input="onInput"
			@keydown="onKeydown"
			@blur="onBlur"
			@compositionstart="setComposing(true)"
			@compositionend="setComposing(false)"
		/>

		<!-- Stacked and labelled, never side by side: the panel is ~390px wide and
		     two columns of Markdown at that width are unreadable. -->
		<div
			v-if="conflict !== null"
			class="border-separator mt-2 rounded-md border p-2"
			role="group"
			aria-label="This note changed on disk"
		>
			<p class="text-text-primary text-meta font-semibold">This note changed on disk.</p>
			<p class="text-text-secondary mt-0.5 text-meta">
				Nothing is written until you choose. Saving is paused meanwhile.
			</p>

			<p class="text-text-secondary mt-2 text-meta font-semibold uppercase">On disk</p>
			<pre
				class="text-text-secondary bg-surface-hover mt-1 max-h-32 overflow-auto rounded p-1.5 text-meta whitespace-pre-wrap"
				>{{ conflict }}</pre>

			<div class="mt-2 flex flex-wrap gap-2">
				<button
					type="button"
					class="border-separator hover:bg-surface-hover rounded-md border px-2 py-1 text-meta transition-colors duration-fast"
					@click="keepMine"
				>
					Keep my version
				</button>
				<button
					type="button"
					class="border-separator hover:bg-surface-hover rounded-md border px-2 py-1 text-meta transition-colors duration-fast"
					@click="useExternal"
				>
					Use the external version
				</button>
			</div>
		</div>

		<p v-if="actionError" class="text-destructive mt-1 text-meta">{{ actionError }}</p>
	</div>
</template>
