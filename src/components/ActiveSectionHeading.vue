<script setup lang="ts">
/**
 * The heading under the search field: what the panel is currently pointed at,
 * and the control that points it somewhere else.
 *
 * It began life as a chip beside the composer, on the reasoning that "where the
 * next capture lands" belongs next to the thing that captures. It reads better as
 * a heading: the active section is what the list below is *of*, and a label above
 * a list is where a reader already looks for that. It is still the same control —
 * the same switcher, the same `Ctrl+K` anchor — and still a button in its own
 * right, so the switcher is reachable by mouse without opening `...`.
 *
 * Task-004 pins the composer *placeholder* to the space name and says explicitly
 * that it does not change when the active section does. That rule is upheld here,
 * not amended: this is a separate surface, and the placeholder still names the
 * space.
 *
 * Deliberately not an `<h2>`. The list's own section headers already are, and a
 * second heading carrying the active section's name would put the same text in
 * the document outline twice with nothing between them to distinguish it.
 */

const { activeSectionObject } = useSpace()
const { boundary, portalTo } = useOverlayHost()
const { isSwitcherOpenIn, setSwitcherOpen, closeSwitcher } = useSections()

/**
 * Reka's own close-focus event, forwarded rather than consumed.
 *
 * Left alone it returns focus to this trigger, which is the right answer when the
 * switcher was opened by clicking it. It is the wrong answer for `Ctrl+K`, where
 * the user was mid-sentence in the composer — so the composer takes the event and
 * calls `preventDefault()` on it only when it actually held focus at the time.
 * Deciding that here would mean this component tracking where focus came from,
 * which the composer already knows.
 */
const emit = defineEmits<{ closed: [event: Event] }>()

/** Never blank, so the heading occupies its slot at all times and activating a
 *  section shifts no layout. */
const name = computed(() => activeSectionObject.value?.name ?? 'No section')

function onOpenChange(next: boolean) {
	setSwitcherOpen('chip', next)
}
</script>

<template>
	<DropdownMenu :open="isSwitcherOpenIn('chip')" @update:open="onOpenChange">
		<!-- The accent colour and the weight are the list's own active-section
		     header, so the two say the same thing the same way. The negative margin
		     lets the hover surface breathe without the text losing its alignment with
		     the search field above it. -->
		<DropdownMenuTrigger
			type="button"
			:aria-label="`Active section: ${name}. Switch section`"
			:title="name"
			class="text-accent-text hover:bg-surface-hover active:bg-surface-active outline-focus-ring -mx-1 flex min-w-0 max-w-full items-center gap-1 rounded-md px-1 py-0.5 transition-colors duration-fast focus-visible:outline-2 focus-visible:-outline-offset-1"
		>
			<span class="truncate text-label font-semibold uppercase">{{ name }}</span>
			<IconLucideChevronDown class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
		</DropdownMenuTrigger>

		<!-- Opens downward now that it sits at the top of the panel rather than
		     above the composer. -->
		<DropdownMenuContent
			v-if="portalTo"
			align="start"
			side="bottom"
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
