<script setup lang="ts">
/**
 * One rebindable shortcut: its current binding, an Edit affordance, and the
 * recording state.
 *
 * The control sits **below** the label rather than beside it, which is where this
 * departs from the other rows. A recording box carrying "Press the keys you
 * want…" is wider than the trailing column a 390px panel can spare, and squeezing
 * it there is how the row ends up shifting — or overflowing — the moment
 * recording starts. Full width also lets a four-chip chord render without
 * wrapping mid-chord.
 *
 * State is read from the composables rather than passed down, exactly as the two
 * context menus and `PanelMenu` do: the recorder is module-scoped by necessity
 * (the view-level Escape handler has to consult it), so props would be a second
 * copy of something already shared.
 */
import type { ShortcutTarget } from '@/composables/useSettings'

const props = defineProps<{
	label: string
	description: string
	target: ShortcutTarget
	/** The live binding, as `settings.json` stores it. */
	value: string
	/** The shipped binding, so Reset needs no second copy of the defaults here. */
	defaultValue: string
	error?: string | null
	/** A standing condition, distinct from a failed action — the keyboard hook
	 *  being unavailable, say. */
	note?: string | null
}>()

const {
	isRecording,
	target: recordingTarget,
	pending,
	start,
	cancel,
	onKeydown,
	onKeyup,
} = useShortcutRecorder()
const { resetShortcut } = useSettings()

const box = useTemplateRef<HTMLElement>('box')

/** True only for *this* row — one recording is live at a time, and the other rows
 *  must keep showing their bindings rather than all entering the state at once. */
const recording = computed(() => isRecording.value && recordingTarget.value === props.target)

/** `"Shift Shift"` and friends: the same modifier twice, separated by a space. */
const doubleTap = computed(() => {
	const parts = props.value.split(' ')
	return parts.length === 2 && parts[0] === parts[1] ? parts[0] : null
})

const chips = computed(() => (doubleTap.value ? [doubleTap.value] : props.value.split('+')))

const canReset = computed(() => props.value !== props.defaultValue)

const recordingAnnouncement = computed(() =>
	recording.value ? 'Recording. Press the keys you want.' : '',
)

/**
 * The field itself takes focus, because the button that opened it has just
 * unmounted and nothing else would move focus into a control that did not exist
 * a tick ago.
 *
 * Driven from the crossfade's own hook rather than from a watcher on `recording`.
 * The states swap `out-in`, so at the tick the flag changes the outgoing branch is
 * still leaving and the box has not been created yet — a `nextTick` focus call
 * there would find nothing and recording would begin with focus on the body,
 * which is the one state this control cannot be in.
 *
 * `@enter` rather than `@after-enter`: the box exists the moment it is inserted,
 * and every keystroke between the click and the focus call goes nowhere — so the
 * focus belongs at the earliest hook that can land it, not after the fade the
 * keys don't care about.
 */
function focusRecordingBox() {
	if (recording.value) box.value?.focus()
}
</script>

<template>
	<SettingsRow :label="label" :description="description">
		<template #below>
			<!-- The two states share a height, so entering recording shifts nothing.
			     The spec asks for exactly that, and it is why the swap cross-fades
			     opacity rather than animating a box that grows.

			     `out-in` rather than a true crossfade: the two states occupy the same
			     36px of row, so overlapping them would put the chips and the recording
			     box on top of each other for the length of the fade. Leaving is the
			     faster half — the state being left is already answered. -->
			<div class="mt-2 flex min-h-9 items-center gap-2">
				<Transition
					mode="out-in"
					enter-active-class="transition-opacity duration-base ease-out-quint"
					enter-from-class="opacity-0"
					leave-active-class="transition-opacity duration-fast ease-out-quint"
					leave-to-class="opacity-0"
					@enter="focusRecordingBox"
				>
					<!-- One wrapper per state, which the Transition requires and which is
					     the only reason these divs exist: the row's own flex layout is
					     reproduced on them so the chips, the box and the buttons sit
					     exactly where they did. -->
					<div v-if="!recording" class="flex w-full min-w-0 items-center gap-2">
						<div class="flex min-w-0 flex-wrap items-center gap-1">
							<KbdChord :keys="chips" />
							<!-- Plain text, never a chip: "double-tap" is an interaction, and
							     chip styling would claim it is a key you can press. -->
							<span v-if="doubleTap" class="text-text-secondary text-meta">double-tap</span>
						</div>

						<!-- `gap-2`, not `gap-1`: these two carry 44px hit areas, and expanded
						     areas must never overlap or each makes part of the other
						     unhittable. Eight pixels is what puts their centres far enough
						     apart for two 44px boxes to clear each other. -->
						<div class="ml-auto flex shrink-0 items-center gap-2">
							<button type="button" class="panel-button hit-44 relative" @click="start(target)">
								Edit
							</button>
							<!-- No confirmation: rebinding a key is not destructive. Shown only
							     when there is something to undo. -->
							<button
								v-if="canReset"
								type="button"
								aria-label="Reset to default"
								title="Reset to default"
								class="icon-button hit-44 relative"
								@click="resetShortcut(target)"
							>
								<IconLucideRotateCcw class="size-4" aria-hidden="true" focusable="false" />
							</button>
						</div>
					</div>

					<div v-else class="flex w-full min-w-0 items-center gap-2">
						<!-- Focusable on purpose: the keys have to land somewhere, and every
						     one of them is prevented here except Tab, which is the way out
						     that does not need a mouse or a remembered Escape. -->
						<div
							ref="box"
							tabindex="0"
							role="group"
							aria-label="Recording a shortcut"
							class="recording-box flex min-h-9 min-w-0 flex-1 items-center gap-1 rounded-md px-3 outline-none"
							@keydown="onKeydown"
							@keyup="onKeyup"
						>
							<KbdChord v-if="pending.length > 0" :keys="pending" />
							<span v-else class="text-text-secondary text-meta">Press the keys you want…</span>
						</div>

						<div class="flex shrink-0 items-center gap-2">
							<span class="text-text-secondary text-meta">Esc to cancel</span>
							<button type="button" class="panel-button hit-44 relative" @click="cancel">
								Cancel
							</button>
						</div>
					</div>
				</Transition>
			</div>

			<!-- A failure the user did not ask for is urgent rather than routine, so
			     this is an alert. The row above still shows the previous binding,
			     because nothing actually changed. -->
			<p v-if="error" class="text-text-primary mt-1.5 flex items-start gap-1.5 text-meta">
				<IconLucideAlertCircle
					class="mt-0.5 size-3.5 shrink-0"
					aria-hidden="true"
					focusable="false"
				/>
				<span>{{ error }}</span>
			</p>

			<p v-else-if="note" class="text-text-secondary mt-1.5 text-meta text-pretty">{{ note }}</p>

			<!-- Two permanent regions, not one node swapping its role. Changing `role`
			     re-registers the live region, and an announcement written in the same
			     tick can be dropped on the floor — which is worst for the alert, the
			     one message that matters most. Both are pre-rendered and empty:
			     injecting an element and its text together does not announce, only a
			     text change inside a region already in the accessibility tree does. -->
			<span class="sr-only" role="status">{{ recordingAnnouncement }}</span>
			<span class="sr-only" role="alert">{{ error ?? '' }}</span>
		</template>
	</SettingsRow>
</template>

<style scoped>
/* The pulse is on a pseudo-element rather than on the box, so the chord building
   up inside stays at full opacity — a chip that faded while the user was reading
   it would be the one thing this animation must not do. */
.recording-box {
	position: relative;
}

.recording-box::after {
	content: '';
	position: absolute;
	inset: 0;
	border-radius: inherit;
	box-shadow: inset 0 0 0 2px var(--accent-ring);
	pointer-events: none;
	/* A slow opacity oscillation, not a blink — nowhere near the three-per-second
	   flash threshold. WCAG 2.2.2 does not bite even though this can run past five
	   seconds: it is user-initiated, it is the focus of that moment rather than
	   something running alongside other content, and any key or the Cancel button
	   ends it. */
	animation: recording-pulse 1.6s ease-in-out infinite;
}

@keyframes recording-pulse {
	0%,
	100% {
		opacity: 1;
	}
	50% {
		opacity: 0.4;
	}
}

/* Reduce, do not remove. The ring is the box-shadow, so stopping the animation
   leaves it static rather than leaving nothing — and the placeholder text
   already carries the state. */
@media (prefers-reduced-motion: reduce) {
	.recording-box::after {
		animation: none;
	}
}
</style>
