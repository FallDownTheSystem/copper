<script setup lang="ts">
/**
 * The settings view: an inline, full-surface replacement for the list inside the
 * one fixed panel. No second window, no router.
 *
 * It **replaces** the list's three-region frame rather than extending it — there
 * is no composer or search equivalent here — so it carries its own fixed header
 * and one scroll region running to the panel's bottom edge.
 *
 * No scrim of its own. This renders inside task-004's panel root, whose
 * translucent tint already carries the alpha calculated to stabilise contrast
 * over an acrylic backdrop; a second layer would make this view visibly darker
 * than the list it transitions from.
 */
const {
	shortcuts,
	autostartEnabled,
	theme,
	soundsEnabled,
	motionPreference,
	errorFor,
	refresh,
	setTheme,
	setAutostart,
	setSounds,
	setMotion,
} = useSettings()
const { isRecording, cancel } = useShortcutRecorder()
const { showList } = useView()

const themeError = errorFor('theme')
const autostartError = errorFor('autostart')
const soundsError = errorFor('sounds')
const motionError = errorFor('motion')
const summonError = errorFor('summon')
const captureError = errorFor('capture')

const back = useTemplateRef<HTMLButtonElement>('back')

/**
 * Pulled on open, not only at startup. Two reasons, and each would be a stale
 * value on its own: a summon chord that failed to register during `setup()` is
 * state with no event behind it, and autostart can be switched off in Task
 * Manager while Copper is running with nothing notifying us.
 */
onMounted(() => {
	void refresh()

	// Focus has to be moved deliberately, or Escape does not work on arrival.
	// Neither entry path leaves it anywhere useful: the `...` menu's trigger is
	// unmounted by `AnimatePresence` as the list leaves, and the tray's
	// `open-settings` lands focus on `document.body` — which is an *ancestor* of
	// this root, so a press there never bubbles down to the handler below. The
	// Back button rather than the root: it is a visible, obvious starting point,
	// and it is where a keyboard user wants to be anyway.
	void nextTick(() => back.value?.focus())
})

/**
 * Leaving cancels any recording. Belt and braces to the Rust lease rather than
 * the mechanism — Rust restores the chords on unmount, on a WebView reload and on
 * a watchdog timeout, because those are the paths this handler never runs on.
 */
onBeforeUnmount(() => {
	void cancel()
})

/**
 * Escape resolves at two levels here, and the second one is the reason this
 * consults `isRecording` rather than relying on the recorder's own
 * `stopPropagation`.
 *
 * Propagation only shadows this handler while focus is *inside* the recorder. One
 * Tab moves focus to Back, to Cancel or to another row while recording is still
 * live, and Escape would then reach this handler and leave the view with the
 * lease still open — the summon chord gone until the watchdog fires. The
 * recorder's state is module-scoped and already shared, so reading it costs no
 * coupling that does not already exist.
 */
function onEscape(event: KeyboardEvent) {
	if (event.key !== 'Escape') return
	event.preventDefault()
	if (isRecording.value) {
		void cancel()
		return
	}
	showList()
}

/** The switch is the presence of animation, but the *setting* is a two-value
 *  preference rather than a boolean, because "auto" means "defer to Windows" and
 *  a boolean has nowhere to say that. Off is the only thing this switch can
 *  assert; on merely stops asserting. */
function setAnimations(on: boolean) {
	void setMotion(on ? 'auto' : 'off')
}

/** The summon binding's error is either a live failure from a rebind the user
 *  just attempted or the startup registration failure, which has no action behind
 *  it at all. Both belong on the same row. */
const summonRowError = computed(() => summonError.value ?? shortcuts.value?.summonError ?? null)
const captureRowError = computed(() => captureError.value ?? shortcuts.value?.captureError ?? null)

/** Standing conditions rather than failed actions: the keyboard hook is down and
 *  a conventional chord is covering for the double-tap. */
const captureNote = computed(() => {
	const fallback = shortcuts.value?.captureFallback
	if (!fallback) return null
	return `Copper couldn't install its keyboard hook, so the double-tap isn't available. Use ${fallback} to capture until the next restart.`
})
</script>

<template>
	<!-- `tabindex="-1"` so the container can hold focus itself if a control it owns
	     ever unmounts under one. It is not in the tab order and shows no ring. -->
	<div
		tabindex="-1"
		class="flex h-full min-h-0 w-full flex-col outline-none select-none font-sans text-body"
		@keydown="onEscape"
	>
		<!-- The drag region is the header's centre column only. The attribute is not
		     inherited by children and the window is `decorations: false`, so without
		     it this view cannot be dragged at all — and putting it on the button
		     would swallow the button's own pointer events. -->
		<header
			class="border-separator grid h-12 shrink-0 grid-cols-[2rem_1fr_2rem] items-center border-b px-2"
		>
			<!-- 32px visually, 44px to the pointer: the expander is a pseudo-element,
			     so the hit area grows without the button growing. -->
			<button
				ref="back"
				type="button"
				aria-label="Back to notes"
				class="squircle text-text-secondary hover:bg-surface-hover active:bg-surface-active outline-focus-ring hit-44 relative grid size-8 place-items-center rounded-md transition-colors duration-fast focus-visible:outline-2 focus-visible:-outline-offset-1"
				@click="showList"
			>
				<IconLucideChevronLeft class="size-4" aria-hidden="true" focusable="false" />
			</button>

			<h1 data-tauri-drag-region class="text-text-primary text-center text-body font-medium">
				Settings
			</h1>

			<!-- Empty, so the title is optically centred rather than centred in the
			     space the back button left over. -->
			<span aria-hidden="true" />
		</header>

		<div class="thin-scrollbar min-h-0 min-w-0 flex-1 space-y-6 overflow-y-auto px-4 py-4">
			<SettingsSection title="Theme">
				<SettingsRow
					label="Theme"
					description="Match your system, or set it manually."
					:error="themeError"
				>
					<ThemeToggle :model-value="theme" @update:model-value="setTheme" />
				</SettingsRow>
			</SettingsSection>

			<SettingsSection title="Shortcuts">
				<template v-if="shortcuts">
					<!-- The description names no keys. The spec's copy was "Double-tap
					     Shift to save your current selection, anywhere.", which stops
					     being true the moment the binding is changed — and the chips
					     directly below it already say which keys, correctly. A row that
					     contradicts its own control is worse than one that says less. -->
					<ShortcutRecorder
						label="Capture"
						description="Save whatever you have selected, from any app."
						target="capture"
						:value="shortcuts.capture"
						:default-value="shortcuts.defaults.capture"
						:error="captureRowError"
						:note="captureNote"
					/>
					<ShortcutRecorder
						label="Summon Copper"
						description="Open the panel from anywhere."
						target="summon"
						:value="shortcuts.summon"
						:default-value="shortcuts.defaults.summon"
						:error="summonRowError"
					/>
				</template>
			</SettingsSection>

			<SettingsSection title="Sound and motion">
				<SettingsRow
					label="Sound"
					description="A short sound when you complete a note, add one, or a capture fails."
					label-for="sounds"
					:error="soundsError"
				>
					<SettingsSwitch
						id="sounds"
						:model-value="soundsEnabled"
						@update:model-value="setSounds"
					/>
				</SettingsRow>

				<!-- The description says Windows wins rather than leaving the user to
				     discover it: with reduced motion set system-wide this switch reads
				     "on" and nothing animates, which looks like a broken control unless
				     the row says why. Turning it off is the only assertion available —
				     there is deliberately no value that animates against the OS. -->
				<SettingsRow
					label="Animate controls"
					description="Windows' own animation setting always wins; this can only turn animation off."
					label-for="motion"
					:error="motionError"
				>
					<SettingsSwitch
						id="motion"
						:model-value="motionPreference === 'auto'"
						@update:model-value="setAnimations"
					/>
				</SettingsRow>
			</SettingsSection>

			<SettingsSection title="Startup">
				<SettingsRow
					label="Launch Copper at login"
					description="Start automatically when you sign in to Windows."
					label-for="autostart"
					:error="autostartError"
				>
					<SettingsSwitch
						id="autostart"
						:model-value="autostartEnabled"
						@update:model-value="setAutostart"
					/>
				</SettingsRow>
			</SettingsSection>
		</div>
	</div>
</template>
