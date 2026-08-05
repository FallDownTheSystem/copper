<script setup lang="ts">
/**
 * The `...` menu ships empty at the end of Phase 3, deliberately.
 *
 * "New section" moved to task-007, where it is designed as an inline-input flow
 * alongside the other Spaces features (planning ruling R-Q5). Placeholder or
 * disabled items for work that has not been designed yet are worse than an empty
 * menu — they promise an action and then refuse it.
 */

defineProps<{
	/** The panel root, so the menu is bounded by the window it lives in. */
	boundary: HTMLElement | null
	/** The in-panel portal host. Reka teleports to document.body otherwise,
	 *  which escapes the panel root's clip and its rounded rect. */
	portalTo: HTMLElement | null
}>()
</script>

<template>
	<DropdownMenu>
		<DropdownMenuTrigger
			aria-label="More actions"
			class="text-text-secondary hover:bg-surface-hover active:bg-surface-active outline-focus-ring grid size-8 shrink-0 place-items-center rounded-md transition-colors duration-fast focus-visible:outline-2 focus-visible:-outline-offset-1"
		>
			<IconLucideEllipsis class="size-4" aria-hidden="true" focusable="false" />
		</DropdownMenuTrigger>

		<DropdownMenuContent
			v-if="portalTo"
			align="end"
			:to="portalTo"
			:collision-boundary="boundary ?? undefined"
			:collision-padding="8"
			class="text-text-secondary w-48 max-h-(--reka-dropdown-menu-content-available-height) text-meta"
		>
			<p class="px-2 py-1.5">Nothing here yet.</p>
		</DropdownMenuContent>
	</DropdownMenu>
</template>
