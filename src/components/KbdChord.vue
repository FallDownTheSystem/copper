<script setup lang="ts">
/**
 * One chord as key caps, with the `+` separators between them.
 *
 * Beside `KbdChip` rather than inside it: a chip is one cap and knows nothing
 * about what sits next to it, while the separator only exists *between* caps.
 * `ShortcutRecorder` renders this twice — the live binding and the chord being
 * recorded — and the two must not be able to drift apart.
 *
 * Keyed by value *and* index, because a chord can legitimately repeat a key:
 * `Shift Shift` arrives here as one chip, but nothing stops a future binding
 * from holding two of the same cap.
 */
defineProps<{ keys: readonly string[] }>()
</script>

<template>
	<template v-for="(key, index) in keys" :key="`${key}-${index}`">
		<!-- Hidden from the accessibility tree: the caps read as the chord on their
		     own, and "Ctrl plus Shift plus M" is not how anyone says it. -->
		<span v-if="index > 0" class="text-text-disabled text-meta" aria-hidden="true">+</span>
		<KbdChip :label="key" />
	</template>
</template>
