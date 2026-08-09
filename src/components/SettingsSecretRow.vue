<script setup lang="ts">
/**
 * A write-only credential field, with two mutually exclusive states.
 *
 * **It has no prop for a current value and no way to display one.** That is the
 * whole design: Rust never sends a stored secret back, so this component takes a
 * `set` boolean and nothing else. While a value is stored it renders a fixed run
 * of mask dots — a literal, not the value, with a length that deliberately does
 * not echo the real one — beside the only action that applies, **Clear**. While
 * nothing is stored it renders an empty input beside **Set**. There is no state
 * in which an input and a stored value coexist, so there is nothing to disable
 * and no code path that could render a secret.
 *
 * **A dirty flag guards the blur.** Without it, tabbing through an untouched
 * empty field would emit `''` and clear a stored credential the user never
 * meant to touch — the field is empty by construction, so "empty on blur" is the
 * normal state rather than a request. Clearing is the explicit **Clear** button,
 * which emits `null`.
 */
defineProps<{
	/** Whether a value is stored. Never the value. */
	set: boolean
	label: string
	placeholder?: string
	errorId?: string
}>()

const emit = defineEmits<{
	/** A string sets the value; `null` clears it. */
	commit: [value: string | null]
}>()

const draft = ref('')
/** Whether the user has typed in this field since it was last committed. */
const dirty = ref(false)

function commit() {
	if (!dirty.value) return
	const value = draft.value.trim()
	// An edit that ends up empty is an abandoned edit, not a clear. Clearing has
	// its own button.
	if (value === '') {
		draft.value = ''
		dirty.value = false
		return
	}
	// Emptied immediately, so the typed value does not sit in the DOM after it has
	// been handed over. It is only ever a keystroke away from being retyped, and
	// this is a credential.
	draft.value = ''
	dirty.value = false
	emit('commit', value)
}

function clear() {
	draft.value = ''
	dirty.value = false
	emit('commit', null)
}
</script>

<template>
	<div class="mt-2 flex items-center gap-2">
		<template v-if="set">
			<div
				class="panel-field text-text-secondary flex h-8 min-w-0 flex-1 select-none items-center px-2 text-meta"
				aria-hidden="true"
			>
				••••••••••••••••
			</div>
			<span class="sr-only" role="status">{{ label }} is set</span>
			<button
				type="button"
				class="panel-button hit-44 relative h-8 shrink-0 px-2 text-meta"
				@click="clear"
			>
				Clear
			</button>
		</template>

		<template v-else>
			<input
				v-model="draft"
				type="password"
				autocomplete="off"
				autocapitalize="off"
				autocorrect="off"
				spellcheck="false"
				:placeholder="placeholder"
				:aria-label="label"
				:aria-invalid="errorId ? 'true' : undefined"
				:aria-describedby="errorId"
				class="panel-field h-8 min-w-0 flex-1 px-2 text-meta"
				@input="dirty = true"
				@keydown.enter.prevent="commit"
				@blur="commit"
			/>

			<!-- The blur above already commits; this button is the visible promise that
			     it happens. Disabled while there is nothing to hand over, like the mask
			     state's Clear-only: a control that cannot act is not offered as one that
			     can. -->
			<button
				type="button"
				class="panel-button hit-44 relative h-8 shrink-0 px-2 text-meta"
				:disabled="draft.trim() === ''"
				@click="commit"
			>
				Set
			</button>
		</template>
	</div>
</template>
