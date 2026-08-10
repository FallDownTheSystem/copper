<script setup lang="ts">
/**
 * The done filter and, once the done view is up, the purge that goes with it.
 *
 * **It sits in the chip's row rather than the search row**, in the strip of list
 * controls at the opposite end from `ActiveSectionChip`. That row exists precisely
 * so the heading area can gain and lose controls without the search field ever
 * moving (`PanelHeader`), which is how AC10 is satisfied structurally rather than
 * by careful sizing: the chip keeps its `min-w-0` and truncates, the strip keeps
 * `shrink-0`, and neither can push the other out of the header.
 *
 * The delete button appears only in the done view, which is AC5, and only when
 * there is something to delete — a button that explains it has nothing to do is
 * worse than one that is not there.
 */
const { doneFilter, doneOnly, doneTotal, todoTotal, allTotal, nextDoneFilter, cycleDoneFilter } =
	useNoteList()
const { doneCount, doneTargets, deleteDoneInActiveSection } = useNoteActions()
const { activeSectionObject } = useSpace()
// The same in-clip portal host and collision boundary every menu here uses —
// see `useOverlayHost` for why reka's document.body default is wrong for this
// panel.
const { boundary, portalTo } = useOverlayHost()

/** Never blank, so the label always names a scope. */
const sectionName = computed(() => activeSectionObject.value?.name ?? 'this section')

/**
 * The confirmation, as a popover rather than as a state of the button.
 *
 * It was an inline label first — the armed button grew `DELETE 2 DONE?` into
 * the strip it shares with the section chip — and live use found the flaw: the
 * armed width comes out of the chip beside it, and in the done view it pushed
 * the chip's chevron out of the header. A label that must name a count of any
 * size cannot live in a row that promises its width to a neighbour, so the
 * question moved into an overlay with room to name the whole scope — count,
 * noun and section — while the button itself stays icon-wide in every state.
 *
 * The popover also puts the confirming press on a *separate control*, which is
 * what retires the two guards the one-button form carried: a held Enter now
 * toggles the popover instead of confirming it — reka autofocuses the content,
 * where Cancel is the first tabbable, so even the browser's synthesised repeat
 * clicks land somewhere safe — and the second click of a double-click merely
 * closes what the first opened. Escape and a click elsewhere dismiss it through
 * reka's own layer, which `inOverlay` keeps off the shell's Escape ladder.
 *
 * Still not a modal dialog, deliberately: deleting a section ships with no
 * confirmation at all because the operation is one undo (`SectionContextMenu`),
 * and this prompt exists for the count-vs-view discrepancy, not because the
 * action is unrecoverable. The undo message stays the real safety net.
 */
const confirming = ref(false)

/**
 * The armed offer's identity: *which* notes it would delete, not how many.
 *
 * Sorted before joining, so this is genuinely the set rather than the set in a
 * particular order — a document change that only reorders the same notes is not a
 * different offer and must not throw the confirmation away.
 *
 * A count alone is not an identity, and the gap is reachable: marking one note
 * done and unmarking another leaves the total unchanged over a different set, and
 * a confirmation armed before that lands would delete notes it never offered.
 *
 * The separator is NUL because a note id cannot contain one, which makes the join
 * collision-free.
 *
 * **Spelled `\u0000`, and never as a literal NUL byte.** A raw control byte in the
 * source makes git classify the whole file as binary — no diffs, no reviewable
 * history — and neither the formatter nor the typechecker notices, so nothing in
 * the gates would catch it coming back. The long form rather than `\0` because
 * `\0` is only safe while nothing follows it: append a digit and it silently
 * becomes a legacy octal escape, which is a syntax error in a module.
 */
const armedTargets = computed(() => [...doneTargets.value].sort().join('\u0000'))

/** Withdrawn whenever the offer stops being the one the user is looking at: a
 *  different set of targets under them, or the view they opened it from going
 *  away. Keyed on the whole filter rather than on `doneOnly`, so leaving the done
 *  view closes the popover whichever of the other two states the press lands in. */
watch([armedTargets, doneFilter], () => {
	confirming.value = false
})

/** The confirming press, on its own control inside the popover. */
function confirm() {
	confirming.value = false
	void deleteDoneInActiveSection()
}

/**
 * The question the popover asks. It names the count, the noun *and* the section
 * — the overlay has the width the strip never did, and the section is the half
 * a reader cannot infer: the view behind the button is document-wide while the
 * delete takes the active section's notes alone (AC9), so the two counts can
 * legitimately disagree.
 */
const confirmQuestion = computed(() =>
	countMessage(doneCount.value, {
		one: `Delete 1 done note in ${sectionName.value}?`,
		many: (count) => `Delete ${count} done notes in ${sectionName.value}?`,
	}),
)

/**
 * The accessible name, which is the *only* name this button has: it is an icon
 * and nothing else. It names the section as well as the count — the view is
 * document-wide and this is not, so a screen reader landing on "Delete done
 * notes" would be told a scope the button does not have.
 */
const label = computed(() =>
	countMessage(doneCount.value, {
		one: `Delete 1 done note in ${sectionName.value}`,
		many: (count) => `Delete ${count} done notes in ${sectionName.value}`,
	}),
)

/**
 * **The visible label names the state the next press produces, not the one in
 * effect** — which is the opposite of `SortControl` beside it, deliberately.
 *
 * The sort has to report itself because it is otherwise invisible: Manual is the
 * order most lists are already in, so nothing on screen distinguishes it from a
 * sorted one. The done filter's state is the list itself — a view with no
 * finished notes in it is what "hiding done" looks like — so the button's width
 * is better spent on where the press goes, which is the one thing the list
 * cannot show.
 *
 * **All three offers carry their own total**, because the number is what makes a
 * destination worth choosing: "Todo 2" and "Todo 41" are different offers behind
 * the same word, and the word alone leaves the reader to press and find out. A
 * zero is the sharpest case of it — `Done 0` warns that the press leads somewhere
 * empty, which is the one thing the current list cannot show about a view it is
 * not displaying.
 *
 * All three are document-wide, matching the view the press produces. The delete
 * button beside this one counts the active section instead, and they can
 * legitimately disagree; that one names its scope in its own accessible name.
 */
const cycleLabel = computed(
	() =>
		({
			all: `All ${allTotal.value}`,
			todo: `Todo ${todoTotal.value}`,
			done: `Done ${doneTotal.value}`,
		})[nextDoneFilter.value],
)

/** The state, then what the press does with it — `SortControl`'s sentence, and
 *  it has to end with the visible label so the accessible name contains it. A
 *  voice-input user says the words on the button, and a count is one of them. */
const CYCLE_STATES = {
	todo: 'Unfinished notes only',
	done: 'Done notes only',
	all: 'All notes',
} as const satisfies Record<DoneFilter, string>

const cycleTitle = computed(
	() => `${CYCLE_STATES[doneFilter.value]} · press for ${cycleLabel.value}`,
)
</script>

<template>
	<!-- `gap-3` for the same reason the header cluster wearing these two uses it:
	     both children carry their own border, and two bordered controls a pixel
	     apart read as one control with a seam. -->
	<div class="flex shrink-0 items-center gap-3">
		<!-- **Icon-only, in every state.** The resting button is one of three
		     controls in a strip a panel wide, and "Delete done" beside a trash icon
		     told the pointer nothing the icon had not; the accessible name carries
		     it for everyone the icon does not reach. The confirmation lives in the
		     popover rather than on the button, so no state of this control can
		     borrow width from the chip beside it — which is the overflow the armed
		     inline label caused.

		     It leads the strip rather than following the filter it belongs to: the
		     destructive control is the one that must not sit where a mis-aimed press
		     can find it, and the filter is what the pointer arrives for. -->
		<Popover v-if="doneOnly && doneCount > 0" :open="confirming" @update:open="confirming = $event">
			<PopoverTrigger
				data-delete-done
				class="panel-button text-text-secondary inline-flex min-h-6 shrink-0 items-center px-1.5"
				:title="label"
				:aria-label="label"
			>
				<IconLucideTrash2 class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
			</PopoverTrigger>

			<!-- `align="end"` because the trigger sits near the panel's right edge;
			     the collision boundary slides it back inside the rounded clip either
			     way, exactly as it does for the menus. -->
			<PopoverContent
				v-if="portalTo"
				:to="portalTo"
				align="end"
				:collision-boundary="boundary ?? undefined"
				:collision-padding="8"
				class="w-64 text-meta"
			>
				<p class="text-text-primary">{{ confirmQuestion }}</p>
				<!-- Cancel first, and the order is load-bearing rather than layout: reka
				     autofocuses the first tabbable on open, so the clicks a held Enter
				     synthesises land on the dismissal, never on the delete. -->
				<div class="mt-2 flex justify-end gap-1">
					<button type="button" class="panel-button py-0.5" @click="confirming = false">
						Cancel
					</button>
					<button
						type="button"
						data-confirm-delete-done
						class="panel-button text-destructive-text py-0.5"
						@click="confirm"
					>
						Delete
					</button>
				</div>
			</PopoverContent>
		</Popover>

		<!-- **A three-state cycle, not a toggle**, and `aria-pressed` went with the
		     second state: a control with three positions is not pressed or unpressed,
		     and a screen reader announcing "not pressed" for the done-only view would
		     be stating something false. The accessible name carries all of it instead
		     — the view in effect, then the one a press produces.

		     The accent marks *any* departure from the resting view rather than
		     "something is hidden". The panel rests on `all`, and both of the other two
		     hide half the document in opposite directions, so a colour that meant
		     "done notes are hidden" would leave the review view unmarked while it is
		     hiding just as much. What is worth a colour is that the list on screen is
		     not the whole document and a press leads back. -->
		<button
			type="button"
			data-done-filter
			class="panel-button inline-flex min-h-6 shrink-0 items-center gap-1 px-1.5"
			:class="doneFilter === 'all' ? 'text-text-secondary' : 'text-accent-text'"
			:title="cycleTitle"
			:aria-label="cycleTitle"
			@click="cycleDoneFilter"
		>
			<IconLucideCircleCheck class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
			<!-- `tabular-nums` because every one of the three labels now ends in a
			     number, and marking a note done moves two of those counts at once. The
			     button sits in a row it shares with the chip, so proportional digits
			     would let a tick nudge the strip sideways. -->
			<span class="text-label tabular-nums uppercase">{{ cycleLabel }}</span>
		</button>
	</div>
</template>
