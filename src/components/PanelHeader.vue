<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'

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

/** Focus stays in the field rather than moving to the list, for the reason the
 *  empty state's own Clear search gives: what follows a cleared query is almost
 *  always another one. Also the only way the button's own focus survives it
 *  unmounting itself. */
function clearAndFocus() {
	clearQuery()
	void nextTick(() => input.value?.focus())
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
	     the field boxes' left edge — the composer's box starts there too — so
	     widening it would push the search field out of line with the one under it
	     to buy a 2px strip nobody could aim at anyway. The list under it is on a
	     different edge and always was: a card's own box starts at 4px, and what
	     lines up with the header is neither of those boxes but the *leading marks*
	     — the search icon at 20px, and the completion box and section dot brought
	     onto it. The chip pays for its share in its own padding rather than here,
	     because moving this number would move the field with it.

	     **`pb-2` against `pt-3`, so the bottom strip is 8px and not 12.** The gap
	     above the chip is the row `gap-1.5` plus the pixel or so the taller
	     controls beside it centre it by — about 7px — and 12 below it read as the
	     chip sitting high in its own row. Eight brings the two within 2px, which is
	     as close as they should come: below the chip is a hard rule rather than
	     another control, and an object set the same distance off a line as off its
	     neighbour reads tighter than it measures. The strip stays a full-width 8px
	     of drag region, and the band above and the two side strips are untouched at
	     12 — what was given up is the least aimed-at edge of the four. -->
	<header
		data-tauri-drag-region
		class="border-separator flex min-h-12 flex-col gap-1.5 border-b px-3 pt-3 pb-2"
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
					class="panel-field h-8 w-full min-w-0 pr-8 pl-8"
					@keydown="onKeydown"
				/>

				<!-- Chromium renders no clear affordance of its own for `type="search"`,
				     and Escape is a rung of a ladder nobody is told about — so without
				     this the only way out of a query is selecting it and deleting it.

				     Not the header's `icon-button`, which is a fixed 32px and would fill
				     the 32px field edge to edge. 24px inset by 4px reads as part of the
				     field and still clears WCAG 2.5.8's 24px floor; nothing is close
				     enough to overlap it, and the field itself is not a hit target this
				     can steal from. -->
				<button
					v-if="hasQuery"
					type="button"
					aria-label="Clear search"
					title="Clear search"
					class="squircle text-text-secondary hover:bg-surface-hover active:bg-surface-active focus-ring absolute top-1/2 right-1 grid size-6 -translate-y-1/2 place-items-center rounded-md transition-colors duration-fast"
					@click="clearAndFocus"
				>
					<IconLucideX class="size-3.5" aria-hidden="true" focusable="false" />
				</button>
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

			<!-- Rightmost, where a caption control sits on Windows — the panel has no
			     native title bar, so this is its minimize box, and the `...` menu to
			     its left mirrors where a browser puts its own. Through
			     `minimize_panel` rather than `getCurrentWindow().minimize()`, for the
			     reason the menu's Hide to tray gives: minimizing also ends an open
			     recording session, and the window operations live in Rust so a second
			     path cannot end up doing half of one. -->
			<button
				type="button"
				class="icon-button shrink-0"
				aria-label="Minimize"
				title="Minimize"
				@click="invoke('minimize_panel')"
			>
				<IconLucideMinus class="size-4" aria-hidden="true" focusable="false" />
			</button>
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
			<!-- `gap-3`, not `gap-1`: both of these carry their own border, and two
			     bordered controls a pixel apart read as one segmented control with a
			     seam. Twelve pixels is what separates them into two. The chip beside
			     them truncates, so the width comes out of a long section name rather
			     than off the edge of the panel. -->
			<div class="ml-auto flex shrink-0 items-center gap-3">
				<DoneFilter />
				<SortControl />
			</div>
		</div>
	</header>
</template>
