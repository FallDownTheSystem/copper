<script setup lang="ts">
/**
 * A space with nothing in it yet: what the panel is for, where the next note
 * lands, and the presses that put one there.
 *
 * **This is the zero-data state, and it is the only one of the four that is
 * allowed to teach.** `SearchEmptyState` and `DoneEmptyState` answer a question
 * the user just asked — a query, a filter — so they say one sentence and offer
 * the way back. Nothing has been asked here. The panel is empty because the user
 * has never used it, which makes this the one moment where a list of shortcuts is
 * an answer rather than clutter, and the one moment it costs nothing: it is on
 * screen exactly until the first note exists and never again.
 *
 * **The headline keeps `PanelShell`'s wording rather than improving on it.** "No
 * notes yet." is already the register the other two empty states are written in,
 * and it is named by comments in `PanelStates` and `NoteList` and by an assertion
 * in `PanelShell.test.ts` that this state is *not* what a filtered-empty list
 * shows. Rewording it would make three files describe copy that no longer exists
 * and quietly turn that assertion into one that can never fail.
 *
 * **Nothing here is focusable, and that is deliberate.** The action this state
 * points at is the composer directly below it, which `PanelShell` already gives
 * focus to the moment this renders — a button in here would be a second thing to
 * reach for and would take the caret away from the field the user is being told
 * to type in.
 */

import { capLabel } from '@/lib/chordDisplay'
import { CHORDS } from '@/lib/chords'

const { activeSectionObject } = useSpace()
const { shortcuts } = useSettings()

/**
 * The section a capture or a composed note will land in. Never blank: with no
 * section at all the sentence still has to say something true, and "this space"
 * is the widest scope that remains correct.
 */
const destination = computed(() => activeSectionObject.value?.name ?? 'this space')

type Row = {
	/** What the row does, in the same words the rest of the panel uses for it. */
	action: string
	/** The key caps, when the action has a binding to show at all. */
	keys?: readonly string[]
	/** The modifier that has to be pressed twice, when the binding is a
	 *  double-tap rather than a chord. */
	doubleTap?: string | null
	/** Text typed into the composer rather than a press. Rendered as a code chip
	 *  and never as a cap — see the template. */
	typed?: string
	/** The menu an action with no binding lives in. Only `...` can appear here —
	 *  it is the panel's one menu — so the template pairs it with that trigger's
	 *  own icon. */
	menu?: string
}

/**
 * One binding split into caps, which is `ShortcutRecorder`'s own reading of the
 * string `settings.json` stores.
 *
 * Replicated here rather than imported because it lives inside that component
 * and this is the second reader; the two are eight lines of the same rule and
 * both are driven by the same stored value, so they cannot show different caps
 * for the same binding without the value itself having changed. Extracting it
 * would be the better answer the moment a third caller appears.
 *
 * A double-tap is stored as the same modifier twice separated by a space
 * (`"Shift Shift"`); everything else is a conventional `+`-joined chord.
 */
function caps(binding: string): Pick<Row, 'keys' | 'doubleTap'> {
	const taps = binding.split(' ')
	const doubleTap = taps.length === 2 && taps[0] === taps[1] ? taps[0]! : null
	// `capLabel` so a sided binding reads `Left Ctrl` here exactly as it does on
	// the recorder's own chips.
	const keys = (doubleTap ? [doubleTap] : binding.split('+')).map(capLabel)
	return { keys, doubleTap }
}

/**
 * The rows, in the order a first run needs them.
 *
 * The two global bindings lead because they are the only two that cannot be
 * discovered by using the panel: they fire from other applications, so nothing
 * on screen would ever reveal them. The four below them are in-panel and the
 * user is already looking at the surfaces that carry them, so they descend by how
 * hidden they are: the composer's own `Enter`, then a chord, then a directive
 * with no visible affordance at all, then a menu entry.
 *
 * **The two global rows are absent rather than empty until Rust answers.**
 * `App` pulls the shortcut state on mount, so this is a tick or two at startup —
 * but a row naming an action with nothing beside it reads as a shortcut the app
 * has forgotten, and a placeholder chord would be worse still: these are
 * rebindable, and printing the shipped default would be a lie for anyone who has
 * changed one.
 *
 * **`captureFallback` wins over `capture` when it is set.** It is not a
 * preference — it is Rust reporting that the low-level keyboard hook could not be
 * installed, so the double-tap is not live and a conventional chord is standing
 * in for it until the next restart. Teaching the stored binding in that state
 * would teach a press that does nothing at all.
 */
const rows = computed<Row[]>(() => {
	const state = shortcuts.value
	const global: Row[] = state
		? [
				{
					// "from any app" is the whole reason this row leads, and it is the
					// scope `SettingsView` gives the same binding ("Save whatever you have
					// selected, from any app."). Without it the row reads as something the
					// panel does to its own text.
					action: 'Capture selected text from any app',
					...caps(state.captureFallback ?? state.capture),
				},
				{ action: 'Show or hide Copper', ...caps(state.summon) },
			]
		: []

	return [
		...global,
		// `Enter` is written out rather than taken from `CHORDS.edit`, which also
		// displays "Enter" and is a different binding: that one opens the inline
		// editor on a focused card, and the composer's submit is handled in
		// `Composer` itself. Sharing the constant would claim a link between the two
		// that does not exist, and would silently mis-state this row if the note
		// list's Enter were ever rebound.
		{ action: 'Add a note', keys: ['Enter'] },
		// This one is genuinely the chord table's, so it is read from there: a
		// palette that moved to another binding must not leave a card behind
		// teaching the old one.
		{ action: 'Open the command palette', keys: [CHORDS.commandPalette.display] },
		// A real directive, not a hint at one: `submit_entry` classifies a leading
		// `#` in Rust and makes or activates that section, which is the same call
		// `SectionSwitcher`'s create row makes. It earns a row because it is the one
		// route with no affordance anywhere on screen — the two menu paths at least
		// announce themselves once `...` is open — and because the composer it is
		// typed into is the field this card already has focus in.
		{ action: 'New section', typed: '# Section name' },
		// No binding and no directive, so the row names where it lives. "More
		// actions" is the trigger's own accessible name rather than a description of
		// it, so the words here and the words a screen reader announces at the
		// button are the same words; the icon beside them is that button's icon, for
		// the eye doing the same search.
		{ action: 'New space', menu: 'More actions' },
	]
})
</script>

<template>
	<!-- **`px-4` puts this text on the marks column, which is the edge the list is
	     read down.** Inside `PanelShell`'s `px-1` it resolves to 20px, and 20px is
	     where the note row's completion box, the section heading's marker dot and
	     the search icon all sit. Not the note *body*, which is a further 24px in at
	     44px: what lines up down the region is the leading mark, and this card is
	     standing in for the rows, not for their text.

	     It is the row gridcell's value, so it moves when that moves — the two are
	     only ever right together, and `SectionHeader` carries the same note.

	     **This is not a hypothetical alignment.** `PanelShell` renders this card
	     *additively*, so a zero-note space still shows its section headings above it
	     — their marker dots are the 20px column, on screen at the same time as this
	     sentence. The bands outside the region (`PanelHeader`, `Composer`) are still
	     at 12px and this card no longer matches them, which is a divergence the whole
	     scroll region now has rather than this card's to fix alone: the two narrowing
	     empty states and the section's own empty line are at 12px too, and moving one
	     of the four would only trade which pair disagrees.

	     **In flow, not centred, and the composer is the reason.** Centring this in
	     the free space would mean measuring against the scroll region's height, and
	     that height changes every time the composer grows a line: the card would
	     drift upward as the user types the very note that is about to dismiss it.
	     `pt-6` instead — deeper than the 16px the two narrowing states use, because
	     this one carries a list rather than a sentence and wants the air, and fixed
	     so nothing below it can move it. -->
	<div class="px-4 pt-6 pb-2">
		<!-- `p` rather than a heading, matching `SearchEmptyState` and
		     `DoneEmptyState` exactly. An `h2` here would read as a peer of the
		     section headings in the list above it, which are the outline's real
		     structure; three empty states with three different heading treatments
		     would be the defect, not the fix. -->
		<p class="text-text-primary text-body font-semibold">No notes yet.</p>
		<p class="text-text-secondary mt-1 text-meta text-pretty">
			New notes land in {{ destination }}.
		</p>

		<!-- A description list because that is what these rows are: the action is
		     the term and the press is its description. The `div` wrapper around each
		     pair is valid inside `dl` and is what lets a row be a flex line without
		     the `dt` and `dd` having to be siblings in the layout.

		     **`min-h-7` is the rhythm, and no `space-y-*` beside it.** A row holding
		     caps is 24px tall and a row of plain text is 17px, so an even gap between
		     them produced uneven-looking spacing: what the eye measures is the
		     distance between the words, not between the boxes. A shared floor taller
		     than the tallest content makes every row the same band, and then the gaps
		     are equal because they are the same gap. An explicit gap on top of it
		     would only reintroduce a second, smaller unevenness whenever a row wraps.

		     16px above the list against ~11px inside it — the group break stays wider
		     than the row rhythm, so the sentence above does not read as the first
		     row. -->
		<dl class="mt-4">
			<div v-for="row in rows" :key="row.action" class="flex min-h-7 min-w-0 items-center gap-3">
				<dt class="text-text-primary min-w-0 flex-1 text-meta">{{ row.action }}</dt>
				<!-- `flex-wrap` and no truncation: a rebound summon chord can be four
				     caps wide, and a row that grows a line is a row that still reads.
				     `text-meta` sits here rather than on each child so the size is set
				     once on the block that owns them. -->
				<dd
					class="text-text-secondary flex shrink-0 flex-wrap items-center justify-end gap-1 text-meta"
				>
					<KbdChord v-if="row.keys" :keys="row.keys" />
					<!-- Plain text, never a cap, for the same reason `ShortcutRecorder`
					     refuses to chip it: a cap says "press this key", and neither
					     "double-tap" nor the name of a menu is a key. -->
					<span v-if="row.doubleTap">double-tap</span>
					<!-- A `code` chip rather than a cap, because this is a string the user
					     types and not a key they press — the one distinction the cap
					     treatment must keep. It borrows the cap's family (mono, filled,
					     primary text, so a literal to be typed exactly is legible) and
					     drops what makes a cap a key: no ring, no inset highlight, no
					     `h-6` box. The 6px corner is `.note-prose code`'s, which is the
					     app's existing appearance for typed text; the cap's own 8px is
					     pinned to its 24px height and means nothing on a chip that is only
					     as tall as its line box.

					     On one line because `condense` turns the newlines around an
					     interpolation into a leading and trailing space, and inside a
					     filled chip that space is padding the chip did not ask for. -->
					<code v-else-if="row.typed" class="bg-surface-hover text-text-primary rounded-[6px] px-1.5 py-0.5 font-mono">{{ row.typed }}</code>
					<template v-else-if="row.menu">
						<span>{{ row.menu }}</span>
						<!-- The trigger's own glyph at the trigger's own size, so the search
						     it starts ends at the right button. Hidden from the
						     accessibility tree because the name beside it is what a screen
						     reader announces at that button too. -->
						<IconLucideEllipsis class="size-4 shrink-0" aria-hidden="true" focusable="false" />
					</template>
				</dd>
			</div>
		</dl>
	</div>
</template>
