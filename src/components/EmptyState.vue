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
	/** Where the action lives, for the row that has no binding. Rendered as plain
	 *  text and never as a cap — see the template. */
	where?: string
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
 * on screen would ever reveal them. The three below them are in-panel and the
 * user is already looking at the surfaces that carry them.
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
					action: 'Capture selected text',
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
		// No binding exists for either, so the row says where they live instead of
		// inventing one. "More actions" is the menu button's own accessible name
		// rather than a description of it, so the words here and the words a screen
		// reader announces at the trigger are the same words.
		{ action: 'New space or section', where: 'More actions menu' },
	]
})
</script>

<template>
	<!-- `px-3` is the list's own left edge — note rows, section headings and the
	     composer all start there — so the card lines up with everything it sits
	     among rather than floating in the middle of the column.

	     **In flow, not centred, and the composer is the reason.** Centring this in
	     the free space would mean measuring against the scroll region's height, and
	     that height changes every time the composer grows a line: the card would
	     drift upward as the user types the very note that is about to dismiss it.
	     `pt-6` instead — deeper than the 16px the two narrowing states use, because
	     this one carries a list rather than a sentence and wants the air, and fixed
	     so nothing below it can move it. -->
	<div class="px-3 pt-6 pb-2">
		<!-- `p` rather than a heading, matching `SearchEmptyState` and
		     `DoneEmptyState` exactly. An `h2` here would read as a peer of the
		     section headings in the list above it, which are the outline's real
		     structure; three empty states with three different heading treatments
		     would be the defect, not the fix. -->
		<p class="text-text-primary text-body font-semibold">No notes yet.</p>
		<p class="text-text-secondary mt-1 text-meta text-pretty">
			Whatever you add lands in {{ destination }}.
		</p>

		<!-- A description list because that is what these rows are: the action is
		     the term and the press is its description. The `div` wrapper around each
		     pair is valid inside `dl` and is what lets a row be a flex line without
		     the `dt` and `dd` having to be siblings in the layout.

		     16px above it against 4px between the rows — the group break has to be
		     several times the gap inside the group or the sentence above reads as the
		     first row of the list. -->
		<dl class="mt-4 space-y-1">
			<div v-for="row in rows" :key="row.action" class="flex min-w-0 items-center gap-3">
				<dt class="text-text-primary min-w-0 flex-1 text-meta">{{ row.action }}</dt>
				<!-- `flex-wrap` and no truncation: a rebound summon chord can be four
				     caps wide, and a row that grows a line is a row that still reads. -->
				<dd class="flex shrink-0 flex-wrap items-center justify-end gap-1">
					<KbdChord v-if="row.keys" :keys="row.keys" />
					<!-- Plain text, never a cap, for the same reason `ShortcutRecorder`
					     refuses to chip it: a cap says "press this key", and neither
					     "double-tap" nor the name of a menu is a key. -->
					<span v-if="row.doubleTap" class="text-text-secondary text-meta">double-tap</span>
					<span v-else-if="row.where" class="text-text-secondary text-meta">{{ row.where }}</span>
				</dd>
			</div>
		</dl>
	</div>
</template>
