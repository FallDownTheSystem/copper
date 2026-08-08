<script setup lang="ts">
/**
 * The chip under the search field: what the panel is currently pointed at, how
 * much is in it, and the control that points it somewhere else.
 *
 * It began beside the composer, on the reasoning that "where the next capture
 * lands" belongs next to the thing that captures. It reads better here: the
 * active section is what the list below is *of*, and a label above a list is
 * where a reader already looks for that. It is still the same control — the same
 * switcher — and still a button in its own right, so the switcher is reachable
 * by mouse without opening `...`. It was also the `Ctrl+K` anchor until task-019
 * gave that chord to the command palette; the pointer route stayed, because the
 * palette absorbs section *switching* and not section *creation*.
 *
 * Task-004 pins the composer *placeholder* to the space name and says explicitly
 * that it does not change when the active section does. That rule is upheld here,
 * not amended: this is a separate surface, and the placeholder still names the
 * space.
 *
 * **The count is the section's, not the list's.** It counts what the document
 * holds, so an active search does not make the destination look emptier than it
 * is — this says where a capture will land and what is already there, which is a
 * different question from what is currently on screen.
 *
 * Deliberately not an `<h2>`. The list's own section headers already are, and a
 * second heading carrying the active section's name would put the same text in
 * the document outline twice with nothing between them to distinguish it.
 *
 * The space this all sits in is switched from the `...` menu and nowhere else:
 * sections and spaces are different scopes, and one menu offering both is one
 * menu in which the wrong row is easy to hit.
 */

import { countMessage } from '@/composables/useStatusMessage'

const { activeSectionObject, activeSection, notesInSection } = useSpace()
const { boundary, portalTo } = useOverlayHost()
const { isSwitcherOpenIn, setSwitcherOpen, closeSwitcher } = useSections()

/**
 * Reka's own close-focus event, forwarded rather than consumed.
 *
 * Left alone it returns focus to this trigger, which is the right answer when the
 * switcher was opened by clicking it. It is the wrong answer whenever the user
 * was mid-sentence in the composer — so the composer takes the event and calls
 * `preventDefault()` on it only when it actually held focus at the time. Deciding
 * that here would mean this component tracking where focus came from, which the
 * composer already knows.
 */
const emit = defineEmits<{ closed: [event: Event] }>()

/** Never blank, so the chip occupies its slot at all times and activating a
 *  section shifts no layout. */
const name = computed(() => activeSectionObject.value?.name ?? 'No section')

const count = computed(() => {
	const id = activeSection.value
	return id === null ? 0 : notesInSection(id).length
})

/** Spoken rather than shown: the bare numeral beside the name is unambiguous to
 *  a reader looking at it and means nothing read aloud on its own. */
const spokenCount = computed(() =>
	countMessage(count.value, { one: '1 note', many: (total) => `${total} notes` }),
)

function onOpenChange(next: boolean) {
	setSwitcherOpen('chip', next)
}
</script>

<template>
	<DropdownMenu :open="isSwitcherOpenIn('chip')" @update:open="onOpenChange">
		<!-- The accent colour and the weight are the list's own active-section
		     header, so the two say the same thing the same way.

		     **No negative margin, and that is the alignment.** It used to carry
		     `-mx-1` so the hover surface could breathe past the chip's text, which
		     bought 4px of padding at the cost of hanging the chip's whole box 4px
		     left of the search field directly above it — two stacked controls in one
		     column with two different left edges, which is the one thing a reader
		     comparing them cannot unsee. The field's edge wins: it is the wider
		     object and the one the header's `px-3` was set for.

		     `rounded-inset` rather than any step of the surface ramp: a `text-label`
		     line inside `py-0.5` is about 21px tall, so half its height is ~10px and
		     every step on that ramp — `sm` included, now that it is 12px — lands past
		     the capsule threshold. The chip would stop reading as the same kind of
		     object as the section header it mirrors. -->
		<DropdownMenuTrigger
			type="button"
			:aria-label="`Active section: ${name}, ${spokenCount}. Switch section`"
			:title="name"
			class="text-accent-text hover:bg-surface-hover active:bg-surface-active focus-ring squircle border-separator flex min-w-0 max-w-full items-center gap-1.5 rounded-inset border px-1.5 py-0.5 transition-colors duration-fast"
		>
			<IconLucideListTree class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
			<span class="truncate text-label font-semibold uppercase">{{ name }}</span>
			<!-- `tabular-nums` so the chip does not change width as the count ticks
			     between digits of the same length. -->
			<span aria-hidden="true" class="text-text-secondary shrink-0 text-meta tabular-nums">
				{{ count }}
			</span>
			<IconLucideChevronDown
				class="text-text-disabled size-3.5 shrink-0"
				aria-hidden="true"
				focusable="false"
			/>
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
			     "Switch section" and the field reads "Filter or create a section…". -->
			<SectionSwitcher @close="closeSwitcher('chip')" />
		</DropdownMenuContent>
	</DropdownMenu>
</template>
