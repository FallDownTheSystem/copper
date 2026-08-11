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
	VIBRANCY_DIAL_MAX,
	VIBRANCY_DIAL_MIN,
	VIBRANCY_DIAL_STEP,
	dialToVibrancy,
	formatVibrancy,
	vibrancyToDial,
	type DoubleClickAction,
	type EnterKeyAction,
	type InsertionPoint,
} from '@/composables/useSettings'
import { openUrl } from '@tauri-apps/plugin-opener'
import { inOverlay, inTextSurface } from '@/lib/chords'
import { ACCENT_COLORS, NEUTRAL_TONES, type AccentColor, type NeutralTone } from '@/lib/palette'
import { SHARE_SETUP_PROMPT } from '@/lib/shareSetupPrompt'

const {
	shortcuts,
	autostartEnabled,
	theme,
	soundsEnabled,
	motionPreference,
	insertionPoint,
	doubleClickAction,
	enterKeyAction,
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
	setEnterKey,
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
const { pasteAttachment } = useAttachments()
const { clearActionError, reportActionError } = useSpace()
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
const enterKeyError = errorFor('enterKey')
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

// --- share (task-026) --------------------------------------------------------

const {
	config: shareConfig,
	actionError: shareError,
	revealedSecret,
	testing: shareTesting,
	generating: shareGenerating,
	lastTest,
	patchConfig,
	generateSecret,
	clearRevealedSecret,
	testRelay,
} = useDeviceShare()

/** The two roles, in the shape `SettingsChoice` takes. */
const ROLE_OPTIONS = [
	{ value: 'first' as const, label: 'First' },
	{ value: 'second' as const, label: 'Second' },
]

/**
 * What **Test connection** last found, as one sentence.
 *
 * Rust supplies the transport wording; the four kinds are turned into copy here
 * because they are UI text rather than error messages, and only this view knows
 * that "the missing field" means a field the reader can see above.
 */
const testMessage = computed(() => {
	const outcome = lastTest.value
	if (!outcome) return null
	switch (outcome.kind) {
		case 'ok':
			return 'The relay answered. This device is set up.'
		case 'unauthorised':
			return 'The relay refused the token. Check that it matches the one you set with wrangler.'
		case 'unconfigured':
			return `Fill in the ${outcome.missing} first.`
		case 'unreachable':
			return outcome.message
	}
})

/** Rust's poll failure and this view's own action failure, in one line. Both are
 *  about the same section and only one can usefully be read at a time. */
const shareRowError = computed(() => shareError.value ?? shareConfig.value.lastError)

/** One appearance for the two buttons in this section, on `updateButtonClass`'s
 *  precedent: a copy each is what lets them drift apart. */
const shareButtonClass = 'panel-button hit-44 relative h-8 px-2 text-meta'

// --- the share setup guide ---------------------------------------------------

/**
 * **An inline disclosure, not an overlay, and the panel is why.**
 *
 * The one dialog precedent in the app is `ImageViewer`, and it lives inside
 * `PanelShell` — the *other* view. This view replaces that whole frame, so it
 * hosts no overlay layer, no portal target and no Escape ladder to join: an
 * overlay here would be a second modal system rather than a reuse of the first,
 * and it would cover the very rows the guide tells the reader to fill in.
 *
 * A disclosure also gets the two things this content needs for free. It scrolls
 * in the view's one scroll region rather than needing a scroll region of its own
 * at 440 × 760, and it sits directly above the Relay URL, Relay token and
 * Pairing secret rows, so the guide and the fields it describes are readable
 * together.
 *
 * **Unanimated, matching the section it is in.** The rows below the enable switch
 * already appear and disappear with no transition, and a height animation over a
 * block this tall is the one motion.md singles out as expensive. A guide that
 * faded in beside rows that snap would read as two different kinds of change.
 */
const guideOpen = ref(false)
const guide = useTemplateRef<HTMLElement>('guide')

/**
 * Focus deliberately **stays on the toggle**. That is the disclosure pattern —
 * the revealed content follows the button in the DOM, so the next Tab and a
 * screen reader's next line both land in the guide already, and moving focus
 * would be dialog behaviour on something that is not a dialog.
 *
 * What does move is the scroll position, because the row can sit near the bottom
 * of the panel: expanding there would otherwise add several hundred pixels below
 * the fold and change nothing the reader can see. `block: 'nearest'` scrolls the
 * least amount that brings the guide into view, and nothing at all when it is
 * already there.
 */
function toggleGuide() {
	guideOpen.value = !guideOpen.value
	if (!guideOpen.value) return
	void nextTick(() => guide.value?.scrollIntoView({ block: 'nearest' }))
}

const { writeText } = useSystemClipboard()

/** What the last **Copy prompt** press did, or null before the first one. */
const promptCopyMessage = ref<string | null>(null)

/**
 * The one clipboard write in the settings view.
 *
 * Reported in place rather than through `useStatusMessage`: the toast stack is
 * rendered by `StatusToaster`, which `PanelShell` mounts and this view does not,
 * so a message sent there would be written to a pill nobody can see.
 *
 * The confirmation carries no timer, unlike the toast's five seconds. It is a
 * statement that stays true — the prompt really is still on the clipboard — and
 * the failure half must not expire at all, since the reader is about to paste
 * nothing into an assistant.
 */
async function copyPrompt() {
	const written = await writeText(SHARE_SETUP_PROMPT)
	promptCopyMessage.value = written
		? 'The prompt is on your clipboard. Paste it into an assistant.'
		: "Couldn't write to the clipboard."
}

/** Opened in the user's browser rather than navigated to: a real anchor would
 *  replace the WebView with a web page and take the panel with it. `NoteBody` and
 *  `LinkPreviewCard` reach the same `openUrl` for the same reason. */
function openGuideLink(url: string) {
	void openUrl(url)
}

const CLOUDFLARE_SIGNUP_URL = 'https://dash.cloudflare.com/sign-up'
const REPOSITORY_URL = 'https://github.com/FallDownTheSystem/copper'

/** Three appearances used across a dozen elements in the guide's markup, on the
 *  same rule as `shareButtonClass`: written once so they cannot drift. */
const guideHeadingClass = 'text-text-primary text-meta font-semibold'
/** `break-words` is load-bearing rather than defensive. The longest span wearing
 *  this is the relay URL's generic form, which is one 48-character unbreakable
 *  word — wider than the guide's text column, so without a break rule it would
 *  push the panel into a horizontal scroll the app must never have. */
const guideCodeClass =
	'bg-code-surface text-text-primary rounded px-1 font-mono text-[0.9em] break-words'
const guideCommandClass =
	'bg-code-surface border-code-border text-text-primary mt-1 block break-words rounded-md border px-2 py-1 font-mono text-meta select-all'
/** The link treatment, since these are `<button>`s and inherit none of an
 *  anchor's. The hover step is `.note-prose a:hover`'s, through the token. */
const guideLinkClass =
	'text-link hover:text-link-hover focus-ring rounded underline underline-offset-2'

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
	// A generated pairing secret is shown once, for as long as this view is open.
	// Leaving it in a module-scoped ref would put it back on screen the next time
	// Settings was opened, which is not what "shown once" means.
	clearRevealedSecret()
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

/**
 * `Ctrl+V` of a file, from the settings view.
 *
 * The list view's zero-focus paste captures text as a note and files as
 * attachments; this view has no composer, so only the file half applies here.
 * Text is deliberately left alone — capturing a note from a view that does not
 * show notes would be an invisible mutation — and Rust already embodies the
 * split: `attach_paste` answers with nothing whenever the clipboard carries
 * text, so the two halves cannot both claim one paste.
 *
 * A handled paste returns to the list, success and refusal alike: the tray it
 * filled and the error line that reports it both live in the composer, so
 * staying here would leave the outcome somewhere the user cannot see it.
 */
function onPaste(event: ClipboardEvent) {
	if (inTextSurface(event.target) || inOverlay(event.target)) return
	// Cleared before the attempt and reported after it — every ingest path's
	// rule; see `Composer`'s `beginAttach`.
	clearActionError('composer')
	void pasteAttachment().then((outcome) => {
		if (!outcome.handled) return
		if (outcome.message) reportActionError('composer', outcome.message)
		showList()
	})
}

useEventListener(document, 'paste', onPaste)

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

const ENTER_KEY_OPTIONS = [
	{ value: 'submit', label: 'Submit' },
	{ value: 'newline', label: 'New line' },
] as const satisfies readonly { value: EnterKeyAction; label: string }[]

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
				title="Back to notes"
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
					:label="`Gray tone: ${NEUTRAL_TONES[neutralTone].label}`"
					description="The cast of the panel's grays, from warm through to blue."
					:error="neutralError"
				>
					<SettingsPalette
						:model-value="neutralTone"
						:options="NEUTRAL_OPTIONS"
						label="Gray tone"
						:error-id="errorId"
						@update:model-value="setNeutralTone"
					/>
				</SettingsRow>

				<SettingsRow
					v-slot="{ errorId }"
					:label="`Accent color: ${ACCENT_COLORS[accentColor].label}`"
					description="What selection, focus and the active section are colored with."
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
					description="How colorful the accent is, from gray to as vivid as your screen can show."
					:error="vibrancyError"
				>
					<!-- `#below="{ errorId }"`, not `v-slot` on the row: the control lives
					     down here, so the id has to arrive through this slot — and claiming
					     the default slot from the tag alongside a named template is the one
					     slot shape Vue refuses to compile. -->
					<template #below="{ errorId }">
						<!-- The slider speaks dial units — 0 to 100 — and the two converters
						     at its edges are the only place the stored multiplier and the
						     dial meet. The store keeps the multiplier so a 0.1.1 file still
						     means what it meant. -->
						<SettingsSlider
							:model-value="vibrancyToDial(vibrancy)"
							:min="VIBRANCY_DIAL_MIN"
							:max="VIBRANCY_DIAL_MAX"
							:step="VIBRANCY_DIAL_STEP"
							label="Vibrancy"
							:value-text="formatVibrancy(vibrancy)"
							:error-id="errorId"
							@update:model-value="(dial) => previewVibrancy(dialToVibrancy(dial))"
							@commit="(dial) => setVibrancy(dialToVibrancy(dial))"
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

				<!-- Outside the `v-if`: the in-panel chords it lists are live whether or
				     not Rust has answered for the two global rows above. -->
				<ShortcutReference />
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

				<SettingsRow
					v-slot="{ errorId }"
					label="Enter key"
					description="What Enter does when writing a note. The other action moves to Ctrl+Enter."
					:error="enterKeyError"
				>
					<SettingsChoice
						:model-value="enterKeyAction"
						:options="ENTER_KEY_OPTIONS"
						label="What the Enter key does when writing"
						:error-id="errorId"
						@update:model-value="setEnterKey"
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
					description="Show a notification for captures made while the panel is hidden, with buttons to file them."
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
				     words a person would use — the IP and the moment of reading are
				     exactly what a fetch discloses. Filed under Notes because
				     the preview is a thing a note shows, and a reader deciding whether to
				     enable it reads the sentence either way. -->
				<SettingsRow
					v-slot="{ errorId }"
					label="Link previews"
					description="Show a page's title and picture below links in a note. Fetching one tells that site your IP address and when you read the note."
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

			<!-- Its own section rather than a row under Notes, unlike link previews:
			     this is six controls and four facts a person cannot infer, and it is
			     the only feature in the app whose setup happens somewhere else. -->
			<SettingsSection title="Share">
				<!-- **Two lines, and the guide below carries the rest.** This description
				     used to run to five: what a relay is, whose account it lives in, that
				     the relay holds only ciphertext, and where to find the commands. All
				     of those are still on screen, one press away, in a place with room to
				     explain them properly. A row description is the sentence a reader
				     scans while deciding whether the switch is for them. -->
				<SettingsRow
					label="Send notes to my other device"
					description="Send a note to your other machine through your own free relay. Notes are encrypted before they leave."
					label-for="share-enabled"
					:error="shareRowError"
				>
					<SettingsSwitch
						id="share-enabled"
						:model-value="shareConfig.enabled"
						@update:model-value="(value) => patchConfig({ enabled: value })"
					/>
				</SettingsRow>

				<!-- **Outside the `v-if` below, and that is the point of it.** Every
				     other row here is hidden until Share is switched on; the guide is
				     the one thing a reader needs *before* they have anything to
				     configure, so hiding it behind the switch would put the
				     instructions on the far side of the step they explain. -->
				<SettingsRow
					label="Setup guide"
					description="Everything you do outside Copper, in order. There is a prompt at the end that hands the job to an AI assistant."
				>
					<!-- The control is in `#below` rather than the trailing column, as the
					     Test connection row's is: the guide it opens has to span the row's
					     full width, and a trailing button would take a column out of it. -->
					<template #below>
						<div class="mt-2 flex items-center gap-2">
							<!-- `aria-expanded` and no `aria-controls`: the guide is rendered
							     only while open, and an `aria-controls` pointing at an id that
							     does not exist yet is worse than none. The label says the state
							     as well, so the button reads correctly with the attribute
							     unsupported. -->
							<button
								type="button"
								:class="shareButtonClass"
								:aria-expanded="guideOpen"
								data-testid="share-guide-toggle"
								@click="toggleGuide"
							>
								{{ guideOpen ? 'Hide setup guide' : 'Show setup guide' }}
							</button>
						</div>

						<!-- `select-text` because the view's root sets `select-none`: the
						     whole point of a command block is that it can be selected. -->
						<div
							v-if="guideOpen"
							ref="guide"
							data-testid="share-setup-guide"
							class="border-separator mt-2 space-y-3 rounded-md border p-3 select-text"
						>
							<!-- **The paragraph the enable row used to carry.** Trimming that row
							     to two lines cost it the sentence about what a relay actually
							     is and what it can see, which is the one thing a reader wants
							     before they hand a feature their notes. It reads better here
							     anyway: a row description is scanned, and this is worth
							     reading. -->
							<p class="text-text-secondary text-meta text-pretty">
								The relay is a small program you deploy to your own free Cloudflare account. Copper
								encrypts every note on this machine before it leaves, so the relay only ever holds
								ciphertext. It deletes each message after seven days, collected or not.
							</p>

							<!-- `h3` under `SettingsSection`'s `h2` and the view's `h1`, so the
							     guide extends the outline rather than skipping a level in it. -->
							<section>
								<h3 :class="guideHeadingClass">Before you start</h3>
								<ul class="text-text-secondary mt-1 list-disc space-y-1 pl-4 text-meta">
									<li>
										Create a free Cloudflare account. No card is needed.
										<button
											type="button"
											:class="guideLinkClass"
											@click="openGuideLink(CLOUDFLARE_SIGNUP_URL)"
										>
											Open the Cloudflare sign-up page
										</button>
									</li>
									<li>
										Download or clone Copper's repository, then open the
										<code :class="guideCodeClass">worker</code> folder inside it. Every command
										below runs from there.
										<button
											type="button"
											:class="guideLinkClass"
											@click="openGuideLink(REPOSITORY_URL)"
										>
											Open the Copper repository
										</button>
									</li>
								</ul>
							</section>

							<section>
								<h3 :class="guideHeadingClass">Deploy the relay</h3>
								<ol class="text-text-secondary mt-1 list-decimal space-y-2 pl-4 text-meta">
									<li>
										Sign in. This opens a browser window.
										<code :class="guideCommandClass">pnpm dlx wrangler@4 login</code>
									</li>
									<li>
										Create the mailbox store. This prints an
										<code :class="guideCodeClass">id</code>.
										<code :class="guideCommandClass"
											>pnpm dlx wrangler@4 kv namespace create MAILBOX</code
										>
									</li>
									<li>
										Open <code :class="guideCodeClass">wrangler.jsonc</code> and paste that
										<code :class="guideCodeClass">id</code> over the placeholder in
										<code :class="guideCodeClass">kv_namespaces</code>.
									</li>
									<li>
										Set the relay token. Wrangler asks for the value. Invent a long random string
										and keep a copy, because you type it into both machines.
										<code :class="guideCommandClass"
											>pnpm dlx wrangler@4 secret put RELAY_TOKEN</code
										>
									</li>
									<li>
										Deploy. This prints your relay URL, of the form
										<code :class="guideCodeClass"
											>https://copper-relay.&lt;your-subdomain&gt;.workers.dev</code
										>.
										<code :class="guideCommandClass">pnpm dlx wrangler@4 deploy</code>
									</li>
								</ol>
								<!-- The one thing that fails silently for a reader who is inside a
								     checkout, which is the likeliest place to be after cloning. -->
								<p class="text-text-secondary mt-2 text-meta text-pretty">
									Inside a clone of the repository, plain
									<code :class="guideCodeClass">npx</code> fails, because Copper's root
									<code :class="guideCodeClass">package.json</code> pins pnpm. Use
									<code :class="guideCodeClass">pnpm dlx</code> there, as written above. If you
									downloaded only the <code :class="guideCodeClass">worker</code> folder,
									<code :class="guideCodeClass">npx wrangler@4</code> works too.
								</p>
							</section>

							<section>
								<h3 :class="guideHeadingClass">Put the values into Copper</h3>
								<p class="text-text-secondary mt-1 text-meta text-pretty">
									Turn Share on above, on <strong class="font-medium">both</strong> machines, and
									fill in the rows that appear.
								</p>
								<ul class="text-text-secondary mt-1 list-disc space-y-1 pl-4 text-meta">
									<li>Relay URL: the address deploy printed.</li>
									<li>
										Relay token: the <code :class="guideCodeClass">RELAY_TOKEN</code> value you set.
									</li>
									<li>
										Pairing secret: press Generate on one machine, then paste the value into the
										other.
									</li>
									<!-- The warning comes before the instruction, because this is the
									     one field a reader can fill in twice without anything ever
									     telling them it is wrong. -->
									<li>
										This device is: set the two machines differently, or nothing is ever delivered
										in either direction and nothing detects it. Use
										<strong class="font-medium">First</strong> on one machine and
										<strong class="font-medium">Second</strong> on the other.
									</li>
								</ul>
								<p class="text-text-secondary mt-1 text-meta text-pretty">
									Then press Test connection on both machines.
								</p>
							</section>

							<!-- Where the arithmetic lives now. The Share rows and the send
							     failure both state one number — about 14 MB of attachments —
							     because that is the only figure a reader can act on. The
							     mechanism behind it is background, and this is the one place
							     with room to be background in. -->
							<section>
								<h3 :class="guideHeadingClass">What a note can carry</h3>
								<p class="text-text-secondary mt-1 text-meta text-pretty">
									About 14 MB of attachments. The relay caps one message at 20 MiB after encryption,
									and attachments are encoded on the way in, which makes them about a third larger.
									Nothing else you write comes close to the rest.
								</p>
							</section>

							<section>
								<h3 :class="guideHeadingClass">Hand it to an AI instead</h3>
								<p class="text-text-secondary mt-1 text-meta text-pretty">
									The prompt below carries every step above. Paste it into Claude or another
									assistant, and it hands your relay URL and relay token back at the end. It never
									generates the pairing secret. You do that here, with Generate.
								</p>

								<div class="mt-2 flex items-center gap-2">
									<button
										type="button"
										:class="shareButtonClass"
										data-testid="share-copy-prompt"
										@click="copyPrompt()"
									>
										Copy prompt
									</button>
								</div>

								<!-- The same split every other outcome in this view uses: a visible
								     paragraph hidden from assistive tech, and a permanently mounted
								     live region beside it. A region injected together with its text
								     is the announcement most likely to be dropped. -->
								<p
									v-if="promptCopyMessage"
									aria-hidden="true"
									class="text-text-secondary mt-1.5 text-meta text-pretty"
								>
									{{ promptCopyMessage }}
								</p>
								<div class="sr-only" role="status" aria-live="polite">
									{{ promptCopyMessage ?? '' }}
								</div>
							</section>
						</div>
					</template>
				</SettingsRow>

				<!-- Everything below is hidden while the feature is off. Six controls
				     for a switch nobody has turned on is the settings view's whole
				     scroll length spent on a feature that is doing nothing. -->
				<template v-if="shareConfig.enabled">
					<!-- The warning stays and the provenance goes. "wrangler printed it
					     when you deployed" is a fact the guide states while the reader is
					     actually deploying; "changing this clears your token" is one they
					     can only meet here, at the moment it happens to them. -->
					<SettingsRow
						label="Relay URL"
						description="The https address deploy printed. Changing it clears the stored relay token, so the old host never receives it."
					>
						<!-- `#below="{ errorId }"` for the reason the vibrancy row gives. -->
						<template #below="{ errorId }">
							<SettingsTextRow
								:value="shareConfig.relayUrl"
								label="Relay URL"
								placeholder="https://copper-relay.your-subdomain.workers.dev"
								:error-id="errorId"
								@commit="(value) => patchConfig({ relayUrl: value })"
							/>
						</template>
					</SettingsRow>

					<SettingsRow
						label="Relay token"
						description="The RELAY_TOKEN value you set. It keeps strangers off your relay; it is not what encrypts your notes."
					>
						<template #below="{ errorId }">
							<SettingsSecretRow
								:set="shareConfig.tokenSet"
								label="Relay token"
								placeholder="Paste the relay token"
								:error-id="errorId"
								@commit="(value) => patchConfig({ token: value })"
							/>
						</template>
					</SettingsRow>

					<SettingsRow
						label="Pairing secret"
						description="What encrypts your notes. Generate it on one machine, paste it into the other, and it never leaves the two."
					>
						<template #below="{ errorId }">
							<SettingsSecretRow
								:set="shareConfig.secretSet"
								label="Pairing secret"
								placeholder="Paste the pairing secret"
								:error-id="errorId"
								@commit="(value) => patchConfig({ secret: value })"
							/>

							<div class="mt-2 flex items-center gap-2">
								<!-- Disabled while one is in flight. Two overlapping generates would
								     each store a secret in Rust and race to display one, so the value
								     on screen could be the one the loser replaced — and it is the
								     value the user is about to carry to the other machine. -->
								<button
									type="button"
									:class="shareButtonClass"
									:disabled="shareGenerating"
									@click="generateSecret()"
								>
									{{ shareGenerating ? 'Generating…' : 'Generate' }}
								</button>
								<span class="text-text-secondary text-meta">
									Replaces the stored secret on this device.
								</span>
							</div>

							<!-- The one place in the app a secret value is ever displayed, and
							     it is displayed once, at the moment it is created. There is no
							     command that reads it back, so the sentence beside it is a
							     statement of fact rather than a warning. -->
							<div v-if="revealedSecret" class="border-border-subtle mt-2 rounded-md border p-2">
								<p class="text-text-secondary text-meta">
									Copy this to your other device now. It is shown once.
								</p>
								<code
									class="text-text-primary mt-1 block break-all text-meta select-all"
									data-testid="revealed-secret"
									>{{ revealedSecret }}</code
								>
							</div>
						</template>
					</SettingsRow>

					<SettingsRow
						v-slot="{ errorId }"
						label="This device is"
						description="One machine is First, the other is Second. If they match, nothing is ever delivered."
					>
						<SettingsChoice
							:model-value="shareConfig.role"
							:options="ROLE_OPTIONS"
							label="Which device this is"
							:error-id="errorId"
							@update:model-value="(value) => patchConfig({ role: value })"
						/>
					</SettingsRow>

					<!-- **The Updates row's shape, exactly.** The button is the row's
					     trailing control and the outcome renders in `#below`, under both.
					     It sat in `#below` itself until now, which put a lone button on a
					     line of its own below the description and made the row half a head
					     taller than every other row in the view for no reason a reader
					     could see.

					     The row is named for the *subject* and the button for the action,
					     as Version and **Check for updates** are, so the two do not read
					     as the same words twice. -->
					<SettingsRow
						label="Relay connection"
						description="Asks the relay for this device's mailbox. It sends no note and reads no message."
					>
						<template #below>
							<!-- Permanently mounted and empty until there is something to say,
							     for the same reason `SettingsRow`'s own message region is:
							     injecting an element and its text together does not announce. -->
							<p
								v-if="testMessage"
								class="text-text-secondary mt-1.5 text-meta text-pretty"
								data-testid="share-test-result"
							>
								{{ testMessage }}
							</p>
						</template>

						<button
							type="button"
							:class="shareButtonClass"
							:disabled="shareTesting"
							@click="testRelay()"
						>
							{{ shareTesting ? 'Testing…' : 'Test connection' }}
						</button>
					</SettingsRow>

					<!-- The two facts that are neither a control nor a failure, so they
					     have nowhere else to live: that both machines need the same three
					     values, and what a note can carry.

					     **One number, and it is attachment bytes on disk.** The relay's
					     own cap is 20 MiB of ciphertext and base64 grows attachments by
					     about a third on the way in — which is a multiplication the reader
					     was being asked to do against a size they have never seen. The
					     guide keeps that mechanism; a row states the answer. -->
					<p class="text-text-secondary py-3 text-meta text-pretty">
						Both machines need the same relay URL, relay token and pairing secret. One shared note
						carries about 14 MB of attachments.
					</p>
				</template>
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

				<!-- Moved out of a one-row "Startup" section (2026-08-08): launching at
				     login is the panel conducting itself, the thing this section is
				     named for, and a section of one row was the same shape the Privacy
				     and Notifications folds had already retired. -->
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

		<!-- The same treatment the list mounts, so a file is droppable while the
		     settings are open too. Its overlay is absolute against the panel
		     surface, so it costs this column no layout. -->
		<DropTarget />
	</div>
</template>
