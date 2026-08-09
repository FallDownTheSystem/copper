<script setup lang="ts">
/**
 * The command palette: one field over every space, every section of the open
 * space, and every action the settings surface offers.
 *
 * **A full-surface overlay inside the panel root, not a portalled popover.** The
 * dropdowns teleport through `useOverlayHost` because they are anchored to a
 * trigger; this has no trigger — `Ctrl+K` fires from anywhere — so it follows
 * `ImageViewer` instead: `absolute inset-0`, never `fixed`, nothing in it
 * scrolling but its own capped list, because task-004's first acceptance
 * criterion is that the document scrolls in neither axis.
 *
 * **It takes the keyboard while it is up, through `inOverlay` rather than
 * through a handler of its own.** The shell's chord layer declines every press
 * whose target is inside an overlay, and the palette's outermost element is on
 * that list — which is what stops `Delete` at an open palette from deleting the
 * notes underneath it. Escape resolves there too, so the shell's ladder is never
 * reached and the palette needs no rung on it: the same arrangement the section
 * switcher documents, for the same reason.
 *
 * **Focus is trapped and handed back.** `FocusScope` traps it; `usePalette`
 * records where it came from and returns it. Reka's own unmount auto-focus is
 * declined rather than used, because it restores with `select: true` — which on
 * the composer would select the half-typed line the user was in the middle of.
 *
 * **Three plain arrays, filtered in one synchronous `computed`.** `fuzzyMatch`
 * reuses scratch buffers and caches one needle, so it may not be made async or
 * re-entrant; and it scores rather than merely matching, which is what lets each
 * group be ranked instead of listed in whatever order it was declared.
 */
import { FocusScope } from 'reka-ui'

import { fuzzyMatch, fuzzyNeedle } from '@/lib/fuzzyMatch'
import { countMessage } from '@/composables/useStatusMessage'
import type { PaletteAction } from '@/composables/settingsActions'

const { isOpen, close } = usePalette()
const { recents, probeRecents, openSpace } = useSpaces()
const { sections, activeSection, notesInSection, setActiveSection } = useSpace()

/** Component-local, unlike the switcher's: nothing outside this file reads the
 *  palette's query, and the reset below is the whole of its lifecycle. */
const query = ref('')

const list = useTemplateRef<{ highlightFirstItem: () => void }>('list')

/**
 * The query as a folded character sequence. `fuzzyMatch` requires a needle that
 * has already been through here — a raw query silently under-matches, because
 * the needle is whitespace-stripped and case-folded once rather than per
 * comparison.
 */
const needle = computed(() => fuzzyNeedle(query.value))

/**
 * The filter, in the shape `useNoteSearch` established: **an empty needle is a
 * separate state, not a match-everything one.** `fuzzyMatch` answers `null` for
 * it, so "no query" has to be branched on rather than handed to the matcher.
 *
 * Ranked by score within the group, which is the reason for scoring at all: a
 * subsequence matcher says yes far too often, and the order is what makes the
 * answer useful. `sort` is stable, so equal scores keep the order the group
 * arrived in — Rust's recency for spaces, document order for sections.
 */
function filtered<T>(items: readonly T[], label: (item: T) => string): T[] {
	const text = needle.value
	if (text.length === 0) return [...items]

	const scored: { item: T; score: number }[] = []
	for (const item of items) {
		const match = fuzzyMatch(label(item), text)
		if (match) scored.push({ item, score: match.score })
	}
	scored.sort((a, b) => b.score - a.score)
	return scored.map((entry) => entry.item)
}

const spaceResults = computed(() => filtered(recents.value, (entry) => entry.name))
const sectionResults = computed(() => filtered(sections.value, (section) => section.name))
/**
 * Derived per evaluation rather than held in a constant, so the labels read live
 * state: `Keep on top` has to say `On` or `Off` as it is now, and a list built
 * once at import time would have captured a `settings.value` of `null`.
 */
const actionResults = computed(() =>
	filtered([...settingsActions(), ...shareActions()], (action) => action.label),
)

const empty = computed(
	() =>
		spaceResults.value.length === 0 &&
		sectionResults.value.length === 0 &&
		actionResults.value.length === 0,
)

/**
 * The open lifecycle, in the same shape `PanelMenu.onOpenChange(true)` has.
 *
 * The query is cleared on every open rather than only on close, for the reason
 * `useSections` records about the switcher: a filter that survives a dismissal
 * brings the next opening up pre-filtered.
 *
 * **Probing is started here and only here.** The rule `useSpaces` states is that
 * probes must never be kicked off by a *store event*, because probe results
 * ask for another listing and the two would drive each other in a loop. A person
 * opening the palette is not in that loop — it is exactly the trigger the `...`
 * menu uses — and listing alone is a pure read of cached availability, so
 * without this the rows would show whatever the last menu open learned.
 */
watch(isOpen, (open) => {
	if (!open) return
	query.value = ''
	void probeRecents()
	// After the `v-if` has flushed, which is when the rows exist to highlight one
	// of. Reka does this for itself on every keystroke and never before the first.
	void nextTick(() => list.value?.highlightFirstItem())
})

/** Spoken rather than shown, exactly as the switcher and the chip do it: a bare
 *  numeral beside a name is unambiguous to look at and means nothing read
 *  aloud. */
function spokenCount(sectionId: string) {
	return countMessage(notesInSection(sectionId).length, {
		one: '1 note',
		many: (total) => `${total} notes`,
	})
}

/**
 * Every selection closes first and acts second.
 *
 * Closing first is what makes the palette feel like a launcher rather than a
 * dialog waiting on a round trip, and none of the three actions needs the
 * overlay to report: a refused space switch or section switch lands in the
 * panel's action-error band, which is on screen behind the palette that has just
 * gone. That is the opposite of `SectionSwitcher.choose`, which stays open on a
 * refusal — it has a filter field the user was mid-thought in, and this does not
 * once it has closed.
 */
function chooseSpace(path: string) {
	close()
	void openSpace(path)
}

function chooseSection(id: string) {
	close()
	void setActiveSection(id)
}

function runAction(action: PaletteAction) {
	close()
	void action.run()
}
</script>

<template>
	<!-- `data-slot` is the hook `inOverlay` matches on, and it belongs on the
	     outermost element so that a press anywhere inside — including one that
	     landed on this container rather than on the field — resolves there. -->
	<div
		v-if="isOpen"
		data-slot="command-overlay"
		role="dialog"
		aria-modal="true"
		aria-label="Command palette"
		class="bg-surface/75 absolute inset-0 z-35 flex items-start justify-center px-3 pt-10"
		@click.self="close"
		@keydown.escape="close"
	>
		<!-- Two focusables in principle — the field and the list — so `ImageViewer`'s
		     `@keydown.tab.prevent` would not be a complete trap here. `FocusScope`
		     also moves focus in on mount, which is the half AC-2 needs at the other
		     end. Its unmount half is declined: `usePalette` owns the return, and
		     reka's own restore selects the text of whatever it focuses. -->
		<FocusScope as-child trapped @unmount-auto-focus.prevent>
			<Command
				ref="list"
				highlight-on-hover
				class="bg-popover text-popover-foreground squircle ring-foreground/10 h-fit max-h-full w-full max-w-96 rounded-lg p-1 shadow-md ring-1 dark:shadow-black/40"
			>
				<div class="p-1">
					<label for="command-filter" class="sr-only">Search spaces, sections and actions</label>
					<CommandInput
						id="command-filter"
						v-model="query"
						placeholder="Search spaces, sections and actions…"
					/>
				</div>

				<!-- `role="listbox"` is an ARIA input role and has to be named: the field
				     above it is labelled for the person typing, and this is what the
				     results themselves are announced as. -->
				<CommandList aria-label="Results">
					<!-- Each group is present only when it has something in it, which with
					     three plain arrays is a `v-if` rather than the registration
					     protocol a scraping filter would have needed. -->
					<CommandGroup v-if="spaceResults.length > 0">
						<CommandGroupLabel>Spaces</CommandGroupLabel>
						<!-- Keyed and valued by path rather than by the comparison key: that
						     key is many-to-one over stored paths by design, and the store
						     dedupes recents by resolved path. -->
						<CommandItem
							v-for="entry in spaceResults"
							:key="entry.path"
							:value="`space:${entry.path}`"
							:aria-current="entry.active ? 'true' : undefined"
							@select="chooseSpace(entry.path)"
						>
							<ActiveMarker :active="entry.active" label="active space">
								<span
									class="min-w-0 flex-1 truncate"
									:class="[
										entry.active ? 'text-accent-text font-semibold' : 'text-text-primary',
										entry.availability.state === 'unavailable' ? 'text-text-disabled' : '',
									]"
								>
									{{ entry.name }}
								</span>
							</ActiveMarker>
						</CommandItem>
					</CommandGroup>

					<CommandGroup v-if="sectionResults.length > 0">
						<CommandGroupLabel>Sections</CommandGroupLabel>
						<CommandItem
							v-for="section in sectionResults"
							:key="section.id"
							:value="`section:${section.id}`"
							:data-section-id="section.id"
							:aria-current="section.id === activeSection ? 'true' : undefined"
							@select="chooseSection(section.id)"
						>
							<ActiveMarker :active="section.id === activeSection" label="active section">
								<span
									class="min-w-0 flex-1 truncate"
									:class="
										section.id === activeSection
											? 'text-accent-text font-semibold'
											: 'text-text-primary'
									"
								>
									{{ section.name }}
								</span>
							</ActiveMarker>

							<!-- How much is in each destination, which is what makes one worth
							     picking over another. `tabular-nums` so a column of counts lines
							     up on its digits. -->
							<span aria-hidden="true" class="text-text-secondary shrink-0 tabular-nums">
								{{ notesInSection(section.id).length }}
							</span>
							<span class="sr-only">{{ spokenCount(section.id) }}</span>
						</CommandItem>
					</CommandGroup>

					<CommandGroup v-if="actionResults.length > 0">
						<CommandGroupLabel>Actions</CommandGroupLabel>
						<CommandItem
							v-for="action in actionResults"
							:key="action.id"
							:value="`action:${action.id}`"
							:data-action-id="action.id"
							@select="runAction(action)"
						>
							<span class="text-text-primary min-w-0 flex-1 truncate">{{ action.label }}</span>
							<span v-if="action.value" class="text-text-secondary shrink-0">
								{{ action.value }}
							</span>
						</CommandItem>
					</CommandGroup>

					<!-- A palette that filtered everything away has to say so. Without it
					     the card collapses to a field over nothing, which reads as a
					     surface that has broken rather than one with no answer. -->
					<p v-if="empty" class="text-text-secondary px-2 py-6 text-center text-meta">
						Nothing matches “{{ query }}”.
					</p>
				</CommandList>
			</Command>
		</FocusScope>
	</div>
</template>
