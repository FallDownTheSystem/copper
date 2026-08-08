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
import type { DoubleClickAction, InsertionPoint } from '@/composables/useSettings'

const {
	shortcuts,
	autostartEnabled,
	theme,
	soundsEnabled,
	motionPreference,
	insertionPoint,
	doubleClickAction,
	alwaysOnTop,
	showCreated,
	captureNotifications,
	linkPreviews,
	errorFor,
	refresh,
	setTheme,
	setAutostart,
	setSounds,
	setMotion,
	setInsertionPoint,
	setDoubleClick,
	setShowCreated,
	setCaptureNotifications,
	setLinkPreviews,
	setAlwaysOnTop,
} = useSettings()
const { isRecording, cancel } = useShortcutRecorder()
const { showList } = useView()
const {
	status: updateStatus,
	currentVersion,
	available: availableUpdate,
	percentage,
	canInstall,
	canRecheck,
	busy: updateBusy,
	error: updateError,
	initialize: startUpdater,
	dispose: stopUpdater,
	checkForUpdate,
	installUpdate,
} = useUpdater()

const themeError = errorFor('theme')
const autostartError = errorFor('autostart')
const soundsError = errorFor('sounds')
const motionError = errorFor('motion')
const insertionError = errorFor('insertionPoint')
const doubleClickError = errorFor('doubleClick')
const alwaysOnTopError = errorFor('alwaysOnTop')
const showCreatedError = errorFor('showCreated')
const captureNotificationsError = errorFor('captureNotifications')
const linkPreviewsError = errorFor('linkPreviews')
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
	// Registers the progress listener and reads the installed version. It does
	// **not** check for updates — nothing in the app does that unasked.
	void startUpdater()

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
	// The status itself is module-scoped and survives, so a download started here
	// and still running when the view closes is reported correctly on the way back
	// in. Only the listener comes down, which is what stops repeated visits
	// stacking up duplicates.
	stopUpdater()
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

/** Both task-013 preferences are two-value *choices* rather than booleans —
 *  neither `top`/`bottom` nor `copy`/`edit` has an "off" reading — so they are
 *  segmented controls and not switches. */
const INSERTION_OPTIONS = [
	{ value: 'bottom', label: 'Bottom' },
	{ value: 'top', label: 'Top' },
] as const satisfies readonly { value: InsertionPoint; label: string }[]

const DOUBLE_CLICK_OPTIONS = [
	{ value: 'copy', label: 'Copy' },
	{ value: 'edit', label: 'Edit' },
] as const satisfies readonly { value: DoubleClickAction; label: string }[]

/** The summon binding's error is either a live failure from a rebind the user
 *  just attempted or the startup registration failure, which has no action behind
 *  it at all. Both belong on the same row. */
const summonRowError = computed(() => summonError.value ?? shortcuts.value?.summonError ?? null)
const captureRowError = computed(() => captureError.value ?? shortcuts.value?.captureError ?? null)

/**
 * The one line the Updates row says about itself.
 *
 * `null` for `idle` — nothing has been asked yet, so there is nothing to report —
 * and `null` for `error`, whose message is rendered below as its own
 * `role="alert"` paragraph rather than as status.
 */
const updateStatusLine = computed(() => {
	switch (updateStatus.value) {
		case 'checking':
			return 'Checking…'
		case 'upToDate':
			return 'Up to date.'
		case 'available': {
			const update = availableUpdate.value
			if (!update) return null
			return update.date
				? `Version ${update.version} is available, released ${update.date}.`
				: `Version ${update.version} is available.`
		}
		case 'downloading':
			// The indeterminate wording is not a fallback nobody hits: the total comes
			// from `Content-Length`, which a server is free to omit.
			return percentage.value === null ? 'Downloading…' : `Downloading… ${percentage.value}%`
		default:
			return null
	}
})

/** Check until there is something to install, then install. One button, because
 *  a second one would be disabled for the whole of its life until it wasn't. */
const updateActionLabel = computed(() => {
	const update = availableUpdate.value
	return update ? `Install ${update.version}` : 'Check for updates'
})

/** One appearance for both update buttons. They are the same control in two
 *  positions, so a copy each is what lets the disabled halves drift apart — and
 *  one of them dimming while the other went grey would read as a bug rather than
 *  as a busy state. */
const updateButtonClass =
	'panel-button hit-44 relative disabled:cursor-default disabled:opacity-60 disabled:hover:bg-transparent'

function onUpdateAction() {
	void (canInstall.value ? installUpdate() : checkForUpdate())
}

function onRecheck() {
	void checkForUpdate()
}

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
				class="icon-button hit-44 relative"
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

			<!-- Beside Theme rather than beside Startup: both are about the window
			     itself, and the pin is the one setting here the user can also reach
			     without opening this view. -->
			<SettingsSection title="Panel">
				<!-- The description says what turning it *off* costs, because that is the
				     half nobody expects: the summon chord and the tray still work, and
				     the panel simply stops floating over whatever is in front. -->
				<SettingsRow
					label="Keep on top"
					description="Float the panel above other windows. Off, the summon shortcut and the tray still bring it back."
					label-for="always-on-top"
					:error="alwaysOnTopError"
				>
					<SettingsSwitch
						id="always-on-top"
						:model-value="alwaysOnTop"
						@update:model-value="setAlwaysOnTop"
					/>
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

			<!-- Below Shortcuts because both are about capturing: the row above says
			     which keys save a selection, and the first row here says where it
			     lands. -->
			<SettingsSection title="Notes">
				<SettingsRow
					label="New notes go"
					description="Where a capture, a paste or a composed note lands in its section."
					:error="insertionError"
				>
					<SettingsChoice
						:model-value="insertionPoint"
						:options="INSERTION_OPTIONS"
						label="Where new notes go"
						@update:model-value="setInsertionPoint"
					/>
				</SettingsRow>

				<SettingsRow
					label="Double-click a note"
					description="Copy runs the same Copy the context menu does; Edit opens the inline editor."
					:error="doubleClickError"
				>
					<SettingsChoice
						:model-value="doubleClickAction"
						:options="DOUBLE_CLICK_OPTIONS"
						label="What double-clicking a note does"
						@update:model-value="setDoubleClick"
					/>
				</SettingsRow>

				<!-- The description says the date is already there rather than promising
				     it from now on: `created` has been recorded on every note since the
				     store's first version, so turning this on reveals the whole history
				     at once instead of starting a new one. Without that sentence the
				     switch looks like it begins collecting something. -->
				<SettingsRow
					label="Date added"
					description="Show when each note was created. Every note already carries its date; this only shows it."
					label-for="show-created"
					:error="showCreatedError"
				>
					<SettingsSwitch
						id="show-created"
						:model-value="showCreated"
						@update:model-value="setShowCreated"
					/>
				</SettingsRow>
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

			<!-- Its own section rather than a third row under "Sound and motion":
			     that section is about how the panel behaves while you are looking at
			     it, and this is the only setting in the view that describes what
			     Copper does while you are not. The description names the condition
			     rather than leaving it to be discovered — with the panel on screen
			     this switch reads "on" and nothing appears, which looks broken unless
			     the row says why. -->
			<SettingsSection title="Notifications">
				<SettingsRow
					label="Capture notifications"
					description="When the panel is hidden, show a Windows notification with the capture and buttons to file it in another section."
					label-for="capture-notifications"
					:error="captureNotificationsError"
				>
					<SettingsSwitch
						id="capture-notifications"
						:model-value="captureNotifications"
						@update:model-value="setCaptureNotifications"
					/>
				</SettingsRow>
			</SettingsSection>

			<!-- Its own section, and the only one in this view that is not about how
			     Copper behaves. Every other row here changes something local; this is
			     the one switch that decides whether the app talks to anyone at all,
			     and burying it under "Notes" would make it look like a display
			     preference.

			     The description says what turning it *on* sends, in the words a
			     person would use, rather than the spec's "Show cached page details
			     below links" — which describes the visible half and leaves the half
			     that matters to be discovered. The three things named are exactly what
			     a fetch discloses: the address, the IP, and the moment of reading. -->
			<SettingsSection title="Privacy">
				<SettingsRow
					label="Link previews"
					description="Show a page's title, description and picture below links in a note. Copper has to fetch each linked page to do it, which tells whoever runs that site the address, your IP address, and when you read the note. Off, no page is ever fetched."
					label-for="link-previews"
					:error="linkPreviewsError"
				>
					<SettingsSwitch
						id="link-previews"
						:model-value="linkPreviews"
						@update:model-value="setLinkPreviews"
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

			<SettingsSection title="Updates">
				<!-- The error is rendered below rather than through `SettingsRow`'s own
				     `error` prop, which would place it after this slot. The recovery
				     button has to come *after* the message that explains why it is
				     there. -->
				<SettingsRow
					label="Version"
					:description="currentVersion ? `Copper ${currentVersion}` : 'Reading the version…'"
				>
					<template #below>
						<!-- Two copies on purpose, and only one of them is in the
						     accessibility tree. A live region has to exist before its content
						     changes to be announced reliably, so the spoken copy is a
						     permanently mounted `sr-only` node and the visible paragraph is
						     hidden from assistive tech to stop it being read a second time.
						     PanelShell's status region is the same arrangement. -->
						<p
							v-if="updateStatusLine"
							aria-hidden="true"
							class="text-text-secondary mt-1.5 text-meta text-pretty"
						>
							{{ updateStatusLine }}
						</p>
						<div class="sr-only" role="status" aria-live="polite">
							{{ updateStatusLine ?? '' }}
						</div>

						<p
							v-if="updateStatus === 'available' && availableUpdate?.notes"
							class="text-text-secondary mt-1 text-meta text-pretty"
						>
							{{ availableUpdate.notes }}
						</p>

						<p
							v-if="updateError"
							class="text-text-primary mt-1.5 flex items-start gap-1.5 text-meta"
							role="alert"
						>
							<IconLucideAlertCircle
								class="mt-0.5 size-3.5 shrink-0"
								aria-hidden="true"
								focusable="false"
							/>
							<span>{{ updateError }}</span>
						</p>

						<!-- The way out of an update that will never install. The retained
						     update is reused on retry precisely so a second manifest request
						     is unnecessary — but if the release was re-cut under us, that
						     same download fails every time and Install alone is a one-way
						     door. Checking again discards it. Placed here rather than beside
						     the Install button because two `hit-44` controls sitting flush
						     would overlap each other's hit areas. -->
						<button
							v-if="canRecheck"
							type="button"
							:disabled="updateBusy"
							class="mt-2"
							:class="updateButtonClass"
							@click="onRecheck"
						>
							Check again
						</button>
					</template>

					<!-- Disabled while a command is in flight, so the UI cannot issue the
					     concurrent call the Rust side would then have to refuse. -->
					<button
						type="button"
						:disabled="updateBusy"
						:class="updateButtonClass"
						@click="onUpdateAction"
					>
						{{ updateActionLabel }}
					</button>
				</SettingsRow>
			</SettingsSection>
		</div>
	</div>
</template>
