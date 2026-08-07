<script setup lang="ts">
/**
 * One key cap. Presentational only.
 *
 * `Super` is the parser's word for the Windows key and is what `settings.json`
 * stores — writing `Win` there would produce a file the app itself could not read
 * back. The relabelling belongs here, at the one place a person sees it.
 *
 * Ring rather than a solid border below: an alpha edge adapts to whatever shows
 * through the translucent window, and a solid hex does not. The inset highlight
 * gives the cap its slight lift without a shadow, which would read as elevation
 * this control does not have. (Said here rather than above the element, because a
 * comment before a template's root makes the component a fragment.)
 *
 * **Pinned off the ramp at 8px**, which is what `--radius-sm` used to be and the
 * one place raising `--radius` had to be refused. The comment here already named
 * the failure before it happened: the cap is `h-6`, and a 12px radius on a 24px
 * box is a capsule *exactly* — a key cap that has lost its corners no longer looks
 * like a key. Written as a literal for the reason `Checkbox` writes its 6px as
 * one: the value is a property of this box's height, not a step on a scale, so
 * following the scale is precisely what would break it.
 */
const props = defineProps<{ label: string }>()

const shown = computed(() => (props.label === 'Super' ? 'Win' : props.label))
</script>

<template>
	<kbd
		class="bg-kbd-surface ring-kbd-ring text-text-primary inline-flex h-6 items-center rounded-[8px] px-1.5 font-mono text-meta ring-1 shadow-[inset_0_1px_0_var(--kbd-highlight)]"
	>
		{{ shown }}
	</kbd>
</template>
