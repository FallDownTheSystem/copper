<script setup lang="ts">
// The drag region and the panel radius belong to PanelShell, which owns the
// three regions and the clip; task-008 depends on that narrower drag region.
useTheme()
</script>

<template>
	<PanelShell />
</template>
