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
import {
	DEFAULT_PANEL_HEIGHT,
	DEFAULT_PANEL_WIDTH,
	PANEL_HEIGHT_MAX,
	PANEL_HEIGHT_MIN,
	PANEL_WIDTH_MAX,
	PANEL_WIDTH_MIN,
	VIBRANCY_MAX,
	VIBRANCY_MIN,
	VIBRANCY_STEP,
	formatVibrancy,
	type DoubleClickAction,
	type InsertionPoint,
} from '@/composables/useSettings'
import { ACCENT_COLORS, NEUTRAL_TONES, type AccentColor, type NeutralTone } from '@/lib/palette'

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
	translucent,
	neutralTone,
	accentColor,
	vibrancy,
	resizable,
	panelWidth,
	panelHeight,
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
	setTranslucency,
	setNeutralTone,
	setAccentColor,
	previewVibrancy,
	setVibrancy,
	setResizable,
	setPanelSize,
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
const translucentError = errorFor('translucent')
const neutralError = errorFor('neutral')
const accentError = errorFor('accent')
const vibrancyError = errorFor('vibrancy')
const resizableError = errorFor('resizable')
const panelSizeError = errorFor('panelSize')
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
	// The preview belongs to a drag, and there is no drag once this view is gone.
	// Reka emits no `valueCommit` for a drag that ends where it started, so without
	// this a nudge-and-return would leave a preview standing over `settings.json`
	// for the rest of the session — harmless while the two agree, wrong the moment
	// a pull disagrees.
	previewVibrancy(null)
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

/** Both pickers read their maps rather than repeating them: a family added to
 *  `lib/palette` reaches the settings view with no second edit here to forget.
 *  Declaration order is swatch order, which for the accents is the spectrum
 *  Tailwind already lays its families out in. */
const NEUTRAL_OPTIONS = Object.entries(NEUTRAL_TONES).map(([value, family]) => ({
	value: value as NeutralTone,
	label: family.label,
	swatch: family.swatch,
}))

const ACCENT_OPTIONS = Object.entries(ACCENT_COLORS).map(([value, family]) => ({
	value: value as AccentColor,
	label: family.label,
	swatch: family.swatch,
}))

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

/** The same standing condition for the other role, now that summon can be a
 *  double-tap too: without it a summon whose hook died would say nothing at all,
 *  on the one binding whose silence locks the user out of the panel. */
const summonNote = computed(() => {
	const fallback = shortcuts.value?.summonFallback
	if (!fallback) return null
	return `Copper couldn't install its keyboard hook, so the double-tap isn't available. Use ${fallback} to summon Copper until the next restart.`
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
			<!-- "Appearance" rather than "Theme": the theme is only the first row of
			     several now. Light-or-dark, the grey, the accent and how strong that
			     accent is are one decision made in four parts, and in that order — each
			     row is read against whatever the rows above it chose. Translucency
			     closes the section as the one change that is about the surface all four
			     of them are painted on. -->
			<SettingsSection title="Appearance">
				<!-- `v-slot="{ errorId }"` on every row that can fail: the row owns the
				     message and its live region, and hands the id down so the control
				     can point at it. -->
				<SettingsRow
					v-slot="{ errorId }"
					label="Theme"
					description="Match your system, or set it manually."
					:error="themeError"
				>
					<ThemeToggle :model-value="theme" :error-id="errorId" @update:model-value="setTheme" />
				</SettingsRow>

				<!-- The label carries the current choice, because colour is the only thing
				     telling these swatches apart on screen: a user who cannot see the
				     difference would otherwise have nothing at all to read, and one who
				     can still has eighteen unlabelled circles to hover. -->
				<SettingsRow
					v-slot="{ errorId }"
					:label="`Grey tone: ${NEUTRAL_TONES[neutralTone].label}`"
					description="The cast of the panel's greys, from warm through to blue."
					:error="neutralError"
				>
					<SettingsPalette
						:model-value="neutralTone"
						:options="NEUTRAL_OPTIONS"
						label="Grey tone"
						:error-id="errorId"
						@update:model-value="setNeutralTone"
					/>
				</SettingsRow>

				<SettingsRow
					v-slot="{ errorId }"
					:label="`Accent color: ${ACCENT_COLORS[accentColor].label}`"
					description="What selection, focus and the active section are coloured with."
					:error="accentError"
				>
					<SettingsPalette
						:model-value="accentColor"
						:options="ACCENT_OPTIONS"
						label="Accent color"
						:error-id="errorId"
						@update:model-value="setAccentColor"
					/>
				</SettingsRow>

				<!-- Directly under Accent color, because it is a dial *on* the row above
				     rather than a setting beside it — and it is meaningless read on its
				     own.

				     The description names the design decision it lets the user overrule.
				     Every family is scaled against the copper it was calibrated with, so
				     the whole set arrives as restrained as the shipped panel; that reads
				     as right on copper and as washed out on blue, which is exactly the
				     complaint. Saying so is what makes an unlabelled multiplier a control
				     with a reason.

				     In `below` rather than the trailing column: a track squeezed into the
				     strip beside a description would be about sixty pixels long, and a
				     sixty-pixel track cannot be dragged with any precision. -->
				<SettingsRow
					label="Vibrancy"
					description="How strong the accent is. Copper's palette is deliberately muted, which can leave the brighter colours looking washed out — turn this up to give them back their strength."
					:error="vibrancyError"
				>
					<!-- `#below="{ errorId }"`, not `v-slot` on the row: the control lives
					     down here, so the id has to arrive through this slot — and claiming
					     the default slot from the tag alongside a named template is the one
					     slot shape Vue refuses to compile. -->
					<template #below="{ errorId }">
						<SettingsSlider
							:model-value="vibrancy"
							:min="VIBRANCY_MIN"
							:max="VIBRANCY_MAX"
							:step="VIBRANCY_STEP"
							label="Vibrancy"
							:value-text="formatVibrancy(vibrancy)"
							:error-id="errorId"
							@update:model-value="previewVibrancy"
							@commit="setVibrancy"
						/>
					</template>
				</SettingsRow>

				<!-- Moved here from "Panel" (2026-08-08). It is the most visible
				     appearance change in the view, and a user looking for it looks under
				     the heading that says appearance — the old argument for filing it by
				     mechanism ("Windows paints the blur, not the stylesheet") described
				     how it is implemented rather than what it is for.

				     The description says what it costs rather than only what it does. A
				     translucent panel over busy wallpaper is harder to read than an opaque
				     one, and that is the half a user discovers after switching rather than
				     before. -->
				<SettingsRow
					v-slot="{ errorId }"
					label="Translucent background"
					description="Blur the desktop through the panel instead of covering it. Text is easier to read over a solid panel."
					label-for="translucent"
					:error="translucentError"
				>
					<SettingsSwitch
						id="translucent"
						:model-value="translucent"
						:error-id="errorId"
						@update:model-value="setTranslucency"
					/>
				</SettingsRow>
			</SettingsSection>

			<!-- The window's own properties: whether it floats, whether its edges
			     drag, and how big it opens. Translucency left for Appearance — it is
			     the most visible appearance change in the view — but the pin stayed,
			     because "keep this panel where it is" is a statement about the window
			     and this is the heading that says window. -->
			<SettingsSection title="Panel">
				<!-- The description says what turning it *off* costs, because that is the
				     half nobody expects: the summon chord and the tray still work, and
				     the panel simply stops floating over whatever is in front. -->
				<SettingsRow
					v-slot="{ errorId }"
					label="Keep on top"
					description="Float the panel above other windows. Off, the summon shortcut and the tray still bring it back."
					label-for="always-on-top"
					:error="alwaysOnTopError"
				>
					<SettingsSwitch
						id="always-on-top"
						:model-value="alwaysOnTop"
						:error-id="errorId"
						@update:model-value="setAlwaysOnTop"
					/>
				</SettingsRow>

				<!-- Off by default, and the description says what that buys rather than
				     only what it costs: a fixed panel is one you cannot start dragging by
				     clicking a few pixels too close to its edge. -->
				<SettingsRow
					v-slot="{ errorId }"
					label="Resizable"
					description="Let the panel be resized by dragging its edges. Off, a click near the edge can never start a drag by accident."
					label-for="resizable"
					:error="resizableError"
				>
					<SettingsSwitch
						id="resizable"
						:model-value="resizable"
						:error-id="errorId"
						@update:model-value="setResizable"
					/>
				</SettingsRow>

				<!-- The description has to carry the relationship between the two rows,
				     because nothing on screen shows it: a size dragged with the row above
				     switched on is *not* written back here, so it lasts until the next
				     launch and then this number wins. Without that sentence a user who
				     dragged the panel wider, restarted, and found it narrow again would
				     read it as the app forgetting. -->
				<SettingsRow
					label="Size"
					description="The size the panel opens at. Resizing it by dragging lasts until you restart Copper; this is what it comes back to."
					:error="panelSizeError"
				>
					<!-- `#below="{ errorId }"` for the reason the vibrancy row gives. -->
					<template #below="{ errorId }">
						<SettingsSizeRow
							:width="panelWidth"
							:height="panelHeight"
							:min-width="PANEL_WIDTH_MIN"
							:max-width="PANEL_WIDTH_MAX"
							:min-height="PANEL_HEIGHT_MIN"
							:max-height="PANEL_HEIGHT_MAX"
							:default-width="DEFAULT_PANEL_WIDTH"
							:default-height="DEFAULT_PANEL_HEIGHT"
							:error-id="errorId"
							@commit="setPanelSize"
						/>
					</template>
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
						:note="summonNote"
					/>
				</template>
			</SettingsSection>

			<!-- Below Shortcuts because both are about capturing: the row above says
			     which keys save a selection, and the first row here says where it
			     lands. -->
			<SettingsSection title="Notes">
				<SettingsRow
					v-slot="{ errorId }"
					label="New notes go"
					description="Where a capture, a paste or a composed note lands in its section."
					:error="insertionError"
				>
					<SettingsChoice
						:model-value="insertionPoint"
						:options="INSERTION_OPTIONS"
						label="Where new notes go"
						:error-id="errorId"
						@update:model-value="setInsertionPoint"
					/>
				</SettingsRow>

				<SettingsRow
					v-slot="{ errorId }"
					label="Double-click a note"
					description="Copy runs the same Copy the context menu does; Edit opens the inline editor."
					:error="doubleClickError"
				>
					<SettingsChoice
						:model-value="doubleClickAction"
						:options="DOUBLE_CLICK_OPTIONS"
						label="What double-clicking a note does"
						:error-id="errorId"
						@update:model-value="setDoubleClick"
					/>
				</SettingsRow>

				<!-- The description says the date is already there rather than promising
				     it from now on: `created` has been recorded on every note since the
				     store's first version, so turning this on reveals the whole history
				     at once instead of starting a new one. Without that sentence the
				     switch looks like it begins collecting something. -->
				<SettingsRow
					v-slot="{ errorId }"
					label="Date added"
					description="Show when each note was created. Every note already carries its date; this only shows it."
					label-for="show-created"
					:error="showCreatedError"
				>
					<SettingsSwitch
						id="show-created"
						:model-value="showCreated"
						:error-id="errorId"
						@update:model-value="setShowCreated"
					/>
				</SettingsRow>

				<!-- Here rather than in a section of its own: what the notification
				     carries is a captured note, so this is a fact about notes even though
				     it fires while the panel is hidden. The description names the
				     condition — with the panel on screen this switch reads "on" and
				     nothing appears, which looks broken unless the row says why. -->
				<SettingsRow
					v-slot="{ errorId }"
					label="Capture notifications"
					description="When the panel is hidden, show a Windows notification with the capture and buttons to file it in another section."
					label-for="capture-notifications"
					:error="captureNotificationsError"
				>
					<SettingsSwitch
						id="capture-notifications"
						:model-value="captureNotifications"
						:error-id="errorId"
						@update:model-value="setCaptureNotifications"
					/>
				</SettingsRow>

				<!-- The one row in the view that decides whether Copper talks to anyone
				     at all, so the *description* carries the privacy weight the old
				     section heading used to: it says what turning it on sends, in the
				     words a person would use — the address, the IP, and the moment of
				     reading are exactly what a fetch discloses. Filed under Notes because
				     the preview is a thing a note shows, and a reader deciding whether to
				     enable it reads the sentence either way. -->
				<SettingsRow
					v-slot="{ errorId }"
					label="Link previews"
					description="Show a page's title, description and picture below links in a note. Copper has to fetch each linked page to do it, which tells whoever runs that site the address, your IP address, and when you read the note. Off, no page is ever fetched."
					label-for="link-previews"
					:error="linkPreviewsError"
				>
					<SettingsSwitch
						id="link-previews"
						:model-value="linkPreviews"
						:error-id="errorId"
						@update:model-value="setLinkPreviews"
					/>
				</SettingsRow>
			</SettingsSection>

			<!-- "Behavior" rather than "Sound and motion" (2026-08-08): a title that
			     names the rows has to be rewritten every time one arrives, and what
			     these have in common is how the panel conducts itself while you are
			     using it, as against how it looks or where it sits. -->
			<SettingsSection title="Behavior">
				<SettingsRow
					v-slot="{ errorId }"
					label="Sound"
					description="A short sound when you complete a note, add one, or a capture fails."
					label-for="sounds"
					:error="soundsError"
				>
					<SettingsSwitch
						id="sounds"
						:model-value="soundsEnabled"
						:error-id="errorId"
						@update:model-value="setSounds"
					/>
				</SettingsRow>

				<!-- The description says Windows wins rather than leaving the user to
				     discover it: with reduced motion set system-wide this switch reads
				     "on" and nothing animates, which looks like a broken control unless
				     the row says why. Turning it off is the only assertion available —
				     there is deliberately no value that animates against the OS. -->
				<SettingsRow
					v-slot="{ errorId }"
					label="Animate controls"
					description="Windows' own animation setting always wins; this can only turn animation off."
					label-for="motion"
					:error="motionError"
				>
					<SettingsSwitch
						id="motion"
						:model-value="motionPreference === 'auto'"
						:error-id="errorId"
						@update:model-value="setAnimations"
					/>
				</SettingsRow>
			</SettingsSection>

			<SettingsSection title="Startup">
				<SettingsRow
					v-slot="{ errorId }"
					label="Launch Copper at login"
					description="Start automatically when you sign in to Windows."
					label-for="autostart"
					:error="autostartError"
				>
					<SettingsSwitch
						id="autostart"
						:model-value="autostartEnabled"
						:error-id="errorId"
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

						<!-- The failure is split the same way, and for the sharper version of
						     the same reason: an alert injected together with its text is the
						     announcement most likely to be dropped, and it is the one message
						     here that has to arrive. -->
						<p
							v-if="updateError"
							aria-hidden="true"
							class="text-text-primary mt-1.5 flex items-start gap-1.5 text-meta"
						>
							<IconLucideAlertCircle
								class="mt-0.5 size-3.5 shrink-0"
								aria-hidden="true"
								focusable="false"
							/>
							<span>{{ updateError }}</span>
						</p>
						<span class="sr-only" role="alert">{{ updateError ?? '' }}</span>

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
