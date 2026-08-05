<script setup lang="ts">
/**
 * The detached surface for a draft whose note was deleted — or whose whole
 * document was replaced — while it was being edited.
 *
 * Rendered outside the live-note iteration, which is the only way the draft can
 * survive at all: an editor rendered inside the row unmounts with it.
 *
 * The note is never silently re-created. Recovery is the user's call.
 */

const { recovery, dismissRecovery } = useNoteEditor()

const copied = ref(false)

async function copyDraft() {
	const draft = recovery.value?.draft
	if (!draft) return

	try {
		await navigator.clipboard.writeText(draft)
		copied.value = true
	} catch (error) {
		console.error('[copper] could not copy the draft', error)
	}
}
</script>

<template>
	<div
		v-if="recovery"
		role="group"
		aria-label="Unsaved draft"
		class="border-separator mx-3 my-2 rounded-md border border-dashed p-2"
	>
		<p class="text-text-primary text-meta font-semibold">
			The note you were editing is gone from this space.
		</p>
		<p class="text-text-secondary mt-0.5 text-meta">Your unsaved text is kept here.</p>

		<pre
			class="text-text-secondary bg-surface-hover mt-2 max-h-40 overflow-auto rounded p-1.5 text-meta whitespace-pre-wrap select-text"
			>{{ recovery.draft }}</pre>

		<div class="mt-2 flex flex-wrap items-center gap-2">
			<button
				type="button"
				class="border-separator hover:bg-surface-hover rounded-md border px-2 py-1 text-meta transition-colors duration-fast"
				@click="copyDraft"
			>
				Copy the draft
			</button>
			<button
				type="button"
				class="border-separator hover:bg-surface-hover rounded-md border px-2 py-1 text-meta transition-colors duration-fast"
				@click="dismissRecovery"
			>
				Dismiss
			</button>
			<span v-if="copied" class="text-text-secondary text-meta">Copied</span>
		</div>
	</div>
</template>
