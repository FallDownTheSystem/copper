<script setup lang="ts">
import { focusRowSoon } from '@/composables/useSelection'
import type { Section } from '@/composables/useSpace'

const props = defineProps<{
	section: Section
	active: boolean
	rowId: string
}>()

const emit = defineEmits<{ activate: [] }>()

const { focusedId } = useSelection()
const { renaming, draft, setDraft, endRename, cancelRename } = useSectionEditor()
const { renameSection } = useSpace()

const focused = computed(() => focusedId.value === props.rowId)
const headingId = computed(() => `section-heading-${props.section.id}`)
const editing = computed(() => renaming.value === props.section.id)

const input = useTemplateRef<HTMLInputElement>('input')

// The field replaces the heading in place, so it has to take focus itself —
// nothing else moves focus into a control that did not exist a tick ago.
watch(editing, (open) => {
	if (!open) return
	void nextTick(() => {
		input.value?.focus()
		input.value?.select()
	})
})

/**
 * Enter and blur both land here. The session is ended *before* the write, so the
 * blur the unmounting field fires finds nothing open and returns — which is what
 * makes committing with Enter safe without a re-entry flag.
 */
async function commit() {
	if (!editing.value) return
	const write = endRename(props.section.name)
	// The row is the grid's tab stop; leaving focus on a field that is
	// unmounting drops it to the body and makes the list unreachable.
	focusRowSoon(props.rowId)
	if (write) await renameSection(write.id, write.name)
}

function onKeydown(event: KeyboardEvent) {
	// WebView2 reports keyCode 229 while an IME candidate is open, and accepting
	// one with Enter must not commit the rename.
	if (event.isComposing || event.keyCode === 229) return

	if (event.key === 'Enter') {
		event.preventDefault()
		event.stopPropagation()
		void commit()
	} else if (event.key === 'Escape') {
		event.preventDefault()
		event.stopPropagation()
		cancelRename()
		focusRowSoon(props.rowId)
	}
}
</script>

<template>
	<!-- A `grid` may own only `row` and `rowgroup`, and a `rowgroup` only `row`,
	     so the section header is itself a row rather than an <h2> sitting between
	     rowgroups. It pays for itself: the header becomes keyboard-reachable
	     through ordinary arrow navigation instead of needing a bespoke path.
	     Header rows carry no aria-selected — they are not selectable.

	     The context menu attached here is the *section* menu. A note menu must
	     not open on a header row, which is why the trigger lives on note rows
	     only and this one carries its own content. -->
	<ContextMenu>
		<ContextMenuTrigger as-child>
			<div
				role="row"
				:data-row-id="rowId"
				data-section-row
				:tabindex="focused ? 0 : -1"
				class="min-w-0 outline-focus-ring focus-visible:outline-2 focus-visible:-outline-offset-2"
			>
				<div role="gridcell" class="flex min-h-6 min-w-0 items-center gap-2 px-3">
					<template v-if="editing">
						<label :for="`section-rename-${section.id}`" class="sr-only">Section name</label>
						<input
							:id="`section-rename-${section.id}`"
							ref="input"
							:value="draft"
							type="text"
							autocomplete="off"
							class="border-separator bg-surface-hover text-text-primary outline-focus-ring h-6 min-w-0 flex-1 select-text rounded-md border px-1.5 text-label uppercase focus-visible:outline-2 focus-visible:-outline-offset-1"
							@input="setDraft(($event.target as HTMLInputElement).value)"
							@keydown="onKeydown"
							@blur="commit"
						/>
					</template>

					<template v-else>
						<h2 :id="headingId" class="min-w-0 shrink-0">
							<button
								type="button"
								tabindex="-1"
								:aria-current="active ? 'true' : undefined"
								class="hover:bg-surface-hover active:bg-surface-active flex items-center gap-1.5 rounded-md px-1.5 py-1 transition-colors duration-fast"
								:class="active ? 'text-accent-text' : 'text-text-secondary'"
								@click="emit('activate')"
							>
								<!-- Fixed-width slot, hidden rather than absent, so activating a
								     section shifts no text. -->
								<span
									aria-hidden="true"
									class="bg-accent-ring size-1.5 shrink-0 rounded-full transition-opacity duration-fast"
									:class="active ? 'opacity-100' : 'opacity-0'"
								/>
								<span class="truncate text-label uppercase" :class="active ? 'font-semibold' : ''">
									{{ section.name }}
								</span>
								<!-- The non-colour half of the active cue: colour alone would carry
								     the whole distinction. -->
								<span v-if="active" class="sr-only">(active section)</span>
							</button>
						</h2>
						<span aria-hidden="true" class="bg-separator h-px min-w-0 flex-1" />
					</template>
				</div>
			</div>
		</ContextMenuTrigger>

		<SectionContextMenu :section="section" />
	</ContextMenu>
</template>
