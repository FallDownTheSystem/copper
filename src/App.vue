<script setup lang="ts">
/**
 * The panel's one surface and the switch between its two views.
 *
 * The surface moved up here from `PanelShell` when the second view arrived:
 * background, radius and clip belong to the window, not to either view, and
 * leaving them on the list would have made the settings view a second unrounded
 * rectangle sliding across the first.
 */
import { listen } from '@tauri-apps/api/event'
import { AnimatePresence, motion } from 'motion-v'

const { view, direction, showSettings } = useView()
const { initialize } = useSettings()
useTheme()

/** Copper's own composable rather than VueUse's `usePreferredReducedMotion`
 *  directly: this is the one animation a user is watching while they toggle
 *  "Animate controls", so it has to honour the setting as well as the OS. */
const reduced = useReducedMotion()

/** The view leaves the way it arrived, so the motion says what happened rather
 *  than the opposite of it. */
const sign = computed(() => (direction.value === 'forward' ? 1 : -1))

/** Reduce, do not remove: the translate goes, the cross-fade stays. */
const shift = computed(() => (reduced.value ? 0 : 1))

const EASE = [0.23, 1, 0.32, 1]

const initial = computed(() => ({ opacity: 0, x: 16 * sign.value * shift.value }))
const animate = { opacity: 1, x: 0, transition: { duration: 0.2, ease: EASE } }
/** Exits are faster and smaller than entrances — the thing leaving has already
 *  been read. */
const exit = computed(() => ({
	opacity: 0,
	x: -8 * sign.value * shift.value,
	transition: { duration: 0.15, ease: EASE },
}))

onMounted(() => {
	// Listen, then pull, per task-003's startup contract. `initialize` does both in
	// that order.
	void initialize()

	// The tray's Settings item. Safe as an event, unlike anything emitted from
	// `setup()`: a tray menu cannot be clicked until the webview has loaded and
	// this listener is registered.
	void listen('open-settings', () => showSettings())
})
</script>

<template>
	<div class="panel-surface h-full w-full">
		<!-- `mode="wait"` rather than an overlap: two full-height views crossing
		     inside a 390×660 window would each need absolute positioning and would
		     show through one another over a translucent backdrop. -->
		<AnimatePresence mode="wait" :initial="false">
			<motion.div
				v-if="view === 'settings'"
				key="settings"
				class="h-full w-full"
				:initial="initial"
				:animate="animate"
				:exit="exit"
			>
				<SettingsView />
			</motion.div>
			<motion.div
				v-else
				key="list"
				class="h-full w-full"
				:initial="initial"
				:animate="animate"
				:exit="exit"
			>
				<PanelShell />
			</motion.div>
		</AnimatePresence>
	</div>
</template>
