<script setup lang="ts">
/**
 * The one switch appearance the settings surface uses, three times.
 *
 * `SwitchRoot` has no label part of its own, so the row's visible label is paired
 * to it by `for`/`id` — otherwise the control announces with no name at all,
 * which is why `id` is required here rather than left to fall through. It renders
 * `<button role="switch" aria-checked>`, toggled by both Space and Enter, which
 * is the right pattern for a system-level toggle and the idiom Windows and macOS
 * both use for exactly this setting.
 */
import { SwitchRoot, SwitchThumb } from 'reka-ui'

defineProps<{ id: string; modelValue: boolean }>()
defineEmits<{ 'update:modelValue': [value: boolean] }>()
</script>

<template>
	<SwitchRoot
		:id="id"
		:model-value="modelValue"
		class="bg-surface-hover data-[state=checked]:bg-accent-ring hit-44 relative inline-flex h-6 w-10 shrink-0 items-center rounded-full p-0.5 transition-colors duration-fast focus-visible:outline-2 focus-visible:outline-offset-2"
		@update:model-value="(value) => $emit('update:modelValue', value)"
	>
		<SwitchThumb
			class="block size-5 rounded-full bg-white shadow-sm transition-transform duration-fast data-[state=checked]:translate-x-4"
		/>
	</SwitchRoot>
</template>
