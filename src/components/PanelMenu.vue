<script setup lang="ts">
/**
 * The `...` menu: the spaces switcher, the two space actions, and the inline
 * section-creation field.
 *
 * The switcher is a section of this menu rather than a third view. The design
 * puts space switching behind `...`, the recents list is short, and the panel
 * stays single-view until Phase 7 adds settings.
 *
 * **Switching is always an explicit user choice.** Nothing here infers or
 * auto-switches — including at startup, where the store may already have
 * re-pointed to a loadable entry and this renders whatever is actually active.
 */

defineProps<{
	/** The panel root, so the menu is bounded by the window it lives in. */
	boundary: HTMLElement | null
	/** The in-panel portal host. Reka teleports to document.body otherwise,
	 *  which escapes the panel root's clip and its rounded rect. */
	portalTo: HTMLElement | null
}>()

const { recents, probeRecents, openSpace, pickAndOpenSpace, createSpace, removeRecent } =
	useSpaces()
const { sections, addSection } = useSpace()

const open = ref(false)
const creatingSection = ref(false)
const draft = ref('')
const sectionError = ref<string | null>(null)

const input = useTemplateRef<HTMLInputElement>('input')

/**
 * Probing starts here and nowhere else. Listing recents is a pure read of
 * cached state, so opening the menu is what makes the answers current — and the
 * list paints immediately either way, with unresolved rows in their pending
 * state.
 */
function onOpenChange(next: boolean) {
	open.value = next
	if (next) {
		void probeRecents()
	} else {
		cancelSection()
	}
}

/** The field replaces nothing and appears below the actions, so it has to take
 *  focus itself — nothing else moves focus into a control that did not exist a
 *  tick ago. */
watch(creatingSection, (showing) => {
	if (!showing) return
	void nextTick(() => input.value?.focus())
})

function beginSection() {
	draft.value = ''
	sectionError.value = null
	creatingSection.value = true
}

function cancelSection() {
	creatingSection.value = false
	draft.value = ''
	sectionError.value = null
}

/**
 * Synchronous and local, because it can be: the sections of the active space are
 * already on screen. The store itself treats duplicate names as legal — ids are
 * identity there — so this is the one place the rule lives, and inline is where
 * the answer belongs when the user is still typing.
 */
function validate(name: string): string | null {
	if (name.length === 0) return 'A section needs a name.'
	if (sections.value.some((section) => section.name.toLowerCase() === name.toLowerCase())) {
		return 'This space already has a section with that name.'
	}
	return null
}

async function submitSection() {
	const name = draft.value.trim()
	const invalid = validate(name)
	if (invalid) {
		// The field stays open on a validation failure so the name can be corrected
		// rather than retyped.
		sectionError.value = invalid
		return
	}

	const result = await addSection(name)
	if (!result) {
		// A store-side refusal — the space became unavailable mid-edit, say. It also
		// reaches the panel's action-error band; repeating it here keeps it next to
		// the text it left in place.
		sectionError.value = 'That section could not be created.'
		return
	}

	cancelSection()
	open.value = false
}

function onSectionKeydown(event: KeyboardEvent) {
	// WebView2 reports keyCode 229 while an IME candidate is open, and accepting
	// one with Enter must not submit the name.
	if (event.isComposing || event.keyCode === 229) return

	if (event.key === 'Enter') {
		event.preventDefault()
		void submitSection()
	} else if (event.key === 'Escape') {
		// Consumed here rather than left to bubble: the field is a rung above the
		// menu, so the first Escape closes the field and the second closes the menu.
		event.preventDefault()
		event.stopPropagation()
		cancelSection()
	}
}

function onSectionInput(event: Event) {
	draft.value = (event.target as HTMLInputElement).value
	if (sectionError.value) sectionError.value = null
}
</script>

<template>
	<DropdownMenu :open="open" @update:open="onOpenChange">
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
			class="text-text-secondary w-72 max-h-(--reka-dropdown-menu-content-available-height) text-meta"
		>
			<DropdownMenuLabel class="text-text-disabled">Spaces</DropdownMenuLabel>

			<!-- Capped and scrolled internally, so a full recents list cannot outgrow
			     the fixed panel or make the body scroll. -->
			<div class="thin-scrollbar max-h-44 overflow-y-auto overflow-x-hidden">
				<div v-for="entry in recents" :key="entry.key" class="flex items-stretch gap-1">
					<DropdownMenuItem
						class="min-w-0 flex-1 items-start"
						:aria-current="entry.active ? 'true' : undefined"
						@select="openSpace(entry.path)"
					>
						<!-- Fixed-width slot, hidden rather than absent, so marking the active
						     row shifts no text. -->
						<span
							aria-hidden="true"
							class="bg-accent-ring mt-1.5 size-1.5 shrink-0 rounded-full"
							:class="entry.active ? 'opacity-100' : 'opacity-0'"
						/>
						<span class="min-w-0 flex-1">
							<span
								class="block truncate"
								:class="[
									entry.active ? 'text-accent-text font-semibold' : 'text-text-primary',
									entry.availability.state === 'unavailable' ? 'text-text-disabled' : '',
								]"
							>
								{{ entry.name }}
							</span>
							<span class="text-text-disabled block truncate text-meta">
								{{ entry.displayPath }}
							</span>
							<!-- The non-colour half of both cues: dimming alone would carry the
							     whole distinction, and colour alone would carry the active one. -->
							<span v-if="entry.active" class="sr-only">(active space)</span>
							<span
								v-if="entry.availability.state === 'pending'"
								class="text-text-disabled block text-meta"
							>
								Checking…
							</span>
							<span
								v-else-if="entry.availability.state !== 'available'"
								class="text-text-secondary block text-meta"
							>
								{{ entry.availability.message }}
							</span>
						</span>
					</DropdownMenuItem>

					<!-- Disabled rather than hidden on the active entry: removal would
					     otherwise have to invent a replacement active space. -->
					<DropdownMenuItem
						:disabled="entry.active"
						:aria-label="
							entry.active
								? `Switch to another space before removing ${entry.name}`
								: `Remove ${entry.name} from recents`
						"
						:title="entry.active ? 'Switch to another space first' : 'Remove from recents'"
						class="shrink-0 self-start"
						@select="removeRecent(entry.path)"
					>
						<IconLucideX class="size-3.5" aria-hidden="true" focusable="false" />
					</DropdownMenuItem>
				</div>
			</div>

			<DropdownMenuSeparator />

			<DropdownMenuItem class="min-h-6" @select="pickAndOpenSpace()">
				<IconLucideFolderOpen class="size-4" aria-hidden="true" focusable="false" />
				Open Space…
			</DropdownMenuItem>

			<DropdownMenuItem class="min-h-6" @select="createSpace()">
				<IconLucideFilePlus class="size-4" aria-hidden="true" focusable="false" />
				New Space…
			</DropdownMenuItem>

			<DropdownMenuSeparator />

			<!-- `@select.prevent` keeps the menu standing: the field it opens lives
			     inside it. -->
			<DropdownMenuItem v-if="!creatingSection" class="min-h-6" @select.prevent="beginSection">
				<IconLucidePlus class="size-4" aria-hidden="true" focusable="false" />
				New Section
			</DropdownMenuItem>

			<div v-else class="px-2 py-1.5">
				<label for="new-section-name" class="sr-only">New section name</label>
				<input
					id="new-section-name"
					ref="input"
					:value="draft"
					type="text"
					autocomplete="off"
					placeholder="Section name"
					:aria-invalid="sectionError ? 'true' : undefined"
					aria-describedby="new-section-error"
					class="border-separator bg-surface-hover text-text-primary placeholder:text-text-disabled outline-focus-ring h-7 w-full min-w-0 select-text rounded-md border px-1.5 text-body focus-visible:outline-2 focus-visible:-outline-offset-1"
					@input="onSectionInput"
					@keydown.stop="onSectionKeydown"
				/>
				<p
					v-if="sectionError"
					id="new-section-error"
					class="text-text-primary mt-1 text-meta"
					role="alert"
				>
					{{ sectionError }}
				</p>
				<div class="mt-1.5 flex justify-end gap-1">
					<button
						type="button"
						class="border-separator hover:bg-surface-hover outline-focus-ring rounded-md border px-2 py-0.5 text-meta transition-colors duration-fast focus-visible:outline-2 focus-visible:-outline-offset-1"
						@click="cancelSection"
					>
						Cancel
					</button>
					<button
						type="button"
						class="border-separator hover:bg-surface-hover outline-focus-ring rounded-md border px-2 py-0.5 text-meta transition-colors duration-fast focus-visible:outline-2 focus-visible:-outline-offset-1"
						@click="submitSection"
					>
						Create
					</button>
				</div>
			</div>
		</DropdownMenuContent>
	</DropdownMenu>
</template>
