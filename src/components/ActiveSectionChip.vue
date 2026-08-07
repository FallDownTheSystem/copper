<script setup lang="ts">
/**
 * Where the next capture lands, shown next to the thing that captures.
 *
 * Task-004 pins the composer *placeholder* to the space name and says explicitly
 * that it does not change when the active section does. That rule is upheld
 * here, not amended: the chip is an addition beside the placeholder. But a
 * feature whose whole value is "everything lands in the active section until you
 * switch" is unusable if the active section is only visible on a header row that
 * scrolls out of view, so it needs somewhere permanent to live.
 *
 * It is also the `Ctrl+K` anchor, and a button in its own right — so the
 * switcher is reachable by mouse without opening `...`.
 */

const { activeSectionObject } = useSpace()
const { boundary, portalTo } = useOverlayHost()
const { isSwitcherOpenIn, setSwitcherOpen, closeSwitcher } = useSections()

/**
 * Reka's own close-focus event, forwarded rather than consumed.
 *
 * Left alone it returns focus to this chip, which is the right answer when the
 * switcher was opened by clicking it. It is the wrong answer for `Ctrl+K`, where
 * the user was mid-sentence in the composer — so the composer takes the event
 * and calls `preventDefault()` on it only when it actually held focus at the
 * time. Deciding that here would mean this component tracking where focus came
 * from, which the composer already knows.
 */
const emit = defineEmits<{ closed: [event: Event] }>()

/** Never blank, so the chip occupies its slot at all times and activating a
 *  section shifts no layout. */
const name = computed(() => activeSectionObject.value?.name ?? 'No section')

function onOpenChange(next: boolean) {
	setSwitcherOpen('chip', next)
}
</script>

<template>
	<DropdownMenu :open="isSwitcherOpenIn('chip')" @update:open="onOpenChange">
		<DropdownMenuTrigger
			type="button"
			:aria-label="`Active section: ${name}. Switch section`"
			:title="name"
			class="text-text-secondary hover:bg-surface-hover active:bg-surface-active outline-focus-ring border-separator flex min-w-0 max-w-full items-center gap-1.5 rounded-full border px-2 py-0.5 transition-colors duration-fast focus-visible:outline-2 focus-visible:-outline-offset-1"
		>
			<span aria-hidden="true" class="bg-accent-ring size-1.5 shrink-0 rounded-full" />
			<span class="truncate text-meta">{{ name }}</span>
			<IconLucideChevronsUpDown
				class="text-text-disabled size-3 shrink-0"
				aria-hidden="true"
				focusable="false"
			/>
		</DropdownMenuTrigger>

		<DropdownMenuContent
			v-if="portalTo"
			align="start"
			side="top"
			:to="portalTo"
			:collision-boundary="boundary ?? undefined"
			:collision-padding="8"
			class="text-text-secondary w-64 max-h-(--reka-dropdown-menu-content-available-height) text-meta"
			@close-auto-focus="emit('closed', $event)"
		>
			<!-- No heading row above the list. A `menu` may own only menuitem, group,
			     separator or another menu, and reka's `DropdownMenuLabel` renders a
			     bare div — so a title here fails `aria-required-children`. It would
			     also be a third place saying the same thing: the trigger is labelled
			     "Switch section" and the field reads "Filter sections…". -->
			<SectionSwitcher @close="closeSwitcher('chip')" />
		</DropdownMenuContent>
	</DropdownMenu>
</template>
