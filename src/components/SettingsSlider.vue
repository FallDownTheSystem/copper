<script setup lang="ts">
/**
 * A continuous numeric preference, as a track the user drags.
 *
 * **Two events, not one, and that is the whole design.** `update:modelValue`
 * fires on every pointer move and every arrow press; `commit` fires once, when
 * the interaction settles. Callers preview the first and persist the second —
 * a slider bound to a single event either repaints thirty times a drag with no
 * feedback in between, or writes to disk thirty times a drag. Reka's
 * `valueCommit` already fires on both settle paths (pointer-up and each keyboard
 * step), so the split costs nothing to obtain.
 *
 * Reka's Slider is a *range* primitive: its value is an array and its thumbs are
 * a collection. This wraps the single-thumb case, because that is the only shape
 * a settings row has ever needed and unwrapping the array at four call sites is
 * four places to get the index wrong.
 *
 * The thumb — not the root — is the control: `SliderThumbImpl` renders
 * `role="slider"` with `aria-valuenow/min/max` and takes its name from the
 * attributes forwarded here. `aria-valuetext` is required rather than optional
 * because a raw value is not always a value a person has words for; the vibrancy
 * dial's `1.35` announces as "135%" through it.
 */
import { SliderRange, SliderRoot, SliderThumb, SliderTrack } from 'reka-ui'

const props = defineProps<{
	modelValue: number
	min: number
	max: number
	step: number
	/** The control's accessible name. The row's label is a sibling, not an
	 *  ancestor, so it does not name this on its own. */
	label: string
	/** The value in the words the user reads it in — announced by the thumb and
	 *  shown beside the track, from one string so the two cannot disagree. */
	valueText: string
	/** The row's error region, when the row has one. Present only while the error
	 *  is, so the control is not described by an empty node the rest of the time. */
	errorId?: string
}>()

const emit = defineEmits<{
	/** Live, per pointer move and per arrow press. Nothing durable. */
	'update:modelValue': [value: number]
	/** The settled value: pointer released, or one keyboard step finished. */
	commit: [value: number]
}>()

const model = computed(() => [props.modelValue])

function onUpdate(payload: number[] | undefined) {
	const next = payload?.[0]
	if (next !== undefined) emit('update:modelValue', next)
}

function onCommit(payload: number[]) {
	const next = payload[0]
	if (next !== undefined) emit('commit', next)
}
</script>

<template>
	<div class="mt-2 flex items-center gap-3">
		<!-- 44px of height on the root rather than `hit-44` on the thumb. The
		     utility centres a fixed 44×44 box on the element it is given, which is
		     the right shape for an icon button and the wrong one for a track the
		     width of the panel — the pointer has to be able to land anywhere along
		     it, not only within 22px of the thumb. Giving the root real box height
		     is `accessibility.md`'s layout alternative, and it also hands the
		     browser the true geometry for gestures.

		     `touch-none` because a drag along the track would otherwise be claimed
		     by the scroll container this row sits in. -->
		<SliderRoot
			:model-value="model"
			:min="min"
			:max="max"
			:step="step"
			class="relative flex h-11 min-w-0 flex-1 touch-none items-center select-none"
			@update:model-value="onUpdate"
			@value-commit="onCommit"
		>
			<SliderTrack
				class="bg-surface-hover inset-ring inset-ring-separator relative h-1.5 w-full grow rounded-full"
			>
				<!-- The filled half is the accent, which is what makes the dial legible
				     without reading the number — and on this particular row it is also a
				     sample of the thing being adjusted. -->
				<SliderRange class="bg-accent-ring absolute h-full rounded-full" />
			</SliderTrack>

			<!-- `focus-ring` rather than a treatment of its own: the copper outline and
			     halo are what every other focusable control in the panel shows, and the
			     thumb is round so the utility's radius-following box-shadow needs no
			     geometry of its own. The white fill and 1px edge are `SettingsSwitch`'s
			     thumb, for the same reason — a pale circle on the light panel is
			     otherwise a shape with no edge. -->
			<SliderThumb
				:aria-label="label"
				:aria-valuetext="valueText"
				:aria-invalid="errorId ? 'true' : undefined"
				:aria-describedby="errorId"
				class="focus-ring ring-control-border/40 block size-4 rounded-full bg-white shadow-sm ring-1"
			/>
		</SliderRoot>

		<!-- Hidden from assistive tech: the thumb already announces this exact
		     string as its `aria-valuetext`, and a second copy would be read on every
		     arrow press. `tabular-nums` so the track does not shift as the number
		     changes width, and a fixed width for the same reason. -->
		<span
			aria-hidden="true"
			class="text-text-secondary w-10 shrink-0 text-right text-meta tabular-nums"
		>
			{{ valueText }}
		</span>
	</div>
</template>
