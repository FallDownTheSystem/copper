/**
 * Pointer-drag reordering for the note list.
 *
 * **Why this exists rather than a library.** Reordering ran on
 * `@formkit/drag-and-drop`, which falls back to the HTML5 drag API for mouse
 * pointers. Measured in the real WebView2: `dragstart` fires and then nothing
 * does — no `drag`, no `dragover`, no `drop` — because wry owns the webview's
 * `IDropTarget` and answers `DROPEFFECT_NONE` for a payload with no `CF_HDROP`.
 * The gesture dies at the webview boundary and `dragend` arrives ~370ms later.
 * The library's synthetic path is not a way out either: the native `dragstart`
 * fires `pointercancel` a millisecond later, so the pointer stream a synthetic
 * drag would have to ride is destroyed before it starts. Mouse reordering has
 * therefore never worked in this app.
 *
 * So the gesture is ours: pointer events only, no HTML5 drag API anywhere near
 * it.
 *
 * **The list is not reordered while the drag runs.** The dragged row is
 * translated under the pointer and a line is painted where it would land;
 * everything else holds still. That keeps the geometry measured at drag start
 * valid for the whole gesture, and it means the commit can hand `reorder_note` a
 * section and an index outright instead of reading a mutated DOM back.
 *
 * **Module scope, and the DOM written directly.** A drag touches exactly one row
 * and one indicator. Routing the pointer position through a ref every note card
 * renders from would rebuild all 200 rows on every `pointermove` — the same trap
 * the selection composable documents. The transform goes straight onto the row
 * element; only the indicator, which one component owns, is reactive.
 */

import {
	passedThreshold,
	resolveDrop,
	type DragLayout,
	type DragRow,
	type DragSection,
	type DropTarget,
} from '@/lib/dragGeometry'

import { useNoteActions } from './useNoteActions'
import { rowNoteId } from './useSelection'

/** Distance from a scroll edge at which the list starts following the pointer. */
const EDGE_PX = 48
/**
 * Fastest the edge scroll runs, in pixels **per second** at the very edge.
 *
 * Per second rather than per frame, and the difference is not academic: measured
 * in WebView2 on a high-refresh display, a per-frame step ran the list at over
 * 3000px/s — four times what the same constant does at 60Hz, and far too fast to
 * aim with. Scroll speed is a thing the user feels, so it is expressed in the
 * units they feel it in.
 */
const EDGE_SPEED_PX_PER_SECOND = 800
/** Caps the step a stalled frame can take, so coming back from a hitch does not
 *  fling the list. */
const MAX_FRAME_MS = 100

type Gesture = {
	noteId: string
	pointerId: number
	handle: HTMLElement
	/** The element every coordinate is measured against. */
	root: HTMLElement
	region: HTMLElement
	startX: number
	startY: number
}

/** The row being carried, or null. Read by the list to decide whether a drop
 *  indicator is showing; deliberately not read per card. */
const draggingNoteId = ref<string | null>(null)
const dropTarget = shallowRef<DropTarget | null>(null)

const isDragging = computed(() => draggingNoteId.value !== null)

/** Set for exactly as long as it takes the `click` that follows a drop to
 *  arrive, so the grip can swallow it. Without this, letting go of a note also
 *  selects it — the pointer went down and up on the same element, which is a
 *  click by every definition the browser has. */
let dragClickPending = false

let gesture: Gesture | null = null
let layout: DragLayout | null = null
let listeners: AbortController | null = null
let frame = 0
/** The dragged row and where the pointer was, in list-root coordinates, when the
 *  drag became active. */
let draggedRow: HTMLElement | null = null
let originY = 0
let pointerClientY = 0
/** When the edge scroll last ran, so its speed is measured in seconds rather
 *  than in frames. */
let lastFrameAt = 0

const actions = useNoteActions()

/**
 * Measures every row and section once, at the moment the drag starts.
 *
 * Once is enough because nothing reorders during the gesture, and it is
 * *necessary* rather than merely cheap: re-measuring mid-drag would read the
 * dragged row at its translated position and let it push its own drop target
 * around.
 */
function measure(root: HTMLElement): DragLayout {
	const origin = root.getBoundingClientRect().top
	const sections: DragSection[] = []
	const rows: DragRow[] = []

	for (const group of root.querySelectorAll<HTMLElement>('[data-section-id]')) {
		const sectionId = group.dataset.sectionId
		if (sectionId === undefined) continue

		const box = group.getBoundingClientRect()
		sections.push({ sectionId, top: box.top - origin, bottom: box.bottom - origin })

		for (const row of group.querySelectorAll<HTMLElement>('[data-note-row]')) {
			const noteId = rowNoteId(row.dataset.rowId ?? null)
			if (noteId === null) continue
			const rowBox = row.getBoundingClientRect()
			rows.push({ noteId, sectionId, top: rowBox.top - origin, bottom: rowBox.bottom - origin })
		}
	}

	return { sections, rows }
}

/** The pointer's Y in list-root coordinates. Re-read every time rather than
 *  cached, because the region scrolls under it — that is exactly how an
 *  auto-scroll keeps moving the drop target while the pointer holds still. */
function contentY(root: HTMLElement): number {
	return pointerClientY - root.getBoundingClientRect().top
}

function update() {
	if (!gesture || !layout || !draggedRow) return
	const y = contentY(gesture.root)
	// A direct 1:1 mapping from pointer to row, not an animation — there is no
	// duration here for `useReducedMotion` to have an opinion about.
	draggedRow.style.transform = `translateY(${y - originY}px)`
	dropTarget.value = resolveDrop(y, layout, gesture.noteId)
}

/**
 * Follows the pointer past the ends of the scroll region, so a note can be
 * dragged somewhere that was off screen when the drag started.
 */
function autoScroll() {
	if (!gesture || draggingNoteId.value === null) {
		frame = 0
		return
	}

	const now = performance.now()
	// The first frame moves nothing: there is no interval to measure it over yet.
	const seconds = lastFrameAt === 0 ? 0 : Math.min(now - lastFrameAt, MAX_FRAME_MS) / 1000
	lastFrameAt = now

	const box = gesture.region.getBoundingClientRect()
	const above = box.top + EDGE_PX - pointerClientY
	const below = pointerClientY - (box.bottom - EDGE_PX)

	// Ramped by how deep into the band the pointer is, so nudging the boundary
	// creeps and pinning the very edge moves properly.
	const speed = EDGE_SPEED_PX_PER_SECOND * seconds
	let delta = 0
	if (above > 0) delta = -(Math.min(above, EDGE_PX) / EDGE_PX) * speed
	else if (below > 0) delta = (Math.min(below, EDGE_PX) / EDGE_PX) * speed

	if (delta !== 0) {
		const before = gesture.region.scrollTop
		gesture.region.scrollTop += delta
		// Only when the region actually moved: at either end it cannot, and
		// recomputing the same drop target every frame is pure waste.
		if (gesture.region.scrollTop !== before) update()
	}

	frame = requestAnimationFrame(autoScroll)
}

function onMove(event: PointerEvent) {
	if (!gesture || event.pointerId !== gesture.pointerId) return
	pointerClientY = event.clientY

	if (draggingNoteId.value === null) {
		if (!passedThreshold(event.clientX - gesture.startX, event.clientY - gesture.startY)) return
		activate()
	}

	update()
}

function activate() {
	if (!gesture) return
	const row = gesture.handle.closest<HTMLElement>('[data-note-row]')
	if (!row) return

	layout = measure(gesture.root)
	draggedRow = row
	originY = contentY(gesture.root)
	row.dataset.dragging = ''
	draggingNoteId.value = gesture.noteId

	lastFrameAt = 0
	if (typeof requestAnimationFrame === 'function') frame = requestAnimationFrame(autoScroll)
}

function onUp(event: PointerEvent) {
	if (!gesture || event.pointerId !== gesture.pointerId) return

	const noteId = gesture.noteId
	const target = draggingNoteId.value === null ? null : dropTarget.value
	end()

	if (!target) return
	// The press became a drag, so the `click` the browser is about to synthesise
	// belongs to the gesture rather than to the row under it.
	dragClickPending = true
	void actions.finishDrag(noteId, target.sectionId, target.index)
}

/** Escape abandons the drag and is consumed here rather than left to the shell's
 *  ladder, which would go on to clear the selection or hide the panel. Capture
 *  phase for the same reason: the ladder is an ancestor listener and would
 *  otherwise see the press first. */
function onKeydown(event: KeyboardEvent) {
	if (event.key !== 'Escape' || draggingNoteId.value === null) return
	event.preventDefault()
	event.stopPropagation()
	end()
}

/** Unwinds everything the gesture put in place. Safe to call at any point in it,
 *  including before the drag threshold was ever crossed. */
function end() {
	listeners?.abort()
	listeners = null

	if (frame !== 0) {
		cancelAnimationFrame(frame)
		frame = 0
	}

	if (draggedRow) {
		draggedRow.style.transform = ''
		delete draggedRow.dataset.dragging
		draggedRow = null
	}

	if (gesture?.handle.hasPointerCapture(gesture.pointerId)) {
		gesture.handle.releasePointerCapture(gesture.pointerId)
	}

	gesture = null
	layout = null
	draggingNoteId.value = null
	dropTarget.value = null
}

/**
 * Arms a drag from the grip. Nothing is committed to yet — a press that never
 * travels far enough stays a press, which is what keeps the grip clickable and
 * keeps a twitchy mouse from reordering the list.
 *
 * The pointer is captured immediately even so, so that a fast drag that leaves
 * the grip behind still delivers its moves here.
 */
function beginDrag(noteId: string, event: PointerEvent) {
	if (event.pointerType === 'mouse' && event.button !== 0) return

	const handle = event.currentTarget
	if (!(handle instanceof HTMLElement)) return
	const root = handle.closest<HTMLElement>('[data-note-list]')
	const region = handle.closest<HTMLElement>('[data-scroll-region]')
	if (!root || !region) return

	// Any gesture still standing is stale — a second pointer, or a `pointerup`
	// that never arrived.
	end()
	dragClickPending = false

	gesture = {
		noteId,
		pointerId: event.pointerId,
		handle,
		root,
		region,
		startX: event.clientX,
		startY: event.clientY,
	}
	pointerClientY = event.clientY
	handle.setPointerCapture(event.pointerId)

	listeners = new AbortController()
	const signal = listeners.signal
	window.addEventListener('pointermove', onMove, { signal })
	window.addEventListener('pointerup', onUp, { signal })
	window.addEventListener('pointercancel', end, { signal })
	window.addEventListener('keydown', onKeydown, { signal, capture: true })
}

/** True once per completed drag, for the grip to swallow the trailing click. */
function consumeDragClick(): boolean {
	const pending = dragClickPending
	dragClickPending = false
	return pending
}

export function useNoteDrag() {
	return {
		isDragging,
		draggingNoteId: readonly(draggingNoteId),
		dropTarget: shallowReadonly(dropTarget),
		beginDrag,
		consumeDragClick,
	}
}
