<script setup lang="ts">
import { CHORDS } from '@/lib/chords'
import { useAttachmentSelection } from '@/composables/useAttachmentSelection'
import { useAttachmentActions } from '@/composables/useAttachmentActions'

const { count, selected } = useAttachmentSelection()
const { copy, copying, dismiss } = useAttachmentActions()
</script>

<template>
	<div
		v-if="count > 0"
		data-attachment-actions
		role="group"
		aria-label="Attachment actions"
		title="Drag selected attachments to another app. Ctrl+click toggles selection; Shift+click selects a range."
		class="text-text-secondary flex min-w-0 items-center gap-2 px-3 pb-2 text-meta"
	>
		<span class="min-w-0 flex-1 truncate tabular-nums"
			>{{ count }} {{ count === 1 ? 'attachment' : 'attachments' }}</span
		>
		<button
			type="button"
			class="panel-button shrink-0"
			:disabled="copying"
			:title="`Copy attachments (${CHORDS.copy.display})`"
			@click="copy('auto', [...selected])"
		>
			{{ copying ? 'Copying…' : 'Copy' }}
		</button>
		<button
			type="button"
			class="panel-button shrink-0"
			:disabled="copying"
			@click="copy('paths', [...selected])"
		>
			Copy paths
		</button>
		<button
			type="button"
			class="icon-button shrink-0"
			aria-label="Clear attachment selection"
			@click="dismiss"
		>
			<IconLucideX class="size-4" aria-hidden="true" />
		</button>
	</div>
</template>
