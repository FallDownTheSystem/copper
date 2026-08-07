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
	<!-- The drag region is the header's own padding, never the field or the
	     button: a drag region swallows the pointer events of anything under it, and
	     every control here is something you click rather than grab.

	     **The padding is the grab handle, which is why it is generous.** Copper's
	     `c` mark used to be the dependable one — the field and the two buttons left
	     the header almost no bare area, so the region on the header itself was a
	     strip a few pixels wide. Removing the mark would have left nothing to aim
	     at, so the space it occupied went back into the frame instead: a full-width
	     band above and below the two rows, which is a wider target than a 32px
	     glyph in one corner and is there whichever row the pointer is nearest.

	     **The vertical padding grew and the horizontal padding did not.** `px-3` is
	     the list's own left edge — the note rows, the section headers and the
	     composer all start there — so widening it would push the search field out
	     of line with everything under it to buy a 2px strip nobody could aim at
	     anyway. -->
	<header
		data-tauri-drag-region
		class="border-separator flex min-h-12 flex-col gap-1.5 border-b px-3 py-3"
	>
		<!-- The search row keeps its own line, so the heading below it can be added
		     and removed without the field ever moving. -->
		<div class="flex items-center gap-2">
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
		     nothing the chip or the list controls beside it do can ever push the
		     search field sideways — which is what satisfies AC10 structurally rather
		     than by sizing the two to fit. The chip truncates (`min-w-0`) and the
		     controls hold their width (`shrink-0`), so a long section name loses
		     characters instead of pushing a control off the edge.

		     The controls are one right-aligned strip rather than two components each
		     finding their own way to the edge: they read as a group, and where the
		     boundary between them falls is not something the header should be able to
		     see. -->
		<div class="flex min-w-0 items-center gap-2">
			<ActiveSectionChip @closed="emit('switcherClosed', $event)" />
			<div class="ml-auto flex shrink-0 items-center gap-1">
				<DoneFilter />
				<SortControl />
			</div>
		</div>
	</header>
</template>
