<script setup lang="ts">
const { sections } = useSpace()
const { moveTo, isRedundantTarget } = useNoteActions()
</script>

<template>
	<!-- Sections in document order, which the store has already repaired into
	     `order`. A section every targeted note already lives in is rendered and
	     disabled rather than omitted, so the list does not change shape between
	     openings. -->
	<ContextMenuItem
		v-for="section in sections"
		:key="section.id"
		:disabled="isRedundantTarget(section.id)"
		class="min-h-6"
		@select="moveTo(section.id)"
	>
		<span class="truncate">{{ section.name }}</span>
	</ContextMenuItem>
</template>
