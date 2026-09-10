<script setup lang="ts">
import type { Attachment } from '@/composables/useAttachments'
import { attachmentKey, useAttachmentSelection } from '@/composables/useAttachmentSelection'
import { useAttachmentActions } from '@/composables/useAttachmentActions'
import { useAttachmentDrag } from '@/composables/useAttachmentDrag'
import type { AttachmentCopyFormat } from '@/composables/useSystemClipboard'
import { CHORDS } from '@/lib/chords'

const props = defineProps<{
	attachment: Attachment
	note: string
	/** Task-004's interaction mode: `0` makes this reachable by Tab inside the
	 *  focused row, `-1` keeps it out of the tab order like every other in-card
	 *  control. */
	tabIndex: number
}>()

const emit = defineEmits<{ message: [string] }>()

const { previewFor, requestPreview, previewEpoch, openAttachment, revealAttachment } =
	useAttachments()
const { open: openViewer } = useImageViewer()
const { boundary, portalTo } = useOverlayHost()

const button = useTemplateRef<HTMLButtonElement>('button')
const selection = useAttachmentSelection()
const actions = useAttachmentActions()
const target = computed(() => ({ note: props.note, attachment: props.attachment.id }))
const selected = computed(() => selection.isSelected(target.value))
const copyTargets = computed(() => selection.copyTargets(target.value))
const buttonId = computed(() => `attachment-${attachmentKey(target.value)}`)

function onMousedown(event: MouseEvent) {
	if (!(event.target instanceof Element) || !event.target.closest('button')) event.preventDefault()
}

const drag = useAttachmentDrag()
let press: {
	id: number
	x: number
	y: number
	started: boolean
	controller: AbortController
} | null = null
let dragged = false

function stopPress() {
	press?.controller.abort()
	press = null
	window.removeEventListener('pointermove', onPointermove)
	window.removeEventListener('pointerup', onPointerup)
	window.removeEventListener('pointercancel', onPointerup)
	window.removeEventListener('blur', stopPress)
	window.removeEventListener('keydown', onDragKeydown, true)
}

function onPointerdown(event: PointerEvent) {
	stopPress()
	dragged = false
	if (event.button !== 0 || event.pointerType !== 'mouse' || unavailable.value) return
	press = {
		id: event.pointerId,
		x: event.clientX,
		y: event.clientY,
		started: false,
		controller: new AbortController(),
	}
	window.addEventListener('pointermove', onPointermove)
	window.addEventListener('pointerup', onPointerup)
	window.addEventListener('pointercancel', onPointerup)
	window.addEventListener('blur', stopPress)
	window.addEventListener('keydown', onDragKeydown, true)
}

function onPointerup(event: PointerEvent) {
	if (event.pointerId === press?.id) stopPress()
}

function onDragKeydown(event: KeyboardEvent) {
	if (event.key !== 'Escape') return
	if (press?.started) {
		event.preventDefault()
		event.stopPropagation()
	}
	stopPress()
}

function onPointermove(event: PointerEvent) {
	const gesture = press
	if (!gesture || gesture.id !== event.pointerId) return
	if (!(event.buttons & 1)) {
		stopPress()
		return
	}
	if (gesture.started || Math.hypot(event.clientX - gesture.x, event.clientY - gesture.y) < 6)
		return
	gesture.started = true
	dragged = true
	window.getSelection()?.removeAllRanges()
	if (!selected.value) selection.select(target.value, event)
	button.value?.focus()
	void drag.start(target.value, gesture.controller.signal).finally(() => {
		if (press === gesture) stopPress()
	})
}

onUnmounted(stopPress)

function onClick(event: MouseEvent) {
	if (dragged) return
	window.getSelection()?.removeAllRanges()
	selection.select(target.value, event)
	button.value?.focus()
}

function onFocus() {
	selection.focus(target.value)
}

function onContextMenu() {
	selection.ensure(target.value)
	button.value?.focus()
}

function onKeydown(event: KeyboardEvent) {
	if (event.defaultPrevented || event.altKey) return
	if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') {
		event.preventDefault()
		event.stopPropagation()
		selection.selectAllInNote(props.note)
	} else if ((event.ctrlKey || event.metaKey) && event.key === ' ') {
		event.preventDefault()
		event.stopPropagation()
		selection.toggle(target.value)
	} else if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
		event.preventDefault()
		event.stopPropagation()
		const next = selection.move(target.value, event.key === 'ArrowDown' ? 1 : -1, event)
		if (next) document.getElementById(`attachment-${attachmentKey(next)}`)?.focus()
	}
}

function onOpenKeydown(event: KeyboardEvent) {
	if (event.ctrlKey || event.metaKey || event.altKey) return
	if (event.key !== 'Enter' && event.key !== ' ') return
	event.preventDefault()
	event.stopPropagation()
	void activate()
}

function copy(format: AttachmentCopyFormat = 'auto') {
	void actions.copy(format, copyTargets.value)
}

// The ask, driven from a watcher rather than from the read below: `previewFor`
// is consumed by a computed, and requesting as a side effect of reading would
// write the preview cache during that computed's evaluation. `immediate` keeps
// the two inseparable — a card cannot render without having asked.
//
// The epoch is the second dependency and is not optional. `clearPreviews`
// revokes the cache under cards that are still mounted showing the same file,
// which leaves them with no preview and nothing outstanding; watching the file
// alone, a card in that state would sit on a spinner forever. Reading as a side
// effect of rendering used to cover this for free, and this is what replaces it.
watch(
	() => [props.attachment.file, previewEpoch.value] as const,
	([file]) => requestPreview(file),
	{ immediate: true },
)

// Read once, right after the immediate watcher above has already asked: a
// thumbnail that was cached is `ready` by now, and one that has to be decoded is
// not. Only the second kind should fade — a card whose image is already there at
// panel open would otherwise animate a picture the user is looking at, and every
// visible card would do it at once. Deliberately not reactive: the question is
// "was it there when this card mounted", which has exactly one answer.
const arrivedBeforeMount = previewFor(props.attachment.file).state === 'ready'

const preview = computed(() => previewFor(props.attachment.file))
const unavailable = computed(() => preview.value.state === 'missing')
const thumbUrl = computed(() => (preview.value.state === 'ready' ? preview.value.url : null))

/**
 * The leading box's size, fixed in **height** and derived in width from the
 * dimensions stored at ingest.
 *
 * This is what keeps a thumbnail arriving from reflowing the list. Task-004's
 * sticky-bottom pin measures `scrollHeight` and re-asserts until the list
 * settles, and a card that grew when its image finished loading would move the
 * content under a reader who had already scrolled. A constant height means the
 * arrival changes nothing vertically at all, whatever the image turns out to
 * be — which is a stronger guarantee than reserving space from `width`/`height`
 * alone, since those are advisory and absent for every non-image.
 */
const THUMB_HEIGHT = 56
const MAX_THUMB_WIDTH = 148

const boxStyle = computed(() => {
	const { width, height } = props.attachment
	const ratio = width && height && height > 0 ? width / height : 1
	const scaled = Math.round(THUMB_HEIGHT * ratio)
	return {
		width: `${Math.min(Math.max(scaled, THUMB_HEIGHT), MAX_THUMB_WIDTH)}px`,
		height: `${THUMB_HEIGHT}px`,
	}
})

/**
 * Whether this attachment has a picture, which is the only signal the frontend
 * has for "is this an image" — `is_thumbnailable` is decided in Rust from the
 * bytes on disk, and the document's `mime` is hand-editable.
 *
 * A `ready` preview with a **null** url is the honest description of a `.pdf`,
 * and it has to fall to the OS path below: opening the viewer on one would show
 * an empty sheet with no way to reach the file at all.
 */
const viewable = computed(() => preview.value.state === 'ready' && preview.value.url !== null)

/**
 * The primary gesture, and task-014 moves what it means.
 *
 * Double-click and `Enter` were task-011's route to the OS viewer; they are the
 * two gestures a file has everywhere, so they belong to the thing the user is
 * most likely to want — and for a screenshot pasted into a note, that is looking
 * at it, not launching Photos over the top of Copper.
 *
 * **`Space` no longer keeps the OS route.** It was a second gesture with a second
 * meaning on a control that is a `<button>`, and a button activates identically on
 * `Enter` and on `Space` everywhere the user has ever met one — a promise that
 * outranks the convenience of a spare key, because the cost of breaking it is
 * launching an external application the user did not ask for. The OS route moved
 * to the context menu, where it can carry a name instead of being something you
 * have to already know.
 *
 * Anything with no picture keeps the old behaviour on every gesture: the viewer
 * has nothing to show it, and `attachment_open` reveals it in Explorer.
 */
async function activate() {
	if (unavailable.value) return
	if (viewable.value) {
		void openViewer(props.attachment, button.value)
		return
	}
	await openInSystem()
}

async function openInSystem() {
	if (unavailable.value) return
	const failure = await openAttachment(props.attachment.file)
	if (failure) emit('message', failure)
}

/**
 * The stored copy, selected in Explorer.
 *
 * Ingest *copies* the bytes into the space's sidecar and never links the
 * original, so this shows the only file there is to show — and for an image it
 * is the only route to it at all, since every other gesture on this card ends at
 * the OS viewer.
 */
async function revealInExplorer() {
	const failure = await revealAttachment(props.attachment.file)
	if (failure) emit('message', failure)
}
</script>

<template>
	<!-- **A menu of its own, nested inside the row's.** An attachment is a file, and
	     right-clicking a file asks about that file rather than about what contains
	     it — so this card answers for itself and the note menu keeps the rest of the
	     row. Reka arbitrates the nesting without help: its trigger defers to a
	     `nextTick` and then declines an already-defaulted event, so the innermost
	     trigger opens and `NoteCard`'s outer one stands down. The file wrapper
	     stops propagation so the note's selection handler cannot replace the
	     attachment selection underneath the open menu.

	     Portalled into the panel's in-clip host for the reason the other two menus
	     are: teleported to `document.body` it would escape the rounded rect. -->
	<ContextMenu>
		<ContextMenuTrigger as-child>
			<div
				:data-attachment-note="note"
				:data-attachment-id="attachment.id"
				:data-selected="selected"
				class="squircle hover:bg-surface-hover flex min-w-0 items-center gap-1 rounded-lg border p-1.5"
				:class="selected ? 'border-accent-ring bg-surface-hover' : 'border-separator'"
				@pointerdown.stop="onPointerdown"
				@dragstart.stop.prevent
				@mousedown.stop="onMousedown"
				@click.stop="onClick"
				@dblclick.stop.prevent
				@contextmenu.stop="onContextMenu"
				@focusin="onFocus"
				@keydown="onKeydown"
			>
				<!-- Selection belongs to the file wrapper, not the note row. The button
			     retains Enter/Space and double-click to open the attachment.

			     The label names the primary gesture's destination rather than listing both:
			     a screen reader reading "View or open" on every thumbnail would be reading
			     the implementation.

			     **`aria-disabled`, never the native `disabled`.** A disabled button is
			     removed from the tab order and from the accessibility tree's reach, so
			     the one card that has something to explain — the file is missing, and
			     here is why — becomes the one card a keyboard cannot land on to hear it.
			     It also stops `contextmenu` from firing, which would take the wrapping
			     trigger with it and close the only route left to the folder the file
			     should be in. `activate` and `openInSystem` return early on their own, so
			     nothing here depends on the attribute to refuse. -->
				<button
					ref="button"
					:id="buttonId"
					data-attachment-open
					type="button"
					:tabindex="tabIndex"
					:aria-pressed="selected"
					:title="
						unavailable
							? undefined
							: 'Click to select. Ctrl/Shift+click selects multiple files. Drag to another app. Double-click to open.'
					"
					:aria-disabled="unavailable ? 'true' : undefined"
					:aria-label="
						unavailable
							? `${attachment.name} (unavailable)`
							: `${viewable ? 'View' : 'Open'} ${attachment.name}, ${formatBytes(attachment.bytes)}`
					"
					class="squircle focus-ring flex min-h-14 min-w-0 flex-1 items-center gap-2 rounded-md text-left aria-disabled:cursor-default"
					@click.prevent
					@dblclick.stop.prevent="!dragged && activate()"
					@keydown="onOpenKeydown"
				>
					<span
						class="bg-surface-hover text-text-disabled grid shrink-0 place-items-center overflow-hidden rounded-md"
						:style="boxStyle"
					>
						<!-- No `alt` text of its own: the button already carries the filename, and
					     a second announcement of the same name is noise. -->
						<img
							v-if="thumbUrl"
							:src="thumbUrl"
							alt=""
							class="size-full object-cover"
							:class="arrivedBeforeMount ? '' : 'animate-in fade-in duration-fast ease-out-quint'"
							draggable="false"
						/>
						<IconLucideTriangleAlert
							v-else-if="unavailable"
							class="text-destructive size-5"
							aria-hidden="true"
							focusable="false"
						/>
						<IconLucideFile v-else class="size-5" aria-hidden="true" focusable="false" />
					</span>

					<span class="min-w-0 flex-1">
						<!-- The **original** filename, which is metadata. The stored name is a
					     content hash and means nothing to anyone. -->
						<span class="text-text-primary block truncate text-meta">{{ attachment.name }}</span>
						<span
							v-if="unavailable"
							class="text-destructive-text mt-0.5 block text-meta line-clamp-2"
						>
							{{ preview.state === 'missing' ? preview.reason : '' }}
						</span>
						<span v-else class="text-text-secondary mt-0.5 block text-meta">
							{{ formatBytes(attachment.bytes) }}
						</span>
					</span>
				</button>
			</div>
		</ContextMenuTrigger>

		<ContextMenuContent
			v-if="portalTo"
			:to="portalTo"
			:collision-boundary="boundary ?? undefined"
			:collision-padding="8"
			class="text-text-secondary w-56 text-meta"
		>
			<ContextMenuItem
				class="min-h-6"
				:disabled="unavailable || actions.copying.value"
				@select="copy()"
			>
				{{
					copyTargets.length > 1
						? `Copy ${copyTargets.length} attachments`
						: viewable
							? 'Copy image'
							: 'Copy attachment'
				}}
				<ContextMenuShortcut>{{ CHORDS.copy.display }}</ContextMenuShortcut>
			</ContextMenuItem>
			<ContextMenuItem
				v-if="viewable && copyTargets.length === 1"
				class="min-h-6"
				:disabled="actions.copying.value"
				@select="copy('files')"
			>
				Copy as file
			</ContextMenuItem>
			<ContextMenuItem
				class="min-h-6"
				:disabled="unavailable || actions.copying.value"
				@select="copy('paths')"
			>
				{{ copyTargets.length > 1 ? 'Copy paths' : 'Copy path' }}
			</ContextMenuItem>
			<ContextMenuItem class="min-h-6" @select="selection.selectAllInNote(note)">
				Select all attachments
			</ContextMenuItem>
			<ContextMenuSeparator />

			<!-- Where `Space` used to go. A gesture nobody can see is not a feature, and
			     the OS route is the one thing this card does that leaves Copper entirely
			     — so it belongs somewhere it can say so before it happens.
			     Disabled rather than hidden when the file is missing: the entry is what
			     tells the user this card *has* an OS route, and a menu that changes shape
			     is a menu they have to re-read. -->
			<ContextMenuItem class="min-h-6" :disabled="unavailable" @select="openInSystem">
				Open in default app
			</ContextMenuItem>

			<!-- "Location" rather than "folder": what the user is being shown is where
			     Copper put its copy, and the sidecar directory is not somewhere they
			     chose. Never disabled, unlike the entry above: a missing file is exactly
			     when someone wants to be shown where it should have been. -->
			<ContextMenuItem class="min-h-6" @select="revealInExplorer">
				Open attachment location
			</ContextMenuItem>
		</ContextMenuContent>
	</ContextMenu>
</template>
