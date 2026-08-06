/**
 * The audio engine — synthesizes each sound live via the Web Audio API
 * on one shared, lazily created `AudioContext`. No audio files, no
 * dependencies. Every sound carries a gentle envelope (and often a soft
 * shimmer tail) instead of a hard transient, so nothing feels harsh.
 *
 * Adapted from cuelume (MIT, Daniel Belyi), extended with noise-filter
 * sweeps, layer repeats, per-hit jitter, playback options (rate/gain/pan),
 * held sounds, and `playRecipe()` for auditioning ad-hoc recipes.
 */
import type {
	Jitter,
	LayerRepeat,
	NoiseLayer,
	Shimmer,
	SoundLayer,
	SoundName,
	SoundRecipe,
	ToneLayer,
} from './recipes'
import { RECIPES, isSoundName } from './recipes'

const SOURCE_STOP_PADDING = 0.05
const CLEANUP_MARGIN = 0.05
const INAUDIBLE_GAIN = 0.001
const CANCEL_FADE = 0.06
const HELD_NOISE_SECONDS = 1

/** Per-play adjustments applied on top of a recipe. */
export type PlayOptions = {
	/** Pitch multiplier — 2 is an octave up, 0.5 an octave down. */
	rate?: number
	/** Volume multiplier applied to the recipe's master gain. */
	gain?: number
	/** Stereo position for the whole sound, -1 (left) to 1 (right). */
	pan?: number
}

/** Controls a sound started with `hold()` — call exactly one of the two. */
export type SoundHandle = {
	/** Ends the sound through its natural decay. */
	release: () => void
	/** Aborts the sound with a fast fade — for cancelled gestures. */
	cancel: () => void
}

function pitchMultiplier(semitones: number) {
	return 2 ** (semitones / 12)
}

function clampPan(pan: number) {
	return Math.min(1, Math.max(-1, pan))
}

function jitterValues(jitter: Jitter | undefined) {
	if (!jitter) return { pitchMult: 1, gainMult: 1, timeShift: 0 }
	const rand = () => Math.random() * 2 - 1
	return {
		pitchMult: jitter.cents ? pitchMultiplier((rand() * jitter.cents) / 100) : 1,
		gainMult: jitter.gain ? Math.max(0.05, 1 + rand() * jitter.gain) : 1,
		timeShift: jitter.time ? rand() * jitter.time : 0,
	}
}

/** Start offsets for each hit of a (possibly repeated) layer. */
function hitOffsets(repeat: LayerRepeat | undefined): number[] {
	if (!repeat) return [0]
	const count = Math.max(1, Math.round(repeat.count))
	const offsets: number[] = []
	let time = 0
	let interval = repeat.interval
	for (let i = 0; i < count; i++) {
		offsets.push(time)
		time += interval
		interval *= repeat.intervalFactor ?? 1
	}
	return offsets
}

/** Routes a layer through its own panner when it asks for one. */
function layerDestination(
	context: AudioContext,
	master: AudioNode,
	layer: SoundLayer,
	nodes: AudioNode[],
): AudioNode {
	if (!layer.pan) return master
	const panner = context.createStereoPanner()
	panner.pan.value = clampPan(layer.pan)
	panner.connect(master)
	nodes.push(panner)
	return panner
}

type LiveLayer = { gain: GainNode; source: AudioScheduledSourceNode; decay: number }

function renderTone(
	context: AudioContext,
	destination: AudioNode,
	layer: ToneLayer,
	startTime: number,
	pitchMult: number,
	gainMult: number,
	held: boolean,
): LiveLayer {
	const oscillator = context.createOscillator()
	oscillator.type = layer.waveform
	oscillator.frequency.setValueAtTime(layer.frequency * pitchMult, startTime)
	if (layer.detune) oscillator.detune.value = layer.detune
	if (layer.glideTo !== undefined) {
		const glideTime = layer.glideTime ?? layer.attack + layer.decay
		oscillator.frequency.exponentialRampToValueAtTime(
			layer.glideTo * pitchMult,
			startTime + glideTime,
		)
	}
	const gain = context.createGain()
	gain.gain.setValueAtTime(0.0001, startTime)
	gain.gain.exponentialRampToValueAtTime(layer.peak * gainMult, startTime + layer.attack)
	oscillator.connect(gain).connect(destination)
	oscillator.start(startTime)
	if (!held) {
		gain.gain.exponentialRampToValueAtTime(0.0001, startTime + layer.attack + layer.decay)
		oscillator.stop(startTime + layer.attack + layer.decay + SOURCE_STOP_PADDING)
	}
	return { gain, source: oscillator, decay: layer.decay }
}

function renderNoise(
	context: AudioContext,
	destination: AudioNode,
	layer: NoiseLayer,
	startTime: number,
	pitchMult: number,
	gainMult: number,
	held: boolean,
): LiveLayer {
	const duration = held ? HELD_NOISE_SECONDS : layer.attack + layer.decay + SOURCE_STOP_PADDING
	const length = Math.max(1, Math.floor(duration * context.sampleRate))
	const buffer = context.createBuffer(1, length, context.sampleRate)
	const data = buffer.getChannelData(0)
	for (let i = 0; i < length; i++) data[i] = 2 * Math.random() - 1
	const source = context.createBufferSource()
	source.buffer = buffer
	source.loop = held
	const filter = context.createBiquadFilter()
	filter.type = layer.filterType
	filter.frequency.setValueAtTime(layer.filterFrequency * pitchMult, startTime)
	if (layer.filterQ !== undefined) filter.Q.value = layer.filterQ
	if (layer.filterGlideTo !== undefined) {
		const glideTime = layer.filterGlideTime ?? layer.attack + layer.decay
		filter.frequency.exponentialRampToValueAtTime(
			layer.filterGlideTo * pitchMult,
			startTime + glideTime,
		)
	}
	const gain = context.createGain()
	gain.gain.setValueAtTime(0.0001, startTime)
	gain.gain.exponentialRampToValueAtTime(layer.peak * gainMult, startTime + layer.attack)
	source.connect(filter).connect(gain).connect(destination)
	source.start(startTime)
	if (!held) {
		gain.gain.exponentialRampToValueAtTime(0.0001, startTime + layer.attack + layer.decay)
		source.stop(startTime + duration)
	}
	return { gain, source, decay: layer.decay }
}

/** Wires a soft echo/shimmer send off `source`, feeding back into `destination`. */
function attachShimmer(
	context: AudioContext,
	source: AudioNode,
	destination: AudioNode,
	shimmer: Shimmer,
): AudioNode[] {
	const delay = context.createDelay(1)
	delay.delayTime.value = shimmer.delay
	const feedbackFilter = context.createBiquadFilter()
	feedbackFilter.type = 'lowpass'
	feedbackFilter.frequency.value = shimmer.lowpass
	const feedbackGain = context.createGain()
	feedbackGain.gain.value = shimmer.feedback
	const wetGain = context.createGain()
	wetGain.gain.value = shimmer.wet
	source.connect(delay)
	delay.connect(feedbackFilter)
	feedbackFilter.connect(feedbackGain)
	feedbackGain.connect(delay)
	feedbackFilter.connect(wetGain)
	wetGain.connect(destination)
	return [delay, feedbackFilter, feedbackGain, wetGain]
}

function shimmerTail(shimmer: Shimmer | undefined) {
	if (!shimmer || shimmer.feedback <= 0) return 0
	if (shimmer.feedback >= 1) return shimmer.delay
	return shimmer.delay * (1 + Math.ceil(Math.log(INAUDIBLE_GAIN) / Math.log(shimmer.feedback)))
}

/** Builds the shared output chain: master gain, optional whole-sound pan, shimmer. */
function buildOutput(context: AudioContext, recipe: SoundRecipe, options: PlayOptions | undefined) {
	const nodes: AudioNode[] = []
	const master = context.createGain()
	master.gain.value = recipe.masterGain * (options?.gain ?? 1)
	nodes.push(master)
	let out: AudioNode = context.destination
	if (options?.pan) {
		const panner = context.createStereoPanner()
		panner.pan.value = clampPan(options.pan)
		panner.connect(context.destination)
		nodes.push(panner)
		out = panner
	}
	master.connect(out)
	if (recipe.shimmer) nodes.push(...attachShimmer(context, master, out, recipe.shimmer))
	return { master, nodes }
}

function renderRecipe(context: AudioContext, recipe: SoundRecipe, options?: PlayOptions) {
	const now = context.currentTime
	const rate = options?.rate ?? 1
	const { master, nodes } = buildOutput(context, recipe, options)
	let end = 0
	for (const layer of recipe.layers) {
		const offsets = hitOffsets(layer.repeat)
		for (const [hit, hitOffset] of offsets.entries()) {
			const jitter = jitterValues(recipe.jitter)
			const pitchMult =
				rate * pitchMultiplier((layer.repeat?.pitchStep ?? 0) * hit) * jitter.pitchMult
			const gainMult = (layer.repeat?.gainFactor ?? 1) ** hit * jitter.gainMult
			const startTime = Math.max(now, now + (layer.offset ?? 0) + hitOffset + jitter.timeShift)
			const destination = layerDestination(context, master, layer, nodes)
			if (layer.kind === 'tone')
				renderTone(context, destination, layer, startTime, pitchMult, gainMult, false)
			else renderNoise(context, destination, layer, startTime, pitchMult, gainMult, false)
			end = Math.max(end, startTime - now + layer.attack + layer.decay + SOURCE_STOP_PADDING)
		}
	}
	const cleanupAfterMs = (end + shimmerTail(recipe.shimmer) + CLEANUP_MARGIN) * 1000
	setTimeout(() => {
		for (const node of nodes) node.disconnect()
	}, cleanupAfterMs)
}

/** Renders a recipe that sustains at peak until the returned handle ends it. */
function renderHeldRecipe(
	context: AudioContext,
	recipe: SoundRecipe,
	options?: PlayOptions,
): SoundHandle {
	const now = context.currentTime
	const rate = options?.rate ?? 1
	const { master, nodes } = buildOutput(context, recipe, options)
	const jitter = jitterValues(recipe.jitter)
	const live: LiveLayer[] = []
	for (const layer of recipe.layers) {
		const startTime = now + (layer.offset ?? 0)
		const destination = layerDestination(context, master, layer, nodes)
		const pitchMult = rate * jitter.pitchMult
		const gainMult = jitter.gainMult
		if (layer.kind === 'tone')
			live.push(renderTone(context, destination, layer, startTime, pitchMult, gainMult, true))
		else live.push(renderNoise(context, destination, layer, startTime, pitchMult, gainMult, true))
	}
	let stopped = false
	function stop(fade?: number) {
		if (stopped) return
		stopped = true
		const at = context.currentTime
		let maxDecay = 0
		for (const layer of live) {
			const decay = fade ?? layer.decay
			maxDecay = Math.max(maxDecay, decay)
			layer.gain.gain.cancelScheduledValues(at)
			layer.gain.gain.setValueAtTime(Math.max(layer.gain.gain.value, 0.0001), at)
			layer.gain.gain.exponentialRampToValueAtTime(0.0001, at + decay)
			layer.source.stop(at + decay + SOURCE_STOP_PADDING)
		}
		const cleanupAfterMs = (maxDecay + shimmerTail(recipe.shimmer) + CLEANUP_MARGIN) * 1000
		setTimeout(() => {
			for (const node of nodes) node.disconnect()
		}, cleanupAfterMs)
	}
	return { release: () => stop(), cancel: () => stop(CANCEL_FADE) }
}

let sharedContext: AudioContext | null = null
let enabled = true

/** Enables or disables future playback. Preference storage stays with the app. */
export function setEnabled(value: boolean) {
	if (typeof value === 'boolean') enabled = value
}

function getAudioContext(): AudioContext | null {
	if (sharedContext) return sharedContext
	if (typeof window === 'undefined') return null
	const Ctor =
		window.AudioContext ??
		(window as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
	if (!Ctor) return null
	try {
		sharedContext = new Ctor()
	} catch {
		return null
	}
	return sharedContext
}

function render(recipe: SoundRecipe, options?: PlayOptions) {
	if (!enabled) return
	const context = getAudioContext()
	if (!context) return
	if (context.state === 'running') {
		renderRecipe(context, recipe, options)
	} else {
		try {
			void context.resume().then(
				() => {
					if (enabled && context.state === 'running') renderRecipe(context, recipe, options)
				},
				() => {},
			)
		} catch {
			// Some browsers throw synchronously when audio is blocked.
		}
	}
}

/**
 * Plays a named sound immediately. Safe to call from anywhere — lazily
 * creates the shared `AudioContext` on first use, resumes it if the
 * browser started it suspended (e.g. before any user gesture), and is a
 * no-op when Web Audio is unavailable (SSR, old browsers).
 */
export function play(sound: SoundName = 'chime', options?: PlayOptions) {
	if (!isSoundName(sound)) return
	render(RECIPES[sound], options)
}

/** Plays an arbitrary recipe object — the Sound Lab's audition path. */
export function playRecipe(recipe: SoundRecipe, options?: PlayOptions) {
	if (!recipe.layers.length) return
	render(recipe, options)
}

const NOOP_HANDLE: SoundHandle = { release: () => {}, cancel: () => {} }

/**
 * Starts a sound that sustains at peak until the handle ends it — for
 * hold-to-confirm gestures, drags in progress, and loading ambience.
 * Layers ramp in over their attack and hold there; `release()` plays each
 * layer's decay, `cancel()` cuts off with a fast fade. Layer repeats are
 * ignored while held. Always returns a handle, even when audio is
 * unavailable, so call sites never need to branch.
 */
export function hold(sound: SoundName, options?: PlayOptions): SoundHandle {
	if (!isSoundName(sound)) return NOOP_HANDLE
	return holdRecipe(RECIPES[sound], options)
}

/** `hold()` for an arbitrary recipe object. */
export function holdRecipe(recipe: SoundRecipe, options?: PlayOptions): SoundHandle {
	if (!enabled || !recipe.layers.length) return NOOP_HANDLE
	const context = getAudioContext()
	if (!context) return NOOP_HANDLE
	if (context.state === 'running') return renderHeldRecipe(context, recipe, options)
	// Suspended context: start after resume unless the gesture already ended.
	let ended = false
	let inner: SoundHandle | null = null
	try {
		void context.resume().then(
			() => {
				if (!ended && enabled && context.state === 'running')
					inner = renderHeldRecipe(context, recipe, options)
			},
			() => {},
		)
	} catch {
		// Some browsers throw synchronously when audio is blocked.
	}
	return {
		release: () => {
			ended = true
			inner?.release()
		},
		cancel: () => {
			ended = true
			inner?.cancel()
		},
	}
}
