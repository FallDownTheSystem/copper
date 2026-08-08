<script setup lang="ts">
/**
 * The switcher's contents: a field, the active space's sections with their note
 * counts, and a way to create one that does not exist yet.
 *
 * **The body only, not the menu around it** — exactly as `MoveToSubmenu` renders
 * items for whichever content element hosts it. There are two entry points, and
 * one list: the chip under the search field opens it as a dropdown, and
 * `Switch section ▸` opens it as a submenu of `...`. Building a second list for
 * the second trigger is how they would come to disagree.
 *
 * Both are pointer routes. `Ctrl+K` was the third until task-019, which gave the
 * chord to the command palette — and this survived the takeover because the
 * palette absorbs *switching* and not *creating*: a palette that hides empty
 * groups has nowhere to put the create row below, and without that row a section
 * could only be made from `...` → `New section…` or a `# Name` directive.
 *
 * **One field does the filtering and the creating**, which is why it is named for
 * both. A second, dedicated "new section" input at the top would fork the
 * keyboard path — two places for `Enter` to mean something — and duplicate a
 * creation route that already exists. Typing a name nothing matches turns the
 * list into the offer to create it, so the field a user types a new name into is
 * the field that was already there.
 *
 * It lists **all** the active space's sections regardless of the search query,
 * and counts what the document holds rather than what the query left on screen.
 * It is a destination picker, not a view of the filtered list.
 *
 * Spaces are not switched from here. That lives behind `...`, because a section
 * and a space are different scopes and one menu offering both is one menu in
 * which the wrong row is easy to hit.
 */

import { isComposing } from '@/lib/chords'
import { normaliseSectionName } from '@/lib/sectionName'
import { countMessage } from '@/composables/useStatusMessage'

/**
 * Emitted once an action has actually succeeded, so the host closes itself.
 *
 * The host closes, not this component: the two entry points are closed by
 * different mechanisms — one is a controlled `open` ref, the other is reka's own
 * submenu state — and a component that reached for either would only work in one
 * of them.
 */
const emit = defineEmits<{ close: [] }>()

const { sections, activeSection, notesInSection, submitEntry, setActiveSection } = useSpace()
const { filterQuery } = useSections()

/** Spoken rather than shown: the bare numeral beside a name is unambiguous to a
 *  reader looking at it and means nothing read aloud on its own. */
function spokenCount(sectionId: string) {
	return countMessage(notesInSection(sectionId).length, {
		one: '1 note',
		many: (total) => `${total} notes`,
	})
}

const input = useTemplateRef<HTMLInputElement>('input')

/** The field is the one thing here that did not exist a tick ago, so it takes
 *  focus itself — reka's own open-focus lands on the first item. */
onMounted(() => {
	void nextTick(() => input.value?.focus())
})

/**
 * The query as the **store** would read it, not as it was typed.
 *
 * Both the filter and the create row use it, and they have to: the store
 * collapses whitespace and caps at 80, so filtering on the raw text made
 * `Deep  Research` miss the existing `Deep Research`, offer to create it, and
 * then — because the store resolves the duplicate — silently activate the
 * existing section instead of creating anything. Normalising here closes the gap
 * between what the row promises and what the store does.
 */
const query = computed(() => normaliseSectionName(filterQuery.value))

const matches = computed(() => {
	const text = query.value.toLowerCase()
	if (text.length === 0) return sections.value
	return sections.value.filter((section) =>
		normaliseSectionName(section.name).toLowerCase().includes(text),
	)
})

/**
 * Activation is not optimistic: `set_active_section` returns the updated
 * document and the coordinator applies it like any other mutation, so a failure
 * leaves the previous section active and surfaces through the action-error band.
 * The switcher stays open in that case, with the failure still on screen.
 */
async function choose(id: string) {
	const result = await setActiveSection(id)
	if (result) emit('close')
}

/**
 * The empty-result row, routed through the **same** path as a `# Name`
 * directive: `submit_entry` classifies the string in Rust, so this inherits the
 * duplicate-name rule, the whitespace collapsing and the 80-character cap
 * without a second copy of any of them. A frontend that called `add_section`
 * directly would create a second `Research` next to the existing one.
 */
async function create() {
	const name = query.value
	if (name.length === 0) return
	const result = await submitEntry(`# ${name}`)
	if (result) emit('close')
}

/**
 * The row reka has highlighted, which is not always the first one.
 *
 * Reka highlights on hover and on arrow keys while the filter field keeps focus,
 * so `Enter` has to resolve *that* row or it activates something the user is not
 * pointing at. Read off the DOM because the highlight is reka's state, not ours —
 * asking it is the only way not to keep a second copy that drifts.
 */
function highlighted(): HTMLElement | null {
	const host = input.value?.closest<HTMLElement>(
		'[data-slot="dropdown-menu-content"], [data-slot="dropdown-menu-sub-content"]',
	)
	return host?.querySelector<HTMLElement>('[role="menuitem"][data-highlighted]') ?? null
}

/**
 * The field sits inside a reka menu, which owns arrows, Home/End, Escape and
 * typeahead. Only the presses reka should still act on are let through — the
 * allowlist at the bottom, Escape among them, because closing is reka's job and
 * not this component's. Everything else is stopped, so typeahead cannot steal
 * focus onto an item mid-word; Enter is resolved here against the highlighted
 * row, and ArrowLeft belongs to the caret whenever there is text to move over.
 */
function onKeydown(event: KeyboardEvent) {
	// Stopped rather than merely ignored: the press has to be withheld from
	// reka's own typeahead as well, or composing a name steals focus onto a row.
	if (isComposing(event)) {
		event.stopPropagation()
		return
	}

	if (event.key === 'Enter') {
		event.preventDefault()
		event.stopPropagation()
		const row = highlighted()
		const sectionId = row?.dataset.sectionId
		if (sectionId !== undefined) void choose(sectionId)
		else if (row?.hasAttribute('data-create-row')) void create()
		// Nothing highlighted — the user typed and pressed Enter without ever
		// leaving the field — so the first row is the one they meant.
		else if (matches.value[0]) void choose(matches.value[0].id)
		else void create()
		return
	}

	if (event.key === 'ArrowLeft') {
		// The submenu's close key *and* the caret key. It can only be one of them at
		// a time, and the caret decides: at the very start of an empty selection
		// there is nothing to move left over, so the press belongs to reka.
		const field = event.target as HTMLInputElement
		if (field.selectionStart === 0 && field.selectionEnd === 0) return
		event.stopPropagation()
		return
	}

	if (['ArrowDown', 'ArrowUp', 'Home', 'End', 'Escape', 'Tab'].includes(event.key)) return
	event.stopPropagation()
}
</script>

<template>
	<!-- **A filter field inside a `menu` is invalid ARIA, and knowingly so.**
	     `role="menu"` may own only menuitem, group, separator or another menu, and
	     a `group` wrapper does not help: axe flattens groups inside menus, finds
	     the focusable field underneath, and reports `aria-required-children` on the
	     content element. There is no markup that makes a textbox a legal child of a
	     menu — the ARIA-correct surface for "filter a list in a popover" is a
	     combobox, which is a different primitive from the dropdown this task
	     specifies. The finding is one node on reka's own content element, the field
	     is labelled and fully operable by keyboard, and the switcher's axe
	     assertion names the rule and this reason rather than passing quietly. -->
	<!-- `p-1` rather than `px-1 pb-1`: the content element's own `p-1` is the same
	     4px on every side, so leaving the top off here left the field 4px below the
	     menu's edge and 8px in from its sides. -->
	<div class="p-1">
		<!-- Named for both jobs, because it does both: what it filters to nothing it
		     offers to create. -->
		<label for="section-filter" class="sr-only">Filter or create a section</label>
		<input
			id="section-filter"
			ref="input"
			v-model="filterQuery"
			type="text"
			autocomplete="off"
			placeholder="Filter or create a section…"
			class="panel-field h-7 w-full min-w-0 px-1.5"
			@keydown="onKeydown"
		/>
	</div>

	<!-- Capped and scrolled internally, so twenty sections cannot outgrow the
	     fixed panel or push the menu outside its rounded clip.

	     `px-1 pb-1` puts the rows' own 8px inset on the same two edges the field
	     already sits on — the item's highlight pill is the box being aligned, not
	     its text — and leaves the menu the bottom breathing room its top has.

	     **`scrollbar-gutter: auto` here, against `.thin-scrollbar`'s `stable`,**
	     for the reason `PanelShell`'s scroll region makes the same override: the
	     reserved gutter is permanent, so a list of three sections would still sit a
	     scrollbar's width short of the field above it. Trading a one-time shift,
	     when a list past `max-h-52` grows a real scrollbar, against a right edge
	     that never lines up. -->
	<div
		class="thin-scrollbar max-h-52 overflow-y-auto overflow-x-hidden px-1 pb-1 [scrollbar-gutter:auto]"
	>
		<!-- `@select.prevent` on both rows: reka closes on select by default, and a
		     refused activation has to leave the switcher standing with the failure
		     visible. Closing is the `close` emit's job, and only on success. -->
		<DropdownMenuItem
			v-for="section in matches"
			:key="section.id"
			:data-section-id="section.id"
			class="min-h-6"
			:aria-current="section.id === activeSection ? 'true' : undefined"
			@select.prevent="choose(section.id)"
		>
			<ActiveMarker :active="section.id === activeSection" label="active section">
				<span
					class="min-w-0 flex-1 truncate"
					:class="
						section.id === activeSection ? 'text-accent-text font-semibold' : 'text-text-primary'
					"
				>
					{{ section.name }}
				</span>
			</ActiveMarker>

			<!-- How much is in each destination, which is the thing that makes one
			     worth picking over another. `tabular-nums` so a column of counts lines
			     up on its digits. -->
			<span aria-hidden="true" class="text-text-secondary shrink-0 tabular-nums">
				{{ notesInSection(section.id).length }}
			</span>
			<span class="sr-only">{{ spokenCount(section.id) }}</span>
		</DropdownMenuItem>

		<DropdownMenuItem
			v-if="matches.length === 0 && query.length > 0"
			data-create-row
			class="min-h-6"
			@select.prevent="create"
		>
			<IconLucidePlus class="size-4 shrink-0" aria-hidden="true" focusable="false" />
			<span class="min-w-0 flex-1 truncate">Create section “{{ query }}”</span>
		</DropdownMenuItem>
	</div>
</template>
