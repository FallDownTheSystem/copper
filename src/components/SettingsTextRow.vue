<script setup lang="ts">
/**
 * A free-text settings field, committed on Enter or blur.
 *
 * `SettingsSizeRow`'s pattern generalised past numbers. The two properties worth
 * naming are the same ones it needed:
 *
 * - **The draft is a ref, not the prop.** A field bound straight to the stored
 *   value cannot hold `https://cop` on the way to a whole URL.
 * - **A pull never overwrites a focused field.** `share-changed` fires for
 *   writes this row knows nothing about — a poller failure is one — and the
 *   re-pull that follows would land mid-word and replace what the user was
 *   typing.
 *
 * In the row's `below` slot rather than its trailing column: a URL does not fit
 * the strip a 440px panel leaves beside a description.
 */
const props = defineProps<{
	value: string
	placeholder?: string
	/** For the accessible name, since the visible label belongs to the row. */
	label: string
	/** The row's error region, when the row has one. */
	errorId?: string
}>()

const emit = defineEmits<{ commit: [value: string] }>()

const field = useTemplateRef<HTMLInputElement>('field')
const draft = ref(props.value)

watch(
	() => props.value,
	(value) => {
		if (document.activeElement !== field.value) draft.value = value
	},
)

function commit() {
	const value = draft.value.trim()
	// Written back before the round trip: a value that trims to what is already
	// stored produces no change to send, so the watcher above would never fire and
	// the field would sit there still showing the untrimmed text.
	draft.value = value
	if (value === props.value) return
	emit('commit', value)
}
</script>

<template>
	<div class="mt-2">
		<input
			ref="field"
			v-model="draft"
			type="text"
			autocomplete="off"
			autocapitalize="off"
			autocorrect="off"
			spellcheck="false"
			:placeholder="placeholder"
			:aria-label="label"
			:aria-invalid="errorId ? 'true' : undefined"
			:aria-describedby="errorId"
			class="panel-field h-8 w-full px-2 text-meta"
			@keydown.enter.prevent="commit"
			@blur="commit"
		/>
	</div>
</template>
