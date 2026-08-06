<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'

import { CHORDS, inComposer, inOverlay, inTextSurface } from '@/lib/chords'

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
const { openSwitcher } = useSections()
const { selectedIds, clear } = useSelection()
const { editingNoteId, cancel } = useNoteEditor()
const { interactionRowId, exit } = useInteractionMode()
const { initialize: initializeHandoffs } = useEditorHandoff()
const { initialize: initializeSpaces } = useSpaces()
const {
	copyNotes,
	copyAsList,
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
const composer = useTemplateRef<{ focus: () => void }>('composer')

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
 */
function onEscape(event: KeyboardEvent) {
	if (editingNoteId.value) {
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

	// **Above the suppression guard, and the only chord that is.** Task-006's rule
	// is that no in-panel chord fires from a text surface; this is the documented
	// exception, because switching where the next capture lands is a thing you do
	// while typing the note before it. Still suppressed in the other two surfaces,
	// and it cannot reach task-008's shortcut recorder at all — that lives in the
	// settings view, which unmounts this tree, and it `preventDefault`s and
	// consumes every key but Tab besides.
	if (
		CHORDS.switchSection.matches(event) &&
		(!inTextSurface(event.target) || inComposer(event.target))
	) {
		event.preventDefault()
		openSwitcher()
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
		tabindex="-1"
		class="relative grid h-full min-h-0 w-full grid-rows-[auto_1fr_auto] outline-none select-none font-sans text-body"
		@keydown="onShellKeydown"
		@contextmenu="onContextMenu"
	>
		<PanelHeader ref="header" />

		<!-- The only scrollable region. `min-h-0` is load-bearing: a grid item
		     defaults to `min-height: auto`, so without it this grows to its content
		     height and the whole document scrolls despite `overflow: hidden` on
		     html/body/#app. `min-w-0` is the horizontal equivalent and a separate
		     failure mode — a wide code fence or Markdown table widens the document
		     without it. -->
		<main
			data-scroll-region
			class="thin-scrollbar min-h-0 min-w-0 overflow-y-auto overscroll-contain"
			:aria-busy="refreshing"
		>
			<h1 class="sr-only">{{ spaceName || 'Copper' }}</h1>

			<PanelStates>
				<div class="pt-2 pb-3">
					<NoteList />

					<!-- Additive, not a replacement: a zero-note space still renders its
					     section headers and the active section's own empty line, because
					     hiding where a capture will land is worst exactly when the list
					     is empty. -->
					<div v-if="empty" class="px-3 pt-4">
						<p class="text-text-primary text-body font-semibold">No notes yet</p>
						<p class="text-text-secondary mt-1 text-meta">
							Add one below. It lands in {{ activeSectionObject?.name ?? 'this space' }}.
						</p>
					</div>

					<EditorRecoveryRow />
				</div>
			</PanelStates>
		</main>

		<Composer ref="composer" />

		<!-- Inside the panel root, so teleported menu content stays inside the clip,
		     the rounded rect and the contextmenu policy above. `pointer-events-none`
		     is safe for the content itself: reka's dismissable layer sets
		     `pointer-events: auto` inline on the open menu. -->
		<div ref="portalHost" class="pointer-events-none absolute inset-0 z-30 empty:hidden" />

		<!-- Measured, never shown. -->
		<div
			ref="clampProbe"
			aria-hidden="true"
			class="pointer-events-none absolute h-(--note-clamp) w-0"
		/>

		<!-- Both bands share one cell in the shell's middle row and stack inside it,
		     so neither can displace the pinned composer of a window that cannot
		     grow, and the two can never overlap each other. -->
		<div
			class="pointer-events-none col-start-1 row-start-2 z-20 flex flex-col gap-1 self-end px-3 pb-2"
		>
			<StatusLine />
			<CaptureNotice />
		</div>

		<!-- Pre-rendered and empty. Injecting the element and its text together
		     does not announce; only a text change inside a live region already in
		     the accessibility tree does. -->
		<div class="sr-only" role="alert" aria-live="assertive">{{ actionError?.message ?? '' }}</div>
		<div class="sr-only" role="status" aria-live="polite">
			{{ refreshing ? 'Refreshing notes' : '' }}
		</div>
	</div>
</template>
