<script setup lang="ts">
/**
 * The switcher's contents: a filter field, the active space's sections, and a
 * way to create one that does not exist yet.
 *
 * **The body only, not the menu around it** — exactly as `MoveToSubmenu` renders
 * items for whichever content element hosts it. There are two entry points, and
 * one list: `Ctrl+K` opens it as a dropdown anchored on the composer's chip, and
 * `Switch section ▸` opens it as a submenu of `...`. Building a second list for
 * the second trigger is how they would come to disagree.
 *
 * It lists **all** the active space's sections regardless of the search query.
 * It is a destination picker, not a view of the filtered list.
 */

/**
 * Emitted once an action has actually succeeded, so the host closes itself.
 *
 * The host closes, not this component: the two entry points are closed by
 * different mechanisms — one is a controlled `open` ref, the other is reka's own
 * submenu state — and a component that reached for either would only work in one
 * of them.
 */
const emit = defineEmits<{ close: [] }>()

const { sections, activeSection, submitEntry, setActiveSection } = useSpace()
const { filterQuery } = useSections()

const input = useTemplateRef<HTMLInputElement>('input')

/** The field is the one thing here that did not exist a tick ago, so it takes
 *  focus itself — reka's own open-focus lands on the first item. */
onMounted(() => {
	void nextTick(() => input.value?.focus())
})

const query = computed(() => filterQuery.value.trim())

const matches = computed(() => {
	const text = query.value.toLowerCase()
	if (text.length === 0) return sections.value
	return sections.value.filter((section) => section.name.toLowerCase().includes(text))
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
 * The field sits inside a reka menu, which owns arrows, Home/End, Escape and
 * typeahead. Only the keys that would otherwise be taken from a text input are
 * stopped here: every letter, so typeahead cannot steal focus onto an item
 * mid-word, and Enter, which activates the first row rather than submitting a
 * form that does not exist. Everything else — including Escape, which reka
 * closes on — is left to bubble.
 */
function onKeydown(event: KeyboardEvent) {
	// WebView2 reports keyCode 229 while an IME candidate is open; accepting one
	// with Enter must not choose a section.
	if (event.isComposing || event.keyCode === 229) {
		event.stopPropagation()
		return
	}

	if (event.key === 'Enter') {
		event.preventDefault()
		event.stopPropagation()
		const first = matches.value[0]
		if (first) void choose(first.id)
		else void create()
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
	<div class="px-1 pb-1">
		<label for="section-filter" class="sr-only">Filter sections</label>
		<input
			id="section-filter"
			ref="input"
			v-model="filterQuery"
			type="text"
			autocomplete="off"
			placeholder="Filter sections…"
			class="border-separator bg-surface-hover text-text-primary placeholder:text-text-disabled outline-focus-ring h-7 w-full min-w-0 select-text rounded-md border px-1.5 text-body focus-visible:outline-2 focus-visible:-outline-offset-1"
			@keydown="onKeydown"
		/>
	</div>

	<!-- Capped and scrolled internally, so twenty sections cannot outgrow the
	     fixed panel or push the menu outside its rounded clip. -->
	<div class="thin-scrollbar max-h-52 overflow-y-auto overflow-x-hidden">
		<!-- `@select.prevent` on both rows: reka closes on select by default, and a
		     refused activation has to leave the switcher standing with the failure
		     visible. Closing is the `close` emit's job, and only on success. -->
		<DropdownMenuItem
			v-for="section in matches"
			:key="section.id"
			class="min-h-6"
			:aria-current="section.id === activeSection ? 'true' : undefined"
			@select.prevent="choose(section.id)"
		>
			<!-- Fixed-width slot, hidden rather than absent, so activating a section
			     shifts no text. -->
			<span
				aria-hidden="true"
				class="bg-accent-ring size-1.5 shrink-0 rounded-full"
				:class="section.id === activeSection ? 'opacity-100' : 'opacity-0'"
			/>
			<span
				class="min-w-0 flex-1 truncate"
				:class="
					section.id === activeSection ? 'text-accent-text font-semibold' : 'text-text-primary'
				"
			>
				{{ section.name }}
			</span>
			<!-- The non-colour half of the cue: colour alone would carry the whole
			     distinction. -->
			<span v-if="section.id === activeSection" class="sr-only">(active section)</span>
		</DropdownMenuItem>

		<DropdownMenuItem
			v-if="matches.length === 0 && query.length > 0"
			class="min-h-6"
			@select.prevent="create"
		>
			<IconLucidePlus class="size-4 shrink-0" aria-hidden="true" focusable="false" />
			<span class="min-w-0 flex-1 truncate">Create section “{{ query }}”</span>
		</DropdownMenuItem>
	</div>
</template>
