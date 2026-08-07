<script setup lang="ts">
/**
 * The query lives in `useNoteSearch` at module scope, not here. A ref held
 * inside this component cannot be read by `NoteList` — the same private-copy
 * trap task-004 warns about, one level up.
 */
const { query, hasQuery, clearQuery } = useNoteSearch()

/**
 * The pin, mirroring the settings row rather than owning a second copy of the
 * state: both write through the same setter, so a click here and a switch there
 * cannot disagree.
 *
 * It lives in the header because "put this behind the window I am reading" is
 * something you want while the panel is in the way, and having to open Settings
 * to do it means covering the thing you were trying to see.
 */
const { alwaysOnTop, setAlwaysOnTop, errorFor } = useSettings()
const pinError = errorFor('alwaysOnTop')
const { reportActionError, clearActionError } = useSpace()

/**
 * A refused pin has to say so *here*, because here is the only surface it has.
 *
 * The settings row renders its own failure inline, next to the control that
 * produced it. This control has no such slot — it is a 32-pixel button in a
 * header — so a rejected write would flip nothing and explain nothing, and the
 * user would be left pressing a pin that does not stick. It borrows the panel's
 * one error band, the same `list` scope the space actions in the `...` menu
 * report through, since both are "something you asked the shell to do did not
 * happen".
 *
 * The message is Rust's own, read back off the row rather than written again
 * here: it names whether the window state or the file was the part that failed,
 * and a sentence of our own could only be vaguer.
 */
async function togglePin() {
	clearActionError('list')
	if (await setAlwaysOnTop(!alwaysOnTop.value)) return
	reportActionError('list', pinError.value ?? 'Copper could not change the always-on-top setting.')
}

/** Forwarded from the section heading to the composer, which is the only place
 *  that knows whether the switcher was opened from a half-typed line. */
const emit = defineEmits<{ switcherClosed: [event: Event] }>()

const input = useTemplateRef<HTMLInputElement>('input')

function focusSearch() {
	input.value?.focus()
	input.value?.select()
}

/** One rung of the Escape ladder, handled where the focus is. The press is
 *  consumed only when there is a query to clear, so Escape in an empty field
 *  still falls through to the levels below it. */
function onKeydown(event: KeyboardEvent) {
	if (event.key !== 'Escape' || !hasQuery.value) return
	event.preventDefault()
	event.stopPropagation()
	clearQuery()
}

defineExpose({ focusSearch, query })
</script>

<template>
	<!-- The drag region is the header's empty area and the mark, never the field
	     or the button: a drag region swallows the pointer events of anything under
	     it. -->
	<header
		data-tauri-drag-region
		class="border-separator flex min-h-12 flex-col gap-1.5 border-b px-3 py-2"
	>
		<!-- The search row keeps its own line, so the heading below it can be added
		     and removed without the field ever moving. -->
		<div class="flex items-center gap-2">
			<!-- Copper's mark, and the header's dependable grab handle: the field and
			     the menu button leave the header almost no bare area, so the drag
			     region on the header itself is a strip a few pixels wide in practice.

			     The glyph is this element's own text rather than a child span, because
			     a child element receives the mousedown and `data-tauri-drag-region` is
			     read off the element that does. Branding rather than a control — no
			     hover state, no tab stop, nothing to activate — so it is a `div` and
			     `aria-hidden`: a lone decorative `c` announced to a screen reader is
			     noise. -->
			<div
				data-tauri-drag-region
				aria-hidden="true"
				class="text-accent-text grid size-8 shrink-0 cursor-grab place-items-center text-body font-semibold select-none active:cursor-grabbing"
			>
				c
			</div>

			<label for="panel-search" class="sr-only">Search notes</label>
			<div class="relative min-w-0 flex-1">
				<IconLucideSearch
					class="text-text-disabled pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2"
					aria-hidden="true"
					focusable="false"
				/>
				<input
					id="panel-search"
					ref="input"
					v-model="query"
					data-search
					type="search"
					name="search"
					autocomplete="off"
					placeholder="Search notes…"
					class="panel-field h-8 w-full min-w-0 pr-2 pl-8"
					@keydown="onKeydown"
				/>
			</div>

			<!-- Its own control rather than a menu entry: a menu you have to open to
			     read the state is a state you cannot see. `aria-pressed` carries the
			     toggle to a screen reader, and the glyph carries it to everyone else —
			     the slashed pin is the non-colour half of the same cue. Outside any
			     drag region, like the menu trigger beside it. -->
			<button
				type="button"
				class="icon-button shrink-0"
				:aria-pressed="alwaysOnTop"
				:aria-label="alwaysOnTop ? 'Keep on top: on' : 'Keep on top: off'"
				:title="alwaysOnTop ? 'Stop keeping on top' : 'Keep on top'"
				@click="togglePin"
			>
				<IconLucidePin
					v-if="alwaysOnTop"
					class="text-accent-text size-4"
					aria-hidden="true"
					focusable="false"
				/>
				<IconLucidePinOff v-else class="size-4" aria-hidden="true" focusable="false" />
			</button>

			<PanelMenu />
		</div>

		<!-- Directly under the field, labelling the list below it. Its own row, so
		     it can never push the search field sideways. -->
		<div class="flex min-w-0">
			<ActiveSectionChip @closed="emit('switcherClosed', $event)" />
		</div>
	</header>
</template>
