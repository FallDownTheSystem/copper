<script setup lang="ts">
// The drag region and the panel radius moved into PanelShell, which owns the
// three regions and the clip. Task-002 placed both here when this was the whole
// panel; task-008 depends on the narrowed drag-region placement.
useTheme()
</script>

<template>
	<PanelShell />
</template>
