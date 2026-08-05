<script setup lang="ts">
import type { Section } from '@/composables/useSpace'

const props = defineProps<{
	section: Section
	active: boolean
	rowId: string
}>()

const emit = defineEmits<{ activate: [] }>()

const { focusedId } = useSelection()

const focused = computed(() => focusedId.value === props.rowId)
const headingId = computed(() => `section-heading-${props.section.id}`)
</script>

<template>
	<!-- A `grid` may own only `row` and `rowgroup`, and a `rowgroup` only `row`,
	     so the section header is itself a row rather than an <h2> sitting between
	     rowgroups. It pays for itself: the header becomes keyboard-reachable
	     through ordinary arrow navigation instead of needing a bespoke path.
	     Header rows carry no aria-selected — they are not selectable. -->
	<div
		role="row"
		:data-row-id="rowId"
		:tabindex="focused ? 0 : -1"
		class="min-w-0 outline-focus-ring focus-visible:outline-2 focus-visible:-outline-offset-2"
	>
		<div role="gridcell" class="flex min-h-6 min-w-0 items-center gap-2 px-3">
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
		</div>
	</div>
</template>
