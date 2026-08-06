<script setup lang="ts">
import { isComposing } from '@/lib/chords'
import { normaliseSectionName } from '@/lib/sectionName'

/**
 * The `...` menu: the spaces switcher, the two space actions, and the inline
 * section-creation field.
 *
 * The switcher is a section of this menu rather than a third view. The design
 * puts space switching behind `...` and the recents list is short. Settings is
 * the one entry here that leaves the list entirely, which is why it sits alone
 * at the bottom rather than beside the space actions.
 *
 * **Switching is always an explicit user choice.** Nothing here infers or
 * auto-switches — including at startup, where the store may already have
 * re-pointed to a loadable entry and this renders whatever is actually active.
 */

// Read from the composable rather than taken as props, exactly as the two
// context menus do. `PanelShell` publishes the panel root and the in-clip portal
// host there once, so a menu does not have to be drilled two components' worth of
// props to reach them.
const { boundary, portalTo } = useOverlayHost()
const { recents, probeRecents, openSpace, pickAndOpenSpace, createSpace, removeRecent } =
	useSpaces()
const { sections, addSection, errorFor } = useSpace()
const { isSwitcherOpenIn, setSwitcherOpen, closeSwitcher } = useSections()
const { showSettings } = useView()
/** The band's message for this surface's failures. Read back after a refused
 *  create so the inline field can repeat the store's own cause. */
const listError = errorFor('list')

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
		// The submenu goes down with its parent, and its lifecycle has to run —
		// otherwise the filter it holds survives into the next opening.
		closeSwitcher('menu')
	}
}

/** The switcher chose a section, so the whole menu has done its job. */
function closeMenu() {
	closeSwitcher('menu')
	open.value = false
}

function onSwitcherOpenChange(next: boolean) {
	setSwitcherOpen('menu', next)
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
	// The store's own notion of "the same name", not a second one: it collapses
	// whitespace before matching, so `Deep  Research` and `Deep Research` are one
	// section there and have to be one here. The switcher's create row goes
	// through the same rule.
	const wanted = normaliseSectionName(name).toLowerCase()
	const clash = sections.value.some(
		(section) => normaliseSectionName(section.name).toLowerCase() === wanted,
	)
	if (clash) return 'This space already has a section with that name.'
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
		// A store-side refusal — the space became unavailable mid-edit, say. The
		// cause reaches the panel's action-error band; it is repeated here verbatim,
		// next to the text it left in place. Repeated rather than replaced by a
		// sentence of our own: "could not be created" says nothing the user can act
		// on, and the store's message is the only thing that names what went wrong.
		// The generic sentence is the fallback for the case that leaves no cause
		// behind at all.
		sectionError.value = listError.value ?? 'That section could not be created.'
		return
	}

	cancelSection()
	open.value = false
}

function onSectionKeydown(event: KeyboardEvent) {
	if (isComposing(event)) return

	if (event.key === 'Enter') {
		event.preventDefault()
		void submitSection()
	} else if (event.key === 'Escape') {
		// `@keydown.stop` on the field is what keeps this from reaching the menu,
		// so the first Escape closes the field and the second — with the field
		// unmounted — closes the menu.
		event.preventDefault()
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
			class="squircle text-text-secondary hover:bg-surface-hover active:bg-surface-active outline-focus-ring grid size-8 shrink-0 place-items-center rounded-md transition-colors duration-fast focus-visible:outline-2 focus-visible:-outline-offset-1"
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
				<!-- Keyed by path, not by comparison key: the key is many-to-one over
				     stored paths by design — a hand-edited `%APPDATA%` entry and the
				     same file opened through the picker share one — and a duplicate
				     `:key` makes Vue patch the wrong row. The store dedupes `recents`
				     by resolved path, so the path is the unique one. -->
				<div v-for="entry in recents" :key="entry.path" class="flex items-stretch gap-1">
					<DropdownMenuItem
						class="min-w-0 flex-1 items-start"
						:aria-current="entry.active ? 'true' : undefined"
						@select="openSpace(entry.path)"
					>
						<!-- `mt-1.5` on the dot: this row is two lines tall, so a dot centred on
						     the whole row would sit beside the path rather than the name. -->
						<ActiveMarker :active="entry.active" label="active space" class="mt-1.5">
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
								<!-- The non-colour half of the availability cue; the active one's twin
								     is the marker's own. Dimming alone would carry the distinction. -->
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
						</ActiveMarker>
					</DropdownMenuItem>

					<!-- Disabled rather than hidden on the active entry: removal would
					     otherwise have to invent a replacement active space.

					     The hint lives on this wrapper rather than on the item itself.
					     A disabled menu item carries `pointer-events: none`, so a
					     `title` on it never surfaces — the browser needs a hover to show
					     a tooltip and the element cannot receive one. The wrapper stays
					     interactive, so the explanation is actually reachable by the
					     people who need it. -->
					<div
						:title="entry.active ? 'Switch to another space first' : 'Remove from recents'"
						class="flex shrink-0 items-start"
					>
						<DropdownMenuItem
							:disabled="entry.active"
							:aria-label="
								entry.active
									? `Switch to another space before removing ${entry.name}`
									: `Remove ${entry.name} from recents`
							"
							@select="removeRecent(entry.path)"
						>
							<IconLucideX class="size-3.5" aria-hidden="true" focusable="false" />
						</DropdownMenuItem>
					</div>
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

			<!-- The section group. `Switch section` leads it because picking an
			     existing destination is the common case and creating one is the
			     exception — and both are the same list, since the submenu renders the
			     component the composer's chip does rather than a second copy of it. -->
			<!-- Controlled, not left to reka. The switcher's filter is module state
			     shared with the chip's host, so an uncontrolled submenu never ran the
			     open/close lifecycle: its query survived every dismissal, a reopened
			     submenu came up pre-filtered, and a stale no-match query showed only
			     `Create section "<old query>"` — with Enter creating it. Routing both
			     hosts through the same two functions is also what lets an epoch change
			     close this one rather than silently re-pointing it at a different
			     space's sections.

			     The boundary and padding match the chip's host: reka renders
			     sub-content inside the parent's portal, so it inherits the in-panel
			     host, but not the collision box that keeps it inside the rounded
			     clip. -->
			<DropdownMenuSub :open="isSwitcherOpenIn('menu')" @update:open="onSwitcherOpenChange">
				<DropdownMenuSubTrigger class="min-h-6">
					<IconLucideListTree class="size-4" aria-hidden="true" focusable="false" />
					Switch section
				</DropdownMenuSubTrigger>
				<DropdownMenuSubContent
					:collision-boundary="boundary ?? undefined"
					:collision-padding="8"
					class="w-64 max-h-(--reka-dropdown-menu-content-available-height) text-meta"
				>
					<SectionSwitcher @close="closeMenu" />
				</DropdownMenuSubContent>
			</DropdownMenuSub>

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
					class="squircle border-separator bg-surface-hover text-text-primary placeholder:text-text-disabled outline-focus-ring h-7 w-full min-w-0 select-text rounded-md border px-1.5 text-body focus-visible:outline-2 focus-visible:-outline-offset-1"
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
					<!-- `panel-button` is the project's one secondary-control appearance.
					     Only the tighter padding and the focus ring are local, because
					     the utility carries neither — and `py-0.5` is emitted after it,
					     so at equal specificity it wins. -->
					<button
						type="button"
						class="panel-button outline-focus-ring py-0.5 focus-visible:outline-2 focus-visible:-outline-offset-1"
						@click="cancelSection"
					>
						Cancel
					</button>
					<button
						type="button"
						class="panel-button outline-focus-ring py-0.5 focus-visible:outline-2 focus-visible:-outline-offset-1"
						@click="submitSection"
					>
						Create
					</button>
				</div>
			</div>

			<DropdownMenuSeparator />

			<!-- Its own group at the bottom, below the space and section actions:
			     everything above operates on the open document, and this leaves it. -->
			<DropdownMenuItem class="min-h-6" @select="showSettings()">
				<IconLucideSettings class="size-4" aria-hidden="true" focusable="false" />
				Settings
			</DropdownMenuItem>
		</DropdownMenuContent>
	</DropdownMenu>
</template>
