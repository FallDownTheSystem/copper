<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'

import { PopoverAnchor } from '@/components/ui/popover'
import { rowNoteId } from '@/composables/useSelection'
import { CHORDS, inOverlay, inTextSurface } from '@/lib/chords'
import { splitFlatList } from '@/lib/listPaste'
import { moveFocusOnArrow } from '@/lib/popoverFocus'

// `activeSectionObject` left with `EmptyState`, which is the only thing that
// named the destination and now asks for it itself.
const { loadState, refreshing, actionError, errorFor, noteCount, spaceName, initialize } =
	useSpace()
const { showListError } = useStatusMessage()

/**
 * A failed `list`-scope mutation is shown in the toast stack rather than only
 * reaching the assertive live region below, where it would be invisible to
 * everyone not using a screen reader. The store owns its lifecycle — it appears
 * when a shell operation is refused and leaves when a retry succeeds — so the
 * watch mirrors both directions and the toast carries no Dismiss of its own.
 * (This mirroring lived in StatusLine when the pill was a single surface.)
 *
 * `errorFor('list')`, never the bare `actionError`: the other scopes render
 * beside the surface that produced them — the composer's refusal sits under the
 * composer — and mirroring those here would say everything twice.
 */
watch(errorFor('list'), (message) => showListError(message))
const { setClampHeight } = useNoteDisclosure()
const { ensureHighlighter } = useMarkdown()
// `setOverlayHost` below is what fills the two refs every menu reads; each menu
// reads them from the composable itself rather than being handed them. The
// shell reads the pair back for the list-paste popover it owns itself.
const { setOverlayHost, boundary, portalTo } = useOverlayHost()
const { hasQuery, clearQuery, resultCount } = useNoteSearch()
const { open: openPalette } = usePalette()
const { selectedIds, isSelected, clear } = useSelection()
const { editingNoteId, cancel } = useNoteEditor()
const { isOpen: viewerOpen, close: closeViewer } = useImageViewer()
const { interactionRowId, exit } = useInteractionMode()
const { initialize: initializeHandoffs } = useEditorHandoff()
const { initialize: initializeSpaces } = useSpaces()
const {
	copyNotes,
	copyAsList,
	capturePaste,
	captureListPaste,
	merge,
	openInEditor,
	deleteNotes,
	moveFocusedBy,
	undo,
	redo,
	announceResults,
} = useNoteActions()

// The panel root. It carries `relative` rather than `panel-surface`: the surface
// itself — background, radius, clip — moved to `App.vue` when the settings view
// arrived, because those belong to the window rather than to either view. The
// `relative` is what `panel-surface` used to supply for the `absolute inset-0`
// portal host below.
//
// That explanation lives here rather than above the element it describes because
// a comment before a template's root element makes the component a *fragment*: the
// root then resolves to the comment node, and every listener bound to the element
// stops receiving anything dispatched at the component.
const root = useTemplateRef<HTMLElement>('root')
const portalHost = useTemplateRef<HTMLElement>('portalHost')
const clampProbe = useTemplateRef<HTMLElement>('clampProbe')
const header = useTemplateRef<{ focusSearch: () => void }>('header')
const composer = useTemplateRef<{ focus: () => void; restoreCaret: (event: Event) => void }>(
	'composer',
)

const empty = computed(() => loadState.value === 'ready' && noteCount.value === 0)

// `--note-clamp` is a calc() over other custom properties, and getComputedStyle
// returns it unevaluated — so it is measured off one real box, once, rather
// than per card.
function measureClamp() {
	const probe = clampProbe.value
	if (probe) setClampHeight(probe.getBoundingClientRect().height)
}

useResizeObserver(clampProbe, measureClamp)

const scrollRegion = useTemplateRef<HTMLElement>('scrollRegion')

/**
 * The width the vertical scrollbar actually takes, published as
 * `--scrollbar-space` on the region for the right-edge paddings to absorb —
 * see main.css for the split. Measured rather than assumed 11px: zoom, a
 * future scrollbar style, or an overlay scrollbar (which takes nothing) all
 * change the number, and `offsetWidth - clientWidth` is the number. The
 * observer fires exactly when it can change — the content box resizes when a
 * scrollbar arrives or leaves, and on every panel resize besides.
 */
function measureScrollbar() {
	const region = scrollRegion.value
	if (!region) return
	const taken = Math.max(0, region.offsetWidth - region.clientWidth)
	region.style.setProperty('--scrollbar-space', `${taken}px`)
}

useResizeObserver(scrollRegion, measureScrollbar)

onMounted(() => {
	setOverlayHost(root.value, portalHost.value)

	measureClamp()
	measureScrollbar()

	void initialize()
	void initializeHandoffs()
	// The recents list is pulled on mount for the same reason the document is:
	// a launch-with-file open happens during `setup()`, long before the webview
	// has registered a listener, and Tauri replays nothing.
	void initializeSpaces()
	// Fire and forget: until it resolves, fences render unhighlighted and the
	// panel is fully usable.
	void ensureHighlighter()

	// Focus has to be inside this root or the shell's keydown handler — the
	// Escape ladder and every in-panel chord — never sees a press: `document.body`
	// is an *ancestor* of this element, not a descendant, so a press there does not
	// bubble down here. It matters most on the way back from the settings view,
	// which unmounts and remounts this tree and would otherwise return the user to
	// a panel their keyboard could not reach.
	//
	// Only when focus is nowhere in particular, so nothing that already has it is
	// robbed — including the composer, which claims focus on the empty state a tick
	// later.
	if (document.activeElement === null || document.activeElement === document.body) {
		root.value?.focus()
	}
})

// Focus the composer only when the empty state actually renders — never during
// loading, which would let the panel steal focus before the space arrives.
watch(empty, (isEmpty) => {
	if (isEmpty) void nextTick(() => composer.value?.focus())
})

// The result count is announced, not rendered next to the field: the panel has
// one status region and a second count elsewhere would be a second thing to
// read. Keyed on both, so a capture landing mid-search re-announces the number
// it changed.
watch([hasQuery, resultCount], announceResults)

/**
 * **The `Escape` ladder — one ordered handler, not several competing listeners.**
 *
 * Two rungs resolve above this and never reach it, which is why they are not
 * `if` branches: the inline editor and the section-rename field stop
 * propagation, and an open menu is declined by the `inOverlay` guard in the
 * caller — reka does *not* `preventDefault` the press before this element sees
 * it, so the guard is the menu rung rather than a convenience.
 *
 * **The section switcher is one of those menus, and deliberately has no rung of
 * its own here.** Task-010 asked for a rung above `cancel inline edit`; it would
 * be dead code. Reka traps focus inside the open switcher, so every `Escape`
 * arrives with a target inside it, resolves at the guard above, and closes the
 * switcher without reaching this ladder at all — which is exactly the required
 * behaviour, including leaving the selection and the query untouched. A rung
 * that can never fire would only make a future reader think it did.
 *
 * A level with nothing to do is skipped rather than consuming the press, so
 * `Escape` in an empty search field with notes selected clears the selection.
 * The last rung is Phase 7's and always has something to do: with nothing left
 * to dismiss inside the panel, `Escape` dismisses the panel.
 *
 * Through `hide_panel` rather than `getCurrentWindow().hide()`, even though the
 * capability for the latter is granted: hiding also ends an open recording
 * session, and task-002 centralised the window operations in Rust so that a
 * second path could not end up doing half of one.
 *
 * The composer is a deliberate exception and not a rung: task-004 binds `Escape`
 * there to "move focus to the last note", and it consumes the press itself.
 *
 * **Task-014's image viewer is the new top rung, and unlike the section switcher
 * it is a real one.** The switcher is reka's, so it traps focus and resolves at
 * the `inOverlay` guard before this is ever consulted; the viewer is hand-rolled
 * and matches no reka slot, so without this rung `Escape` over an open image
 * would clear the search or hide the panel while the image stayed up.
 *
 * **Task-019's command palette is hand-rolled too and still has no rung**, which
 * is the one case where the two properties come apart. What decides it is the
 * focus trap, not who wrote the overlay: the palette wraps its contents in a
 * trapped `FocusScope`, so every `Escape` arrives with a target inside it and
 * resolves at the `inOverlay` guard — which now names the palette — before this
 * is reached. It closes itself there. The viewer needs a rung because it matches
 * nothing that guard tests, not because it is hand-rolled.
 */
function onEscape(event: KeyboardEvent) {
	if (viewerOpen.value) {
		event.preventDefault()
		closeViewer()
	} else if (editingNoteId.value) {
		event.preventDefault()
		cancel()
	} else if (interactionRowId.value) {
		event.preventDefault()
		exit()
	} else if (hasQuery.value) {
		event.preventDefault()
		clearQuery()
	} else if (selectedIds.value.length > 0) {
		event.preventDefault()
		clear()
	} else {
		event.preventDefault()
		void invoke('hide_panel')
	}
}

/**
 * Click-away deselect: a click in the list area that does not land on a
 * selected note clears the selection, so no note keeps its ring after the user
 * has visibly clicked somewhere else (user rulings, 2026-08-10 and
 * 2026-08-11).
 *
 * **Capture phase, and that is the fix rather than a detail.** The rule ran on
 * bubble, which let every row control that stops propagation punch a hole in
 * it: pressing another note's completion circle left the old selection
 * standing, because the circle's `@click.stop` kept the press from ever
 * arriving here. Capture sees the click before any control can swallow it, so
 * the rule holds for exactly the presses the controls consume — and the
 * orderings bubble used to provide implicitly become the explicit guards
 * below:
 *
 * - A modifier click is a selection-extending gesture — Ctrl adds, Shift
 *   ranges — never "away". Cleared first, a Ctrl+click would add one note to
 *   an emptied selection.
 * - The grip owns its clicks: a plain one selects through the row, and the
 *   click that trails a drag — committed or abandoned — is swallowed on the
 *   grip itself, which this listener would otherwise beat to the press.
 * - A click anywhere on a *selected* note's row is not away, wherever inside
 *   the row it lands — its own circle, its attachments, its editor.
 * - A click on an unselected note's row clears and then lets the row's own
 *   handler re-aim the selection, which is the order bubbling already
 *   produced.
 *
 * On the scroll region rather than the shell, and the bound is deliberate —
 * context menus portal outside it, so an item click can never deselect the
 * notes the action it runs is about to target. Section bands and the empty
 * space below the list both count as "away".
 */
function onRegionClick(event: MouseEvent) {
	if (event.ctrlKey || event.metaKey || event.shiftKey) return
	const target = event.target
	// `Element`, not `HTMLElement`: the press usually lands on the SVG inside
	// the very control this exists for.
	if (!(target instanceof Element)) return
	if (target.closest('[data-drag-handle]')) return
	const row = target.closest<HTMLElement>('[data-note-row]')
	const noteId = rowNoteId(row?.dataset.rowId ?? null)
	if (noteId !== null && isSelected(noteId)) return
	clear()
}

/**
 * The chord layer. It sits on the shell rather than on the grid because these
 * are panel-scoped bindings — the target set comes from `focusedId`, not from
 * where DOM focus happens to be — and because the ladder above has to be
 * reachable from the search field as well as from the list.
 */
function onShellKeydown(event: KeyboardEvent) {
	if (event.defaultPrevented) return

	// **Above the ladder, not below it.** An open menu owns the keyboard, and reka
	// traps focus inside its content — so this is what makes the menu rung real.
	// Reka listens on the window and does not `preventDefault` an `Escape` before
	// this handler sees it: this element is a DOM *ancestor* of the portalled
	// content, so the press bubbles here first. Left below, `Escape` cleared the
	// selection while the menu stayed open, and closing a submenu did both at once.
	if (inOverlay(event.target)) return

	if (event.key === 'Escape') {
		onEscape(event)
		return
	}

	// **The image viewer owns the keyboard while it is up**, exactly as an open
	// menu does — and below the ladder rather than above it for the same reason
	// the menu guard is above: this one has to let `Escape` through, because
	// closing the viewer is the ladder's job and there is no reka layer here to do
	// it. Without this, `Delete` over an open image would delete the notes
	// underneath it.
	if (viewerOpen.value) return

	// **Above the suppression guard, and the only chord that is.** Task-006's rule
	// is that no in-panel chord fires from a text surface; this is the documented
	// exception. It was the section switcher's, suppressed everywhere but the
	// composer — the only surface where "where does the next capture land" is the
	// question being asked. Task-019 gave the binding to the command palette, and
	// with it the condition went: "open the command palette" is asked from the
	// search field and the inline editor too, so there is no surface left to
	// suppress it in and `inComposer()` had no second caller.
	//
	// It still cannot reach task-008's shortcut recorder — that lives in the
	// settings view, which unmounts this tree, and it `preventDefault`s and
	// consumes every key but Tab besides.
	if (CHORDS.commandPalette.matches(event)) {
		event.preventDefault()
		openPalette()
		return
	}

	// No in-panel chord fires while a text-editing surface has focus. That is
	// three surfaces, not two: the composer, the inline editor **and the search
	// input**. Leaving the search field off would let Ctrl+Z undo a note
	// operation while the user is editing their query.
	if (inTextSurface(event.target)) return

	// Below the guard: from inside an open menu this would otherwise yank focus
	// out to the search field and leave the menu standing.
	if (event.key === 'f' && (event.ctrlKey || event.metaKey)) {
		event.preventDefault()
		header.value?.focusSearch()
		return
	}

	if (CHORDS.copy.matches(event)) {
		// A live text selection means the user is copying text, not notes. Not
		// preventing default is what lets the native copy run.
		if ((window.getSelection()?.toString() ?? '').length > 0) return
		event.preventDefault()
		void copyNotes()
		return
	}

	// preventDefault on both of these because they are Chromium DevTools chords
	// (inspect element, device toolbar) in a dev build.
	if (CHORDS.copyAsList.matches(event)) {
		event.preventDefault()
		void copyAsList()
		return
	}

	if (CHORDS.merge.matches(event)) {
		event.preventDefault()
		void merge()
		return
	}

	if (CHORDS.openInEditor.matches(event)) {
		event.preventDefault()
		void openInEditor()
		return
	}

	if (CHORDS.remove.matches(event)) {
		event.preventDefault()
		void deleteNotes()
		return
	}

	// Checked before undo: Ctrl+Shift+Z is a redo alias and would otherwise be
	// swallowed by the undo matcher's own Ctrl+Z.
	if (CHORDS.redo.matches(event)) {
		event.preventDefault()
		void redo()
		return
	}

	if (CHORDS.undo.matches(event)) {
		event.preventDefault()
		void undo()
		return
	}

	// The keyboard equivalent of a drag, so reordering is not pointer-only.
	if (CHORDS.reorderUp.matches(event) || CHORDS.reorderDown.matches(event)) {
		event.preventDefault()
		void moveFocusedBy(CHORDS.reorderDown.matches(event) ? 1 : -1)
	}
}

/**
 * **Zero-focus paste.** `Ctrl+V` anywhere in the open panel except a text
 * surface captures the clipboard, with no click into the composer first.
 *
 * A DOM `paste` listener rather than a chord in the layer above, and rather than
 * a new Rust clipboard reader. Chromium dispatches `paste` for `Ctrl+V` whatever
 * the focused element is, and its `clipboardData` carries the text — so the text
 * branch needs no round trip at all, and the attachment branch reaches
 * `attach_paste`, which opens the clipboard itself and was never focus-driven.
 * The alternative was a second IPC command whose only job would be to hand the
 * frontend a copy of text the event already has.
 *
 * On `document`, not on the panel root: `document.body` is an *ancestor* of that
 * root, so a press delivered there — which is where focus sits after the tray
 * shows the panel — would never bubble down to it. The listener goes with this
 * component; the settings view carries its own file-only counterpart, which
 * takes the attachment branch and leaves text alone — it has no composer to
 * show a capture in.
 *
 * **Text wins, and the rule is still Rust's.** A clipboard carrying text takes
 * the note branch here; one that does not falls through to `attach_paste`, which
 * returns nothing the moment `CF_UNICODETEXT` is present. The containment runs
 * this way round: taking the note branch requires *non-whitespace* text, and
 * Rust declines to attach for **any** `CF_UNICODETEXT` at all — so the clipboards
 * this treats as a note are a subset of the ones Rust would already have refused
 * to attach, and the two can never both claim the same paste. The gap between the
 * two conditions is a clipboard holding only whitespace, which falls to
 * `attach_paste` and comes back empty: a silent no-op, which is what it should be.
 *
 * The decision is all this handler makes. The mutation goes through
 * `capturePaste`, so it takes its turn in the same queue as every other action
 * rather than racing a paste a keystroke behind it — and reports on the composer's
 * surface, where `DropTarget` puts a failed drop and for the same reason.
 */
function onPaste(event: ClipboardEvent) {
	// The composer, the inline editor, the search field and both rename fields all
	// resolve here, and in every one of them Ctrl+V has a text-editing meaning that
	// this must not take. An open menu owns the keyboard the same way it does in
	// the chord layer.
	// The open image viewer joins them: it is not a text surface and not a reka
	// overlay, but a paste while it is up would silently add a note behind it.
	if (inTextSurface(event.target) || inOverlay(event.target) || viewerOpen.value) return

	// Read here rather than inside the queued action: `clipboardData` is live only
	// while the event is dispatching.
	const text = event.clipboardData?.getData('text/plain') ?? ''

	// A flat list is the one clipboard shape with two right answers — one note,
	// or one note per item — so it is asked about rather than assumed. Anything
	// with more structure (a heading, nesting, prose between bullets) captures
	// as one note exactly as before: `splitFlatList` refuses it. Only this
	// zero-focus path asks; a paste into the composer is text editing and the
	// guard above has already declined it.
	const items = splitFlatList(text)
	if (items) {
		offerListPaste(text, items)
		return
	}

	void capturePaste(text)
}

useEventListener(document, 'paste', onPaste)

/**
 * The list-paste question, and the offer it holds. The popover is anchored to
 * the composer — a paste has no DOM anchor of its own, and the composer is
 * where the panel already says captures happen — and its two offers run the
 * two capture actions; nothing is added until one of them is pressed. Escape
 * and an outside click dismiss through reka's layer, and dismissal adds
 * nothing: the clipboard still holds the text, so declining costs one `Ctrl+V`
 * to change your mind, while a dismissal that silently pasted anyway would
 * make the question pointless.
 *
 * Not a modal dialog, for `DoneFilter`'s reason: both offers are single
 * undoable adds, so this exists to pick a shape, not to gate anything.
 */
const pendingListPaste = ref<{ text: string; items: string[] } | null>(null)

/**
 * Where focus sat when the question opened, so closing can put it back. The
 * popover autofocuses an offer — the question is asked mid-keyboard-flow and
 * has to be answerable without a pointer — which means reka's close would
 * otherwise drop focus to the body, where no chord and no arrow key works.
 * `null` when focus was nowhere in particular; the panel root is the landing
 * that keeps the keyboard alive, the same one `onMounted` chooses.
 */
let listPasteOpener: HTMLElement | null = null

function offerListPaste(text: string, items: string[]) {
	const active = document.activeElement
	listPasteOpener = active instanceof HTMLElement && active !== document.body ? active : null
	pendingListPaste.value = { text, items }
}

/** Escape and an outside click arrive here through reka's layer; the two offer
 *  buttons close by writing the state first, so reka emits nothing for them. */
function onListPasteOpen(open: boolean) {
	if (!open) pendingListPaste.value = null
}

function pasteAsOneNote() {
	const pending = pendingListPaste.value
	pendingListPaste.value = null
	if (pending) void capturePaste(pending.text)
}

function pasteAsSeparateNotes() {
	const pending = pendingListPaste.value
	pendingListPaste.value = null
	if (pending) void captureListPaste(pending.items)
}

const oneNoteButton = useTemplateRef<HTMLButtonElement>('oneNoteButton')

/** Reka's default is the first tabbable, which the DOM order below already
 *  makes the one-note offer — stated explicitly so Enter meaning "what a paste
 *  always did" does not depend on markup order staying put. */
function onListPasteAutoFocus(event: Event) {
	event.preventDefault()
	oneNoteButton.value?.focus()
}

/** Every close path lands here: back to the element that held focus when the
 *  question opened, or to the panel root when that element is gone or was the
 *  body — focus left on a vanished node is a dead keyboard. */
function onListPasteCloseFocus(event: Event) {
	event.preventDefault()
	const target = listPasteOpener?.isConnected ? listPasteOpener : root.value
	listPasteOpener = null
	target?.focus()
}

/** `DoneFilter`'s held-key guard, verbatim and for its reason: the browser
 *  synthesises a click from every repeat of an Enter keydown, and this popover
 *  autofocuses a control on open. */
function onListPasteKeydown(event: KeyboardEvent) {
	if (event.repeat && (event.key === 'Enter' || event.key === ' ')) {
		event.preventDefault()
		return
	}
	moveFocusOnArrow(event)
}

/**
 * The default WebView context menu is suppressed everywhere except the two text
 * fields, where Copy/Paste is genuinely useful. Task-004's `.note-prose`
 * exemption is narrowed away here: note bodies are the bulk of a card, so under
 * that policy right-clicking a note would open the WebView's menu instead of
 * Copper's. Copying text out of a body survives through the `Ctrl+C`
 * text-selection guard above.
 *
 * A row that owns a menu is skipped rather than prevented, and the ordering is
 * the reason: reka's trigger defers to a `nextTick` and then checks
 * `defaultPrevented`, so a `preventDefault` from this ancestor handler — which
 * runs first, as the event bubbles outward — would tell it somebody else had
 * already handled the press, and no menu would ever open.
 */
function onContextMenu(event: MouseEvent) {
	const target = event.target as HTMLElement | null
	if (target?.closest('textarea, input')) return
	if (target?.closest('[data-note-row], [data-section-row]')) return
	event.preventDefault()
}
</script>

<template>
	<div
		ref="root"
		data-panel-root
		tabindex="-1"
		class="relative grid h-full min-h-0 w-full grid-cols-1 grid-rows-[auto_1fr_auto] outline-none select-none font-sans text-body"
		@keydown="onShellKeydown"
		@contextmenu="onContextMenu"
	>
		<!-- The section switcher's close-focus event, relayed from the heading in the
		     header to the composer. The two are siblings, and only the composer knows
		     whether it was mid-sentence when the chip's switcher opened. The relay
		     covers the chip and the `...` submenu only — task-019 moved the keyboard
		     entry point to the palette, which returns focus itself. -->
		<!-- **Every one of the shell's three flow children names its own row *and*
		     its own column, and both halves are load-bearing.** The status band below
		     is placed at `col-start-1 row-start-2` deliberately, to share the middle
		     cell with the scroll region. An item definite in *both* axes is placed
		     before auto-placement runs at all, so that cell is already occupied for
		     everything that follows — and auto-placement will not put anything on top
		     of an occupied cell.

		     Naming only the row was half a fix, and the half that was missing broke
		     the other axis. An item locked to a row but auto in its column is placed
		     at the earliest column that does not *overlap*, and the grid grows an
		     implicit column rather than accept one: the scroll region was pushed out
		     of column 1 into a second track on the right, 111px of a 441px panel,
		     while the header and the composer — whose rows nothing had claimed —
		     stayed in the 330px one. Note bodies wrapped at a character or two per
		     line. Naming neither axis was the failure before that: the three were
		     pushed a row past where they belong, the list into a content-sized row 3
		     and the composer into an implicit row 4, which is what put the toast pill
		     at the *top* of the notes region — correctly at the bottom of a row 2 that
		     was the empty strip above the list.

		     Both axes on all four is what ends it: every flow child is then placed in
		     the first step, where an explicit overlap is precisely what is being asked
		     for. `grid-cols-1` on the root says out loud that the shell is one column;
		     on its own it fixes nothing, because the no-overlap rule creates implicit
		     tracks whatever the explicit grid says. -->
		<PanelHeader
			ref="header"
			class="col-start-1 row-start-1"
			@switcher-closed="composer?.restoreCaret($event)"
		/>

		<!-- The only scrollable region. `min-h-0` is load-bearing: a grid item
		     defaults to `min-height: auto`, so without it this grows to its content
		     height and the whole document scrolls despite `overflow: hidden` on
		     html/body/#app. `min-w-0` is the horizontal equivalent and a separate
		     failure mode — a wide code fence or Markdown table widens the document
		     without it. -->
		<!-- **`scrollbar-gutter: auto` here, against `.thin-scrollbar`'s `stable`.**
		     The utility reserves the gutter permanently so the text column cannot
		     change width as the list crosses the overflow threshold — which is right
		     for the menus that also use it, and wrong for the one surface whose
		     edges the reader compares. A short list reserved 11px on the right and
		     nothing on the left, so the notes sat visibly off-centre in the panel
		     with no scrollbar on screen to explain it.

		     The re-wrap that used to be the price of `auto` is paid differently
		     now: `measureScrollbar` publishes the scrollbar's real width as
		     `--scrollbar-space`, and the right-edge paddings absorb it (the
		     variable split lives in main.css). Crossing the threshold narrows the
		     card boxes by the scrollbar's width — it must, a card cannot underlap
		     a classic scrollbar — but the text column and every trailing control
		     hold their position, so nothing shifts and nothing re-wraps. -->
		<!-- `overflow-x-hidden` states the design rule outright: this region never
		     scrolls horizontally. Left implicit (`overflow-y-auto` computes x to
		     `auto`), any invisible box past the edge summons a horizontal bar —
		     the chevron's centred `hit-44` pokes ~2px out once the row padding
		     yields to the scrollbar, and that conjured one (user report,
		     2026-08-11). Hit areas may overhang; bars may not. -->
		<main
			ref="scrollRegion"
			data-scroll-region
			class="thin-scrollbar col-start-1 row-start-2 min-h-0 min-w-0 overflow-x-hidden overflow-y-auto overscroll-contain [scrollbar-gutter:auto]"
			:aria-busy="refreshing"
			@click.capture="onRegionClick"
		>
			<h1 class="sr-only">{{ spaceName || 'Copper' }}</h1>

			<PanelStates>
				<!-- `px-1` matches the 4px rhythm between rows. Without it the cards sit
				     flush against the panel edge, which puts their rounded corners and
				     the selection ring's outer edge hard against the window.

				     It insets the *cards*, and the section band deliberately escapes it:
				     that row cancels this with `-mx-1` and gives the 4px back through its
				     own padding, so the pinned fill reaches both edges while the heading
				     stays on the same column as everything else. A row that needs its
				     background to be the region's own edge has to reach past this.

				     **No top padding, so the first section heading starts flush against
				     the top of the region.** The heading pins itself there the moment
				     anything scrolls under it, and 8px of lead-in meant the band jumped
				     up by that much the first time it stuck — a heading that moves as you
				     begin to scroll past its own section. Flush at rest is the same
				     position it holds pinned, so nothing moves at all. It costs the
				     focus indicator nothing: `focus-inset` draws inside the band's own
				     box, so a first heading flush against the region's top still shows
				     its whole outline. The landing margins in NoteSection are measured
				     from the region's edge and are unaffected. -->
				<!-- The right half is `--region-inset-r` rather than the left's plain
				     4px: it is the first spacing the scrollbar is paid out of, shrinking
				     toward zero as `--scrollbar-space` grows. The fallback keeps the
				     4px wherever main.css's region block is not in effect. -->
				<div class="pb-3 pl-1 pr-[var(--region-inset-r,--spacing(1))]">
					<NoteList />

					<!-- Additive, not a replacement: a zero-note space still renders its
					     section headers and the active section's own empty line, because
					     hiding where a capture will land is worst exactly when the list
					     is empty.

					     A sibling of the grid rather than a child of it, which is the same
					     rule the drop indicator and the two narrowing states above follow:
					     a `role="grid"` may own only rows and rowgroups, so anything that
					     is not a row belongs outside it.

					     **`!hasQuery` is the half that was missing.** `empty` is a property
					     of the *document* — zero notes across every section — and says
					     nothing about the query, so a search typed into an empty space used
					     to render this and `SearchEmptyState` at once: two answers to one
					     emptiness, one of which blamed a query that is not the reason. The
					     query is the narrower explanation and clearing it is the shorter way
					     back, exactly as `NoteList` decides between its own two states, so
					     it wins and this stands down. -->
					<EmptyState v-if="empty && !hasQuery" />

					<EditorRecoveryRow />
				</div>
			</PanelStates>
		</main>

		<!-- The list-paste question, anchored onto the composer itself: the root
		     renders nothing and the anchor merges onto the form, so the shell's
		     grid still sees the same third-row child it always had. The content
		     goes through the same in-clip portal host as every menu. -->
		<Popover :open="pendingListPaste !== null" @update:open="onListPasteOpen">
			<PopoverAnchor as-child>
				<Composer ref="composer" class="col-start-1 row-start-3" />
			</PopoverAnchor>

			<PopoverContent
				v-if="portalTo"
				:to="portalTo"
				data-list-paste
				side="top"
				align="start"
				:collision-boundary="boundary ?? undefined"
				:collision-padding="8"
				class="w-64 text-meta"
				@open-auto-focus="onListPasteAutoFocus"
				@close-auto-focus="onListPasteCloseFocus"
				@keydown="onListPasteKeydown"
			>
				<p class="text-text-primary">Paste the list as…</p>
				<!-- The split offer carries the count, because the number is what makes
				     it a different offer from the one above it — and it is the one thing
				     the clipboard cannot show from under a popover. -->
				<div class="mt-2 flex flex-col gap-1">
					<button
						ref="oneNoteButton"
						type="button"
						data-paste-one-note
						class="panel-button flex items-center justify-between gap-2 py-1"
						@click="pasteAsOneNote"
					>
						<span class="min-w-0 truncate">One note</span>
					</button>
					<button
						type="button"
						data-paste-separate-notes
						class="panel-button flex items-center justify-between gap-2 py-1"
						@click="pasteAsSeparateNotes"
					>
						<span class="min-w-0 truncate">Separate notes</span>
						<span class="shrink-0 tabular-nums">{{ pendingListPaste?.items.length }}</span>
					</button>
				</div>
			</PopoverContent>
		</Popover>

		<!-- Inside the panel root, so teleported menu content stays inside the clip,
		     the rounded rect and the contextmenu policy above. `pointer-events-none`
		     is safe for the content itself: reka's dismissable layer sets
		     `pointer-events: auto` inline on the open menu. -->
		<div ref="portalHost" class="pointer-events-none absolute inset-0 z-30 empty:hidden" />

		<!-- Panel-wide, so a drop anywhere attaches — including over the list, which
		     is where a pointer carrying a file naturally ends up. -->
		<DropTarget />

		<!-- Measured, never shown. -->
		<div
			ref="clampProbe"
			aria-hidden="true"
			class="pointer-events-none absolute h-(--note-clamp) w-0"
		/>

		<!-- Both bands share one cell in the shell's middle row and stack inside it,
		     so neither can displace the pinned composer of a window that cannot
		     grow, and the two can never overlap each other. Sharing that cell is what
		     `col-start-1 row-start-2` on the scroll region above now guarantees — see
		     the note on the header for the row, and then the column, this used to
		     take from it.

		     **`self-end` with `pb-2`: the foot of the list, floating, not stuck to
		     it.** The pill reports what just happened to the notes, so it belongs at
		     the end of them rather than over the first rows the eye is reading. The
		     8px is the list's own rhythm rather than a new number — it is the gap
		     between a section heading and the first note under it — and it is what
		     keeps the pill from reading as a strip welded to the composer's top edge.

		     `z-20` is measured, not picked: above the note rows, and above the pinned
		     section heading's `z-1`, which is the one thing in the region that can
		     rise to meet it. Both bands are `pointer-events-none` — the pills
		     overlay the tail of the list, and the rows underneath must stay
		     clickable through the bare parts — so nothing here costs the list a hit
		     target, and the layout is untouched either way: these are overlays in a
		     cell that is already the region's. The toasts themselves re-enable
		     pointer events, which is what hover-to-hold and `Undo` need. -->
		<div
			class="pointer-events-none col-start-1 row-start-2 z-20 flex flex-col gap-1 self-end px-3 pb-2"
		>
			<CaptureNotice />
		</div>

		<!-- Spans the whole list cell rather than hugging its foot, because the
		     toast stack is `absolute` against it and grows upward from the bottom
		     edge — see StatusToaster for why the stack cannot ride the flex band
		     above. -->
		<div class="status-toaster-host pointer-events-none relative col-start-1 row-start-2 z-20">
			<StatusToaster />
		</div>

		<!-- After the band it has to paint over, and at a z-index between the band's
		     `z-20` and the portal host's `z-30`: above the list, below any menu. -->
		<ImageViewer />

		<!-- Last of the overlays and above the portal host, because it is the one
		     that is modal to the whole panel. Nothing can be open underneath it: the
		     chord that opens it is declined from inside any other overlay, so the
		     ordering settles a case that cannot arise rather than one that does. -->
		<Palette />

		<!-- Pre-rendered and empty. Injecting the element and its text together
		     does not announce; only a text change inside a live region already in
		     the accessibility tree does. -->
		<div class="sr-only" role="alert" aria-live="assertive">{{ actionError?.message ?? '' }}</div>
		<div class="sr-only" role="status" aria-live="polite">
			{{ refreshing ? 'Refreshing notes' : '' }}
		</div>
	</div>
</template>
