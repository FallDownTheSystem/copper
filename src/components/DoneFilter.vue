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
const { doneCount, deleteDoneInActiveSection } = useNoteActions()
const { activeSectionObject } = useSpace()

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

/** Rearmed whenever the offer stops being the one the user is looking at: a
 *  count that changed under them, or the view they opened it from going away. */
watch([doneCount, doneOnly], () => {
	confirming.value = false
})

function press() {
	if (!confirming.value) {
		confirming.value = true
		return
	}
	confirming.value = false
	void deleteDoneInActiveSection()
}

/** Escape backs out of the offer without leaving the panel's own Escape ladder a
 *  rung short: the press is consumed only while there is something to cancel. */
function onKeydown(event: KeyboardEvent) {
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

		<!-- Named for the scope it actually has. "Delete all done" would overstate
		     it: this deletes the *active* section's done notes and leaves every other
		     section alone (AC9), and the section is the thing the user has to be able
		     to check before pressing twice. -->
		<button
			v-if="doneOnly && doneCount > 0"
			type="button"
			data-delete-done
			class="panel-button inline-flex min-h-6 shrink-0 items-center gap-1 px-1.5"
			:class="confirming ? 'text-destructive' : 'text-text-secondary'"
			:title="`Delete the ${doneCount} done notes in ${activeSectionObject?.name ?? 'this section'}`"
			@click="press"
			@keydown="onKeydown"
			@blur="confirming = false"
		>
			<IconLucideTrash2 class="size-3.5 shrink-0" aria-hidden="true" focusable="false" />
			<span class="text-label">
				{{ confirming ? `Delete ${doneCount}?` : 'Delete done' }}
			</span>
		</button>
	</div>
</template>
