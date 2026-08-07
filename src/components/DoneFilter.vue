<script setup lang="ts">
/**
 * The done filter and, once it is on, the purge that goes with it.
 *
 * **It sits in the chip's row rather than the search row**, at the opposite end
 * from `ActiveSectionChip`. That row exists precisely so the heading area can gain
 * and lose controls without the search field ever moving (`PanelHeader`), which is
 * how AC10 is satisfied structurally rather than by careful sizing: the chip keeps
 * its `min-w-0` and truncates, this keeps `shrink-0`, and neither can push the
 * other out of the header.
 *
 * The delete button appears only in the done view, which is AC5, and only when
 * there is something to delete — a button that explains it has nothing to do is
 * worse than one that is not there.
 */
const { doneOnly, toggleDoneFilter } = useNoteList()
const { doneCount, doneTargets, deleteDoneInActiveSection } = useNoteActions()
const { activeSectionObject } = useSpace()

/** Never blank, so the confirming label always names a scope. */
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
 *  away. */
watch([armedTargets, doneOnly], () => {
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
</script>

<template>
	<div class="ml-auto flex shrink-0 items-center gap-1">
		<!-- A toggle rather than a segmented "all / active / done": the unfiltered
		     list already leads with the active notes, so a third state would divide
		     the same set twice. `aria-pressed` carries the state to a screen reader
		     and the accent carries it to everyone else. -->
		<button
			type="button"
			data-done-filter
			class="panel-button inline-flex min-h-6 shrink-0 items-center gap-1 px-1.5"
			:class="doneOnly ? 'text-accent-text' : 'text-text-secondary'"
			:aria-pressed="doneOnly"
			:title="doneOnly ? 'Show all notes' : 'Show done notes only'"
			@click="toggleDoneFilter"
		>
			<IconLucideCircleCheck class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
			<span class="text-label uppercase">Done</span>
		</button>

		<!-- **The confirming label names the section, not just the count.** This
		     deletes the *active* section's done notes and leaves every other section
		     alone (AC9), while the view behind it is document-wide — so the two can
		     legitimately disagree, and a user looking at nine done notes across three
		     sections can be offered two. A bare "Delete 2?" over a list of nine reads
		     as a bug or, worse, is taken at face value. The scope belongs in the
		     visible text rather than only in the `title`, which a keyboard user never
		     sees and a touch user cannot hover. -->
		<button
			v-if="doneOnly && doneCount > 0"
			type="button"
			data-delete-done
			class="panel-button inline-flex min-h-6 shrink-0 items-center gap-1 px-1.5"
			:class="confirming ? 'text-destructive' : 'text-text-secondary'"
			:title="`Delete the ${doneCount} done notes in ${sectionName}`"
			@click="press"
			@keydown="onKeydown"
			@blur="confirming = false"
		>
			<IconLucideTrash2 class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
			<!-- `min-w-0` and `truncate`: the section name is user text of any length,
			     and this button shares its row with the chip. -->
			<span class="min-w-0 truncate text-label">
				{{ confirming ? `Delete ${doneCount} in ${sectionName}?` : 'Delete done' }}
			</span>
		</button>
	</div>
</template>
