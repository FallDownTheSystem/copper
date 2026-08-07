<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/utils'

const props = defineProps<{
	class?: HTMLAttributes['class']
}>()
</script>

<template>
	<span
		data-slot="context-menu-shortcut"
		:class="
			cn(
				// `shrink-0`: at the 50vw ceiling the row can run out of width, and the
				// half that must not give is the chord — a label wrapping is legible,
				// `Ctrl+Shift…` is not.
				'text-muted-foreground group-focus/context-menu-item:text-accent-foreground ml-auto shrink-0 text-meta',
				props.class,
			)
		"
	>
		<slot />
	</span>
</template>
