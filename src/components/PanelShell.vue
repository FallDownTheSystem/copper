<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'

import { CHORDS, inOverlay, inTextSurface } from '@/lib/chords'

const {
	loadState,
	refreshing,
	actionError,
	noteCount,
	spaceName,
	activeSectionObject,
	initialize,
} = useSpace()
const { setClampHeight } = useNoteDisclosure()
const { ensureHighlighter } = useMarkdown()
// `setOverlayHost` below is what fills the two refs every menu reads; each menu
// reads them from the composable itself rather than being handed them.
const { setOverlayHost } = useOverlayHost()
const { hasQuery, clearQuery, resultCount } = useNoteSearch()
const { open: openPalette } = usePalette()
const { selectedIds, clear } = useSelection()
const { editingNoteId, cancel } = useNoteEditor()
const { isOpen: viewerOpen, close: closeViewer } = useImageViewer()
const { interactionRowId, exit } = useInteractionMode()
const { initialize: initializeHandoffs } = useEditorHandoff()
const { initialize: initializeSpaces } = useSpaces()
const {
	copyNotes,
	copyAsList,
	capturePaste,
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

onMounted(() => {
	setOverlayHost(root.value, portalHost.value)

	measureClamp()

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
	if (event.altKey && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) {
		event.preventDefault()
		void moveFocusedBy(event.key === 'ArrowDown' ? 1 : -1)
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
 * component, so the settings view has none.
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
	void capturePaste(event.clipboardData?.getData('text/plain') ?? '')
}

useEventListener(document, 'paste', onPaste)

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

		     What this trades away is real and much smaller: crossing the threshold
		     now re-wraps the note bodies once, where before it never did. A rare,
		     transient re-wrap against a permanent, always-visible asymmetry. -->
		<main
			data-scroll-region
			class="thin-scrollbar col-start-1 row-start-2 min-h-0 min-w-0 overflow-y-auto overscroll-contain [scrollbar-gutter:auto]"
			:aria-busy="refreshing"
		>
			<h1 class="sr-only">{{ spaceName || 'Copper' }}</h1>

			<PanelStates>
				<!-- `px-1` matches the 4px rhythm between rows. Without it the cards sit
				     flush against the panel edge, which puts their rounded corners and
				     the selection ring's outer edge hard against the window.

				     **No top padding, so the first section heading starts flush against
				     the top of the region.** The heading pins itself there the moment
				     anything scrolls under it, and 8px of lead-in meant the band jumped
				     up by that much the first time it stuck — a heading that moves as you
				     begin to scroll past its own section. Flush at rest is the same
				     position it holds pinned, so nothing moves at all. What it costs is
				     the top 4px of the *first* heading's focus halo, clipped by the
				     region's edge; a pinned heading's is clipped there in every case
				     already, so this makes one row consistent rather than making it
				     worse. The landing margins in NoteSection are measured from the
				     region's edge and are unaffected. -->
				<div class="px-1 pb-3">
					<NoteList />

					<!-- Additive, not a replacement: a zero-note space still renders its
					     section headers and the active section's own empty line, because
					     hiding where a capture will land is worst exactly when the list
					     is empty. -->
					<div v-if="empty" class="px-3 pt-4">
						<p class="text-text-primary text-body font-semibold">No notes yet.</p>
						<p class="text-text-secondary mt-1 text-meta">
							Add one below. It lands in {{ activeSectionObject?.name ?? 'this space' }}.
						</p>
					</div>

					<EditorRecoveryRow />
				</div>
			</PanelStates>
		</main>

		<Composer ref="composer" class="col-start-1 row-start-3" />

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
		     rise to meet it. The whole band is `pointer-events-none` — see StatusLine
		     for why the rows underneath must stay clickable through it — so nothing
		     here costs the list a hit target, and the layout is untouched either way:
		     these are overlays in a cell that is already the region's. -->
		<div
			class="pointer-events-none col-start-1 row-start-2 z-20 flex flex-col gap-1 self-end px-3 pb-2"
		>
			<StatusLine />
			<CaptureNotice />
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
