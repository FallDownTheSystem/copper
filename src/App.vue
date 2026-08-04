<script setup lang="ts"></script>

<template>
	<!-- Panel root only. No layout regions, list or composer — those are task-004.
	     The rounding here matches DWM's DWMWCP_ROUND rather than producing it: the
	     Acrylic/Mica material is painted by DWM across the whole HWND behind the
	     webview, so CSS radius can only avoid squaring the corners on top of it.
	     No background utility — an opaque one would hide the native backdrop. -->
	<div
		data-tauri-drag-region
		class="h-screen w-screen overflow-hidden rounded-2xl font-sans text-foreground"
	/>
</template>
