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
const { doneCount, allDoneCount, allDoneTargets, deleteDoneInActiveSection, deleteAllDone } =
	useNoteActions()
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
 * The popover carries **two offers, not a yes/no**: the active section's done
 * notes and the whole document's (user ruling, 2026-08-10). That is what makes
 * the count-vs-view discrepancy actionable instead of merely visible — a user
 * looking at nine done notes across three sections used to be offered two with
 * no way to take the nine. It also moves the confirming press onto a separate
 * control, which retires the one-button form's guards: the second click of a
 * double-click merely closes what the first opened, and a held Enter's
 * synthesised repeat clicks are refused at the keydown on the content, so no
 * repeat can land on either delete. Escape and a click elsewhere dismiss it
 * through reka's own layer, which `inOverlay` keeps off the shell's ladder.
 *
 * Still not a modal dialog, deliberately: deleting a section ships with no
 * confirmation at all because the operation is one undo (`SectionContextMenu`),
 * and this prompt exists to pick a scope, not because the action is
 * unrecoverable. The undo message stays the real safety net.
 */
const confirming = ref(false)

/**
 * The armed offer's identity: *which* notes it would delete, not how many.
 *
 * Over the **document-wide** set, because the popover now makes two offers and
 * the wide one is a superset of the section one: any change to either offer
 * moves this value, so one watch withdraws both.
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
const armedTargets = computed(() => [...allDoneTargets.value].sort().join('\u0000'))

/** Withdrawn whenever the offer stops being the one the user is looking at: a
 *  different set of targets under them, or the view they opened it from going
 *  away. Keyed on the whole filter rather than on `doneOnly`, so leaving the done
 *  view closes the popover whichever of the other two states the press lands in. */
watch([armedTargets, doneFilter], () => {
	confirming.value = false
})

/** The two confirming presses, each on its own control inside the popover. */
function confirmSection() {
	confirming.value = false
	void deleteDoneInActiveSection()
}

function confirmAll() {
	confirming.value = false
	void deleteAllDone()
}

/**
 * The held-key guard, on the popover content rather than on either button.
 *
 * The browser synthesises a click from every repeat of an Enter keydown, and
 * reka autofocuses the content on open — so a held Enter that opened the
 * popover would land its repeats on whichever control took focus. Refusing the
 * repeat at the source stops the extra clicks being generated at all, which is
 * the same fix the old one-button form carried, moved to where the destructive
 * controls now live.
 */
function onContentKeydown(event: KeyboardEvent) {
	if (event.repeat && (event.key === 'Enter' || event.key === ' ')) {
		event.preventDefault()
	}
}

/**
 * The trigger's accessible name — the *only* name an icon-only button has.
 * It no longer names a count or a section: the button opens a scope choice,
 * and both scopes carry their own counts inside the popover where they are
 * visible to everyone at the moment of choosing.
 */
const label = 'Delete done notes'

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
		<Popover
			v-if="doneOnly && allDoneCount > 0"
			:open="confirming"
			@update:open="confirming = $event"
		>
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
			     way, exactly as it does for the menus.

			     Each offer carries its own count, because the number is what makes a
			     scope worth choosing — the two can legitimately disagree, and the
			     disagreement is the reason this asks at all. The section offer is
			     disabled rather than hidden at zero, so the two scopes keep their
			     places and a press aimed from memory cannot land on the wrong one. -->
			<PopoverContent
				v-if="portalTo"
				:to="portalTo"
				align="end"
				:collision-boundary="boundary ?? undefined"
				:collision-padding="8"
				class="w-64 text-meta"
				@keydown="onContentKeydown"
			>
				<p class="text-text-primary">Delete done notes?</p>
				<div class="mt-2 flex flex-col gap-1">
					<button
						type="button"
						data-delete-done-section
						class="panel-button text-destructive-text flex items-center justify-between gap-2 py-1 disabled:pointer-events-none disabled:opacity-50"
						:disabled="doneCount === 0"
						@click="confirmSection"
					>
						<span class="min-w-0 truncate">In {{ sectionName }}</span>
						<span class="shrink-0 tabular-nums">{{ doneCount }}</span>
					</button>
					<button
						type="button"
						data-delete-done-all
						class="panel-button text-destructive-text flex items-center justify-between gap-2 py-1"
						@click="confirmAll"
					>
						<span class="min-w-0 truncate">In all sections</span>
						<span class="shrink-0 tabular-nums">{{ allDoneCount }}</span>
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
