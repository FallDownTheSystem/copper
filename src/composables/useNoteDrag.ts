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
import { EASE_OUT_QUINT_CSS } from '@/lib/motion'

import { useNoteActions } from './useNoteActions'
import { rowNoteId } from './useSelection'
import { useSpace } from './useSpace'

/**
 * Distance from a scroll edge at which the list starts following the pointer.
 *
 * **It is also what keeps the pinned section heading from swallowing a drop.** A
 * heading pinned across the top of the region overlays the rows under it, so a
 * pointer held there resolves against rows the user cannot see — the one place
 * sticky positioning can lie to a hit test, since it moves what is painted and
 * not what is measured. It cannot be held there: the pinned band is 32px deep
 * (heading plus its vertical padding) and this band overshoots it by half again,
 * so every position that could resolve to a hidden row is already scrolling the
 * list toward the top, where nothing is hidden at all.
 * The drop indicator meanwhile paints at `z-20` against the heading's `z-1`, so
 * the line stays visible across it for the moment that takes.
 */
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
const space = useSpace()

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
		// Below the header row, which is where an empty section's first note lands.
		// A section always renders one, so the fallback is only for a shape that
		// does not currently exist.
		//
		// **The group's top plus the header's *height*, not the header's own
		// bottom.** The header is `position: sticky`: while it is pinned its rect
		// says where it is being painted rather than where it sits in the section,
		// and reading `bottom` off it would put an empty section's insertion line
		// wherever the heading had ridden to. A height is immune — sticky translates
		// a box, it does not resize one — and the two are the same number in every
		// other case, the header being the group's first child.
		const header = group.querySelector<HTMLElement>('[data-section-row]')
		sections.push({
			sectionId,
			top: box.top - origin,
			bottom: box.bottom - origin,
			contentTop: box.top - origin + (header?.getBoundingClientRect().height ?? 0),
		})

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

	// **Settle the list before reading a single rect.** auto-animate is mid-FLIP
	// for 150ms after any list change, and a row being animated reports its
	// *transformed* box — so a drag begun just after a capture landed would measure
	// rows at positions they are still travelling away from and drop the note
	// somewhere nobody pointed at. Finishing the animations puts every row at its
	// real place first. This also removes the ordering hazard in arming the drag:
	// the auto-animate stand-down watcher runs asynchronously off `draggingNoteId`,
	// so it cannot be relied on to have quieted anything by the time we measure.
	for (const animation of gesture.root.getAnimations?.({ subtree: true }) ?? []) {
		if (animation.playState === 'running') animation.finish()
	}

	layout = measure(gesture.root)
	draggedRow = row
	// Anchored to where the pointer went *down*, not to where it crossed the
	// threshold. Measuring from the crossing leaves the row trailing the pointer by
	// the activation distance for the whole gesture — the note never quite sits
	// under the hand carrying it.
	originY = gesture.startY - gesture.root.getBoundingClientRect().top
	row.dataset.dragging = ''
	draggingNoteId.value = gesture.noteId

	lastFrameAt = 0
	if (typeof requestAnimationFrame === 'function') frame = requestAnimationFrame(autoScroll)
}

function onUp(event: PointerEvent) {
	if (!gesture || event.pointerId !== gesture.pointerId) return

	const noteId = gesture.noteId
	const target = draggingNoteId.value === null ? null : dropTarget.value
	// `end` arms the click swallow itself whenever a drag was actually running, so
	// the drop and every abort below are covered by one rule.
	//
	// A release with nowhere to land is an abandonment like any other and the row
	// travels back. A release that commits does not: the reorder the next line
	// requests is what moves the row, through auto-animate, and animating it home
	// first would be the same row travelling twice for one gesture.
	end(target === null)

	if (!target) return
	void actions.finishDrag(noteId, target.sectionId, target.index)
}

function onCancel(event: PointerEvent) {
	if (!gesture || event.pointerId !== gesture.pointerId) return
	end(true)
}

/**
 * Capture lost to something other than us — the row unmounted underneath the
 * pointer, or the browser took it back. The gesture cannot receive another move,
 * so continuing to paint one would leave a row stuck under a pointer that no
 * longer drives it.
 */
function onLostCapture(event: PointerEvent) {
	if (!gesture || event.pointerId !== gesture.pointerId) return
	end(true)
}

/** Escape abandons the drag and is consumed here rather than left to the shell's
 *  ladder, which would go on to clear the selection or hide the panel. Capture
 *  phase for the same reason: the ladder is an ancestor listener and would
 *  otherwise see the press first. */
function onKeydown(event: KeyboardEvent) {
	if (event.key !== 'Escape' || draggingNoteId.value === null) return
	event.preventDefault()
	event.stopPropagation()
	end(true)
}

/**
 * How long an abandoned row takes to travel home, matching `--duration-base`.
 */
const SETTLE_MS = 150
/** Beyond the transition's own length, after which the cleanup runs regardless.
 *  `transitionend` does not fire for a row that was unmounted or re-rendered
 *  mid-flight, and the styles must not outlive the gesture either way. */
const SETTLE_FALLBACK_MS = 250

/** Completes an in-flight settle early, or null when none is running. Held at
 *  module scope because the thing that interrupts one is the *next* drag, and it
 *  may well be on the same row: without this its transform would be wiped by the
 *  previous gesture's timer. */
let cancelSettle: (() => void) | null = null

function clearDragStyles(row: HTMLElement) {
	row.style.transition = ''
	row.style.transform = ''
	delete row.dataset.dragging
	delete row.dataset.settling
}

/**
 * Walks an abandoned row back to where it started instead of teleporting it.
 *
 * The row was under the pointer a moment ago and is about to be somewhere else
 * entirely; cutting between the two in one frame gives the eye nothing to follow,
 * and the note reads as having been *replaced* rather than as having gone back.
 * Only abandonment gets this. A drop that lands is followed by auto-animate's own
 * FLIP, and two motions arguing over one row is worse than either alone.
 *
 * **`data-settling` rather than holding `data-dragging` through the return.** The
 * row does need to keep its surface and its raised stacking order for the trip,
 * or it travels home underneath the rows it passes — but `data-dragging` is not a
 * style hook, it is the document's answer to "is a gesture running", and
 * `useSelection` reads it to decide whether a captured note may scroll itself into
 * view. Left standing for the length of the animation it would swallow a capture
 * that landed in that window. The gesture is over the moment this is called; only
 * the picture is still catching up, so only the picture keeps an attribute.
 */
function settleHome(row: HTMLElement) {
	delete row.dataset.dragging
	row.dataset.settling = ''
	row.style.transition = `transform ${SETTLE_MS}ms ${EASE_OUT_QUINT_CSS}`
	row.style.transform = 'translateY(0)'

	let timer = 0
	const finish = () => {
		if (cancelSettle !== finish) return
		cancelSettle = null
		clearTimeout(timer)
		row.removeEventListener('transitionend', onTransitionEnd)
		clearDragStyles(row)
	}
	// The row's own transform and nothing else: colour transitions on the row and
	// on everything inside it bubble through here too, and any one of them would
	// otherwise cut the return short.
	function onTransitionEnd(event: TransitionEvent) {
		if (event.target === row && event.propertyName === 'transform') finish()
	}

	cancelSettle = finish
	row.addEventListener('transitionend', onTransitionEnd)
	timer = window.setTimeout(finish, SETTLE_FALLBACK_MS)
}

/**
 * Unwinds everything the gesture put in place. Safe to call at any point in it,
 * including before the drag threshold was ever crossed, and safe to call twice.
 *
 * **Every exit runs through here**, which is what makes the ghost-drag cases
 * impossible rather than merely unlikely: a `pointerup` delivered to another
 * window, a lost capture, an alt-tab, a document arriving from disk. Without it
 * the row keeps its transform and its raised z-index, `isDragging` stays true and
 * auto-animate stays switched off, and the auto-scroll loop keeps requesting
 * frames for a gesture nobody is performing.
 *
 * `settle` asks for the row's *visual* return to be animated, and is the one
 * thing here that outlives the call. Everything else — the click swallow, the
 * listeners, the pending frame, the pointer capture, the state the rest of the
 * app reads — is undone immediately whatever it is set to, because a gesture that
 * has ended has ended.
 */
function end(settle = false) {
	// Whatever the reason for this call, a settle still running belongs to a
	// gesture that is over. Finished here rather than left alone so that a drag
	// starting on the same row does not inherit a timer that will clear its
	// transform 150ms in.
	cancelSettle?.()

	// A drag that got as far as moving a row is always followed by a synthesised
	// `click` on the grip, whether it ended in a drop or was abandoned. Arming the
	// swallow here rather than only on the drop path is what stops Escape from
	// cancelling the reorder and then selecting the row anyway on release.
	if (draggingNoteId.value !== null) dragClickPending = true

	listeners?.abort()
	listeners = null

	if (frame !== 0) {
		cancelAnimationFrame(frame)
		frame = 0
	}

	if (draggedRow) {
		if (settle) settleHome(draggedRow)
		else clearDragStyles(draggedRow)
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
 * A document arriving mid-gesture ends it.
 *
 * The geometry was measured once, against a list that has now changed underneath
 * the pointer — so every row position the drop is resolved against is a
 * guess, and committing one would move a note somewhere nobody pointed at. A
 * capture landing from the global hotkey or an external edit to the `.copper`
 * file both land here. The drag the user was performing is abandoned rather than
 * completed on stale numbers.
 *
 * Not triggered by the drag's own commit: `end` has already run by then.
 */
watch(
	() => space.space.value,
	() => {
		// The one abandonment that does not animate the row home. Everything in the
		// list is about to move — auto-animate comes back on as `isDragging` falls,
		// and the new document is what it FLIPs to — so a settle here would be a
		// second transform on a row already being carried by the first.
		if (draggingNoteId.value !== null) end()
	},
)

/**
 * Arms a drag from the grip. Nothing is committed to yet — a press that never
 * travels far enough stays a press, which is what keeps the grip clickable and
 * keeps a twitchy mouse from reordering the list.
 *
 * The pointer is captured immediately even so, so that a fast drag that leaves
 * the grip behind still delivers its moves here.
 */
function beginDrag(noteId: string, event: PointerEvent) {
	// The primary button of the primary pointer, whatever kind of pointer it is.
	// Testing `pointerType === 'mouse'` first let a pen's barrel button and its
	// eraser end start a drag, and let a second finger begin one mid-gesture.
	if (event.button !== 0 || !event.isPrimary) return

	const handle = event.currentTarget
	if (!(handle instanceof HTMLElement)) return
	const root = handle.closest<HTMLElement>('[data-note-list]')
	const region = handle.closest<HTMLElement>('[data-scroll-region]')
	if (!root || !region) return

	// Any gesture still standing is stale — a second pointer, or a `pointerup`
	// that never arrived. Cleared instantly rather than settled: nobody is watching
	// a row go home at the moment they pick one up.
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
	window.addEventListener('pointercancel', onCancel, { signal })
	window.addEventListener('lostpointercapture', onLostCapture, { signal })
	window.addEventListener('keydown', onKeydown, { signal, capture: true })
	// The window losing focus is the one way a `pointerup` never arrives at all:
	// an alt-tab, or a click that raises another window, delivers the release
	// somewhere else entirely. Without this the row stays stuck to the cursor and
	// auto-animate stays switched off for the rest of the session.
	//
	// Wrapped rather than passed straight in: `end`'s first parameter would
	// otherwise be the `Event`, and every blur would ask for a settle by accident.
	window.addEventListener('blur', () => end(true), { signal })
	// The list can also move underneath a pointer that is holding still — a wheel,
	// a trackpad, a scrollbar drag. The drop target is a function of where the
	// pointer is *in the content*, so it has to be recomputed when the content
	// moves, not only when the pointer does.
	region.addEventListener('scroll', update, { signal, passive: true })
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
