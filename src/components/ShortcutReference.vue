<script setup lang="ts">
/**
 * The whole keyboard, on demand — the reference `EmptyState` teaches once and
 * then never again. That card is on screen exactly until the first note exists;
 * afterwards the chords surface only as context-menu hints, and Ctrl+F, F2,
 * Alt+Arrow and the Escape ladder appear nowhere at all. This row is the
 * permanent copy, behind a disclosure so the Shortcuts section stays two
 * recorders and a line until someone asks.
 *
 * Everything with an entry in `CHORDS` is read from it, so a rebinding there
 * cannot leave this list teaching a dead press. The hard-wired rows — the
 * composer's Enter, Ctrl+F, F2, Escape, the heading keys — are written out
 * because their handlers are written out too (`Composer`, `PanelShell`,
 * `NoteList`); each names its owner so the two can be checked against each
 * other.
 *
 * The two *global* bindings are deliberately absent: they are rebindable and
 * live in the recorders directly above this row, which already show the real
 * caps. A second copy here could only agree or lie.
 */

import { CHORDS } from '@/lib/chords'

type Row = {
	/** What the press does, in the words the menus use for it. */
	action: string
	/** One or more chords, each a `+`-joined display string. Several chords render
	 *  separated by `/`, the way `Home / End` is said. */
	chords: readonly string[]
}

type Group = { heading: string; rows: Row[] }

const { enterKeyAction } = useSettings()

/** A computed rather than a constant for exactly one row: the writing group
 *  reads the `Enter key` setting, and a reference that showed the other mode's
 *  chords would be teaching a dead press. */
const GROUPS = computed<Group[]>(() => [
	{
		heading: 'With a note focused',
		rows: [
			{ action: 'Mark done or not done', chords: [CHORDS.markDone.display] },
			{ action: 'Edit', chords: [CHORDS.edit.display] },
			{ action: 'Edit in editor', chords: [CHORDS.openInEditor.display] },
			{ action: 'Copy', chords: [CHORDS.copy.display] },
			{ action: 'Copy as list', chords: [CHORDS.copyAsList.display] },
			{ action: 'Merge notes', chords: [CHORDS.merge.display] },
			{ action: 'Delete', chords: [CHORDS.remove.display, CHORDS.remove.alias] },
			{ action: 'Move the note', chords: [CHORDS.reorderUp.display, CHORDS.reorderDown.display] },
			// `useInteractionMode`'s key. "Buttons and links" is the row's own scope:
			// F2 hands focus to the controls inside the note.
			{ action: 'Use buttons and links inside a note', chords: ['F2'] },
			// `NoteList`'s Home/End handling.
			{ action: 'Jump to the first or last row', chords: ['Home', 'End'] },
		],
	},
	{
		heading: 'On a section heading',
		rows: [
			{ action: 'Collapse or expand', chords: ['Space'] },
			{ action: 'Make it the active section', chords: ['Enter'] },
			{ action: 'Collapse or expand, by direction', chords: ['←', '→'] },
			// `NoteList`'s Delete case: asks first, in the header's popover.
			{
				action: 'Delete the section and its notes',
				chords: [CHORDS.remove.display, CHORDS.remove.alias],
			},
			{
				action: 'Move the section',
				chords: [CHORDS.reorderUp.display, CHORDS.reorderDown.display],
			},
		],
	},
	{
		heading: 'Anywhere in the panel',
		rows: [
			// The grid's Tab order: every row is a sequential stop, so Tab and the
			// arrows walk the same list — Tab also enters and leaves it.
			{ action: 'Move between rows', chords: ['↑', '↓', 'Tab', 'Shift+Tab'] },
			{ action: 'Open the command palette', chords: [CHORDS.commandPalette.display] },
			// `PanelShell`'s hard-wired Ctrl+F.
			{ action: 'Search', chords: ['Ctrl+F'] },
			{ action: 'Undo', chords: [CHORDS.undo.display] },
			{ action: 'Redo', chords: [CHORDS.redo.display] },
			// The ladder: close what is open, clear what is set, then hide the panel.
			{ action: 'Close, clear, then hide, one step at a time', chords: ['Escape'] },
		],
	},
	{
		// One group for the composer and the inline editor, because the `Enter
		// key` setting gives them one matrix.
		heading: 'When writing a note',
		rows:
			enterKeyAction.value === 'newline'
				? [
						{ action: 'Add or save the note', chords: ['Ctrl+Enter'] },
						{ action: 'New line', chords: ['Enter', 'Shift+Enter'] },
					]
				: [
						{ action: 'Add or save the note', chords: ['Enter'] },
						{ action: 'New line', chords: ['Ctrl+Enter', 'Shift+Enter'] },
					],
	},
])

const open = ref(false)
const list = useTemplateRef<HTMLElement>('list')

/** The Share guide's disclosure, exactly: focus stays on the toggle, and the
 *  scroll moves the least amount that brings the revealed list into view —
 *  the row sits at the section's foot, so expanding would otherwise change
 *  nothing the reader can see. Unanimated for the guide's reason too: a height
 *  animation over a block this tall is the expensive kind. */
function toggle() {
	open.value = !open.value
	if (!open.value) return
	void nextTick(() => list.value?.scrollIntoView({ block: 'nearest' }))
}

function caps(chord: string): string[] {
	return chord.split('+')
}
</script>

<template>
	<SettingsRow label="All shortcuts" description="Every key the panel answers to, in one list.">
		<template #below>
			<div class="mt-2 flex items-center gap-2">
				<!-- `aria-expanded` and no `aria-controls`, as the Share guide's toggle:
				     the list renders only while open, and an id that does not exist yet
				     is worse than none. The label carries the state for the same case. -->
				<button
					type="button"
					class="panel-button hit-44 relative h-8 px-2 text-meta"
					:aria-expanded="open"
					data-testid="shortcut-reference-toggle"
					@click="toggle"
				>
					{{ open ? 'Hide all shortcuts' : 'Show all shortcuts' }}
				</button>
			</div>

			<div
				v-if="open"
				ref="list"
				data-testid="shortcut-reference"
				class="border-separator mt-2 space-y-3 rounded-md border p-3"
			>
				<!-- `h3` under `SettingsSection`'s `h2`, extending the outline the way
				     the Share guide's sections do. The rows are `EmptyState`'s dl:
				     action as the term, caps as its description, a shared `min-h-7`
				     floor instead of gaps so every band measures the same. -->
				<section v-for="group in GROUPS" :key="group.heading">
					<h3 class="text-text-primary text-meta font-semibold">{{ group.heading }}</h3>
					<dl class="mt-1">
						<div
							v-for="row in group.rows"
							:key="row.action"
							class="flex min-h-7 min-w-0 items-center gap-3"
						>
							<dt class="text-text-secondary min-w-0 flex-1 text-meta">{{ row.action }}</dt>
							<dd
								class="text-text-secondary flex shrink-0 flex-wrap items-center justify-end gap-1 text-meta"
							>
								<template v-for="(chord, index) in row.chords" :key="chord">
									<!-- Hidden like KbdChord's own `+`: "Alt plus up slash Alt
									     plus down" is not how anyone says it. -->
									<span v-if="index > 0" class="text-text-disabled text-meta" aria-hidden="true"
										>/</span
									>
									<KbdChord :keys="caps(chord)" />
								</template>
							</dd>
						</div>
					</dl>
				</section>
			</div>
		</template>
	</SettingsRow>
</template>
