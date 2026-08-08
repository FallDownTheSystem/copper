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

/** Never blank, so the label always names a scope. */
const sectionName = computed(() => activeSectionObject.value?.name ?? 'this section')

/**
 * The confirmation, as a state of the button rather than as a dialog.
 *
 * **AC6 asks for a prompt, and the codebase has already answered the same
 * question the other way**: deleting a section takes all of its notes with it and
 * ships with no confirmation, because "the whole operation is one undo, and an
 * undoable action reads better as a reversible one than as a question"
 * (`SectionContextMenu`). There is also no dialog primitive here — `ui/` holds
 * checkbox, context-menu and dropdown-menu — so a modal would mean porting a
 * fourth one.
 *
 * The inline form satisfies the criterion without contradicting either. It is
 * two presses in the same place, it names the count so the scope is visible
 * before the second one, and the undo message still carries the real safety net.
 * Nothing is stolen from the surrounding UI, and there is no focus trap to unwind.
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

/** Re-armed whenever the offer stops being the one the user is looking at: a
 *  different set of targets under them, or the view they opened it from going
 *  away. Keyed on the whole filter rather than on `doneOnly`, so leaving the done
 *  view disarms whichever of the other two states the press lands in. */
watch([armedTargets, doneFilter], () => {
	confirming.value = false
})

/**
 * Both presses have to be separate, deliberate ones, and two things defeat that
 * without the user meaning them to.
 *
 * **A held Enter or Space.** The browser synthesises a click from every repeat of
 * the keydown, so holding the key would arm on the first and confirm on the
 * second without it ever coming up — a confirmation nobody consented to, on the
 * one destructive control here. Handled at the source in `onKeydown` by refusing
 * the repeat, rather than here, because the honest fix is to stop the extra click
 * being generated at all.
 *
 * **The second click of a double-click**, which carries `detail === 2`. A
 * double-click is one gesture, and the label it was aimed at changed halfway
 * through it.
 *
 * Neither guard is a timer. A time-based rule would have to pick an interval that
 * is either long enough to swallow a genuine second press or short enough to miss
 * a slow repeat, and it would make the behaviour untestable without faking
 * clocks.
 */
function press(event: MouseEvent) {
	if (confirming.value && event.detail > 1) return

	if (!confirming.value) {
		confirming.value = true
		return
	}
	confirming.value = false
	void deleteDoneInActiveSection()
}

/**
 * Escape backs out of the offer without leaving the panel's own Escape ladder a
 * rung short: the press is consumed only while there is something to cancel.
 *
 * The repeat guard is the other half of `press`'s "two deliberate presses" rule —
 * see there for why it lives on the keydown.
 */
function onKeydown(event: KeyboardEvent) {
	if (event.repeat && (event.key === 'Enter' || event.key === ' ')) {
		event.preventDefault()
		return
	}
	if (event.key !== 'Escape' || !confirming.value) return
	event.preventDefault()
	event.stopPropagation()
	confirming.value = false
}

/** What the confirming press would take, in the words the armed button shows. */
const confirmLabel = computed(() => `Delete ${doneCount.value} done?`)

/**
 * The accessible name, which at rest is the *only* name this button has: it is an
 * icon and nothing else until it is armed. It names the section in both states —
 * the view is document-wide and this is not, so a screen reader landing on
 * "Delete done notes" would be told a scope the button does not have.
 *
 * Armed, it leads with the visible label verbatim and adds the scope after it.
 * That order is the one requirement the two halves cannot negotiate: a voice-input
 * user says the words they can see, and a name that merely *contains* them
 * somewhere is a name "click Delete 2 done" can miss.
 */
const label = computed(() =>
	confirming.value
		? `${confirmLabel.value} In ${sectionName.value}.`
		: countMessage(doneCount.value, {
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
		<!-- **Icon-only at rest, and it grows a label only while it is armed.** The
		     resting button is one of three controls in a strip a panel wide, and
		     "Delete done" beside a trash icon told the pointer nothing the icon had
		     not; the accessible name carries it for everyone the icon does not reach.

		     The armed state is the one place text is not decoration. This deletes the
		     *active* section's done notes and leaves every other section alone (AC9),
		     while the view behind it is document-wide — so the two can legitimately
		     disagree, and a user looking at nine done notes across three sections can
		     be offered two. A bare red icon confirms nothing at all, and "Delete 2?"
		     names neither what is being counted nor that the count is the point, so
		     the noun stays: the number is what makes the discrepancy visible.

		     The *scope* rides on the accessible name and the tooltip rather than on
		     the strip. A section is user text of any length, and the armed label
		     borrows its width from the chip beside it — a label that could grow to
		     any width is one that can push the chip out of a header sized to hold
		     both. Three words are a width the strip can promise, and it is a state
		     that lasts seconds, so what it borrows it gives back.

		     It leads the strip rather than following the filter it belongs to: the
		     destructive control is the one that must not sit where a mis-aimed press
		     can find it, and the filter is what the pointer arrives for. -->
		<button
			v-if="doneOnly && doneCount > 0"
			type="button"
			data-delete-done
			class="panel-button inline-flex min-h-6 shrink-0 items-center gap-1 px-1.5"
			:class="confirming ? 'text-destructive-text' : 'text-text-secondary'"
			:title="label"
			:aria-label="label"
			@click="press"
			@keydown="onKeydown"
			@blur="confirming = false"
		>
			<IconLucideTrash2 class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
			<!-- `min-w-0` and `truncate`: this button shares its row with the chip, and
			     a count has no upper bound. `uppercase` because `text-label` carries
			     0.06em of tracking, which is spacing cut for capitals — every other
			     label in this strip is set the same way. -->
			<span v-if="confirming" class="min-w-0 truncate text-label uppercase">{{
				confirmLabel
			}}</span>
		</button>

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
