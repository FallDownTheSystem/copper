<script setup lang="ts">
/**
 * The panel's default width and height, as two number fields and a reset.
 *
 * **Committed on Enter or blur, never per keystroke.** Every intermediate state
 * of a typed number is a legal number — `4` on the way to `440` clamps to the
 * minimum — so a field that wrote as you typed would resize the window twice
 * before you finished the value you meant.
 *
 * In the row's `below` slot rather than its trailing column, the same departure
 * `ShortcutRecorder` makes and for the same reason: two fields, a separator and a
 * reset button do not fit the strip a 440px panel leaves beside a description.
 *
 * The clamp is duplicated here, in `useSettings` and in Rust. That is deliberate
 * rather than untidy — the store *repairs* an out-of-band size, so a field that
 * could send one would leave the user reading a number the file had silently
 * rewritten. Clamping at the edge means what the field shows after a commit is
 * always what was actually applied.
 */
const props = defineProps<{
	width: number
	height: number
	minWidth: number
	maxWidth: number
	minHeight: number
	maxHeight: number
	defaultWidth: number
	defaultHeight: number
	/** The row's error region, when the row has one. Present only while the error
	 *  is, so neither field is described by an empty node the rest of the time. */
	errorId?: string
}>()

const emit = defineEmits<{ commit: [width: number, height: number] }>()

const widthField = useTemplateRef<HTMLInputElement>('widthField')
const heightField = useTemplateRef<HTMLInputElement>('heightField')

/** The text being typed, which is not the stored value and must not be: a field
 *  bound straight to the prop cannot hold `44` on the way to `440`. */
const draftWidth = ref(String(props.width))
const draftHeight = ref(String(props.height))

/**
 * The stored value flows back in — after a commit, and after any pull that
 * changed it — **except into a field that has focus**.
 *
 * The exception is not defensive padding. `settings-changed` fires for writes
 * this row knows nothing about: moving the panel persists `panelPosition`, and
 * the re-pull that follows would land mid-word and replace what the user was
 * typing with the stored number.
 */
watch(
	() => props.width,
	(value) => {
		if (document.activeElement !== widthField.value) draftWidth.value = String(value)
	},
)
watch(
	() => props.height,
	(value) => {
		if (document.activeElement !== heightField.value) draftHeight.value = String(value)
	},
)

/** An unreadable field falls back to the stored value rather than to the
 *  default: emptying a box and tabbing away is an abandoned edit, not a request
 *  to go back to 440. */
function read(draft: string, min: number, max: number, fallback: number) {
	const parsed = Number.parseInt(draft.trim(), 10)
	if (!Number.isFinite(parsed)) return fallback
	return Math.min(Math.max(parsed, min), max)
}

function commit() {
	const width = read(draftWidth.value, props.minWidth, props.maxWidth, props.width)
	const height = read(draftHeight.value, props.minHeight, props.maxHeight, props.height)

	// Written back before the round trip, not after it. A clamped or unparseable
	// entry produces no change to send, so the watchers above would never fire and
	// the field would sit there still showing the rejected text.
	draftWidth.value = String(width)
	draftHeight.value = String(height)

	if (width === props.width && height === props.height) return
	emit('commit', width, height)
}

/** No confirmation: a window size is not destructive, and it is shown only when
 *  there is something to undo. `ShortcutRecorder`'s Reset, in both respects. */
const canReset = computed(
	() => props.width !== props.defaultWidth || props.height !== props.defaultHeight,
)

const fieldClass = 'panel-field h-8 w-16 px-2 text-center tabular-nums'
</script>

<template>
	<div class="mt-2 flex items-center gap-2">
		<!-- `type="text"` with a numeric inputmode rather than `type="number"`: the
		     spinner buttons are a second way to change the value that would have to
		     grow their own commit behaviour, and a scroll wheel over a focused number
		     field silently edits it — inside a scrolling settings view, that is a
		     resize nobody asked for. `aria-label` rather than a visible label per
		     field, because the row's own label already says "Size" and the `×`
		     between them is what makes the pair read as one dimension. -->
		<input
			ref="widthField"
			v-model="draftWidth"
			type="text"
			inputmode="numeric"
			name="panel-width"
			autocomplete="off"
			spellcheck="false"
			aria-label="Panel width in pixels"
			:aria-invalid="errorId ? 'true' : undefined"
			:aria-describedby="errorId"
			:class="fieldClass"
			@keydown.enter.prevent="commit"
			@blur="commit"
		/>

		<span aria-hidden="true" class="text-text-secondary text-meta">×</span>

		<input
			ref="heightField"
			v-model="draftHeight"
			type="text"
			inputmode="numeric"
			name="panel-height"
			autocomplete="off"
			spellcheck="false"
			aria-label="Panel height in pixels"
			:aria-invalid="errorId ? 'true' : undefined"
			:aria-describedby="errorId"
			:class="fieldClass"
			@keydown.enter.prevent="commit"
			@blur="commit"
		/>

		<span aria-hidden="true" class="text-text-secondary text-meta">px</span>

		<!-- Pushed to the far end rather than sitting beside the fields: it carries a
		     44px hit area, and an expander that close to a 32px field would cover part
		     of the field's own edge. -->
		<button
			v-if="canReset"
			type="button"
			aria-label="Reset to the default size"
			title="Reset to the default size"
			class="icon-button hit-44 relative ml-auto"
			@click="emit('commit', defaultWidth, defaultHeight)"
		>
			<IconLucideRotateCcw class="size-4" aria-hidden="true" focusable="false" />
		</button>
	</div>
</template>
