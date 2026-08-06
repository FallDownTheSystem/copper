/**
 * The sound palette — layer/recipe types plus the built-in recipes.
 * Each sound has its own distinct shape — a chime, an arpeggio, a pitch
 * glide, a warm pad, a breath — rather than being a volume/EQ tweak on
 * the same click. Add a new one here without touching any audio graph code.
 *
 * The first ten recipes are from cuelume (MIT, Daniel Belyi); the rest
 * are this project's own additions.
 */

/** Replays a layer as a burst of hits — ratchets, bounces, zippers, countdowns. */
export type LayerRepeat = {
	/** Total number of hits, including the first. */
	count: number
	/** Seconds between the first two hits. */
	interval: number
	/** Multiplier applied to the interval after each hit — below 1 accelerates. */
	intervalFactor?: number
	/** Semitones added to the layer's pitch on each successive hit. */
	pitchStep?: number
	/** Multiplier applied to the layer's peak on each successive hit — below 1 fades out. */
	gainFactor?: number
}

/** Per-hit randomization so rapid-fire sounds never repeat exactly. */
export type Jitter = {
	/** Maximum random pitch offset, in cents (±). */
	cents?: number
	/** Maximum random gain variation, as a fraction of peak (±). */
	gain?: number
	/** Maximum random start-time shift, in seconds (±). */
	time?: number
}

type BaseLayer = {
	/** Seconds after the trigger that this layer starts. */
	offset?: number
	/** Fade-in time, in seconds. */
	attack: number
	/** Fade-out time, in seconds, starting right after the attack. */
	decay: number
	/** Peak volume reached at the end of the attack. */
	peak: number
	/** Stereo position for this layer, -1 (left) to 1 (right). */
	pan?: number
	/** Replays this layer as a burst of hits. Ignored by held sounds. */
	repeat?: LayerRepeat
}

/** A single note — the building block for chimes, arpeggios, and pads. */
export type ToneLayer = BaseLayer & {
	kind: 'tone'
	waveform: OscillatorType
	frequency: number
	/** Detune in cents, for a gentle chorus/beating effect between layers. */
	detune?: number
	/** If set, the pitch glides smoothly from `frequency` to this value. */
	glideTo?: number
	/** How long the glide takes, in seconds. Defaults to attack + decay. */
	glideTime?: number
}

/** A soft filtered noise bed — used for breathy, textural layers. */
export type NoiseLayer = BaseLayer & {
	kind: 'noise'
	filterType: BiquadFilterType
	filterFrequency: number
	filterQ?: number
	/** If set, the filter sweeps smoothly from `filterFrequency` to this value. */
	filterGlideTo?: number
	/** How long the filter sweep takes, in seconds. Defaults to attack + decay. */
	filterGlideTime?: number
}

export type SoundLayer = ToneLayer | NoiseLayer

/** A soft, spacious echo tail applied to the whole sound — the "magic dust". */
export type Shimmer = {
	delay: number
	feedback: number
	wet: number
	lowpass: number
}

export type SoundRecipe = {
	masterGain: number
	layers: readonly SoundLayer[]
	shimmer?: Shimmer
	/** Randomizes each hit slightly so repeated plays sound organic. */
	jitter?: Jitter
}

export const RECIPES = {
	/** A soft two-note ascending bell, like an iOS/macOS confirmation tink. */
	chime: {
		masterGain: 0.5,
		layers: [
			{ kind: 'tone', waveform: 'sine', frequency: 1046.5, attack: 0.006, decay: 0.22, peak: 0.09 },
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1568,
				offset: 0.09,
				attack: 0.006,
				decay: 0.26,
				peak: 0.08,
			},
		],
		shimmer: { delay: 0.12, feedback: 0.25, wet: 0.18, lowpass: 4000 },
	},
	/** A quick ascending twinkle of four notes — bright and playful. */
	sparkle: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1760,
				offset: 0,
				attack: 0.003,
				decay: 0.09,
				peak: 0.045,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 2217,
				offset: 0.045,
				attack: 0.003,
				decay: 0.09,
				peak: 0.04,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 2637,
				offset: 0.09,
				attack: 0.003,
				decay: 0.1,
				peak: 0.038,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 3520,
				offset: 0.135,
				attack: 0.003,
				decay: 0.12,
				peak: 0.032,
			},
		],
		shimmer: { delay: 0.07, feedback: 0.35, wet: 0.22, lowpass: 6000 },
	},
	/** A single note gliding smoothly downward, like a drop of water. */
	droplet: {
		masterGain: 0.55,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1200,
				glideTo: 550,
				glideTime: 0.14,
				attack: 0.004,
				decay: 0.2,
				peak: 0.075,
			},
		],
		shimmer: { delay: 0.09, feedback: 0.2, wet: 0.15, lowpass: 3000 },
	},
	/** A warm, slow-swelling pad from two gently detuned sines. */
	bloom: {
		masterGain: 0.5,
		layers: [
			{ kind: 'tone', waveform: 'sine', frequency: 528, attack: 0.06, decay: 0.32, peak: 0.06 },
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 528,
				detune: 12,
				attack: 0.06,
				decay: 0.34,
				peak: 0.05,
			},
		],
		shimmer: { delay: 0.15, feedback: 0.2, wet: 0.12, lowpass: 2500 },
	},
	/** The quietest option — a breathy, textureless swell for dense lists. */
	whisper: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'noise',
				filterType: 'lowpass',
				filterFrequency: 1200,
				filterQ: 0.7,
				attack: 0.04,
				decay: 0.16,
				peak: 0.05,
			},
		],
	},
	/** A focused, bandpass-filtered tick with a bright sine ping on top — crisp and instant. */
	tick: {
		masterGain: 0.4,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 5400,
				filterQ: 1.8,
				attack: 0.001,
				decay: 0.018,
				peak: 0.14,
			},
			{ kind: 'tone', waveform: 'sine', frequency: 2600, attack: 0.001, decay: 0.012, peak: 0.018 },
		],
	},
	/** A dull, muted knock — the "down" half of a press/release pair, like a key bottoming out. */
	press: {
		masterGain: 0.4,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 1700,
				filterQ: 1.4,
				attack: 0.001,
				decay: 0.02,
				peak: 0.13,
			},
		],
	},
	/** A brighter, springier tick — the "up" half of a press/release pair, like a key returning. */
	release: {
		masterGain: 0.4,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 4600,
				filterQ: 1.8,
				attack: 0.001,
				decay: 0.016,
				peak: 0.12,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 3200,
				offset: 0.006,
				attack: 0.001,
				decay: 0.05,
				peak: 0.02,
			},
		],
	},
	/** A two-part click-clack, like a mechanical switch flipping between states. */
	toggle: {
		masterGain: 0.4,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 2200,
				filterQ: 1.6,
				attack: 0.001,
				decay: 0.016,
				peak: 0.12,
			},
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 3800,
				filterQ: 1.6,
				offset: 0.024,
				attack: 0.001,
				decay: 0.02,
				peak: 0.1,
			},
		],
	},
	/** A short, warm three-note ascending confirmation — "done", not a fanfare. */
	success: {
		masterGain: 0.5,
		layers: [
			{ kind: 'tone', waveform: 'sine', frequency: 880, attack: 0.004, decay: 0.09, peak: 0.06 },
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1108.73,
				offset: 0.06,
				attack: 0.004,
				decay: 0.1,
				peak: 0.06,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1318.51,
				offset: 0.12,
				attack: 0.004,
				decay: 0.18,
				peak: 0.07,
			},
		],
		shimmer: { delay: 0.1, feedback: 0.22, wet: 0.16, lowpass: 4500 },
	},
	/** A sour descending pair — the landing note doubled slightly flat so it beats. */
	error: {
		masterGain: 0.5,
		layers: [
			{ kind: 'tone', waveform: 'sine', frequency: 622.25, attack: 0.005, decay: 0.11, peak: 0.07 },
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 466.16,
				offset: 0.1,
				attack: 0.005,
				decay: 0.22,
				peak: 0.075,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 466.16,
				detune: -18,
				offset: 0.1,
				attack: 0.005,
				decay: 0.22,
				peak: 0.04,
			},
		],
	},
	/** The same mid-pitch ping twice — insistence without alarm. */
	warning: {
		masterGain: 0.5,
		layers: [
			{ kind: 'tone', waveform: 'sine', frequency: 932.33, attack: 0.004, decay: 0.1, peak: 0.07 },
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 932.33,
				offset: 0.16,
				attack: 0.004,
				decay: 0.14,
				peak: 0.07,
			},
		],
	},
	/** A gentle descending ding-dong — "something arrived", softer than chime. */
	notification: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1318.51,
				attack: 0.005,
				decay: 0.18,
				peak: 0.075,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 987.77,
				offset: 0.13,
				attack: 0.005,
				decay: 0.28,
				peak: 0.07,
			},
		],
		shimmer: { delay: 0.11, feedback: 0.25, wet: 0.18, lowpass: 4200 },
	},
	/** A bubbly upward blip with a tiny noise snap — a bubble bursting. */
	pop: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 380,
				glideTo: 950,
				glideTime: 0.05,
				attack: 0.002,
				decay: 0.075,
				peak: 0.11,
			},
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 900,
				filterQ: 1.2,
				attack: 0.001,
				decay: 0.02,
				peak: 0.05,
			},
		],
	},
	/** An airy noise sweep rising through the spectrum — motion without pitch. */
	whoosh: {
		masterGain: 0.55,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 400,
				filterGlideTo: 2800,
				filterQ: 2.2,
				attack: 0.05,
				decay: 0.22,
				peak: 0.14,
			},
		],
	},
	/** A single note swooping upward and away — droplet's outbound mirror. */
	send: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 540,
				glideTo: 1500,
				glideTime: 0.16,
				attack: 0.01,
				decay: 0.18,
				peak: 0.07,
			},
		],
		shimmer: { delay: 0.09, feedback: 0.22, wet: 0.16, lowpass: 4000 },
	},
	/** A low plunge with a dull thump — something falling out of existence. */
	delete: {
		masterGain: 0.55,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 320,
				glideTo: 90,
				glideTime: 0.16,
				attack: 0.004,
				decay: 0.2,
				peak: 0.1,
			},
			{
				kind: 'noise',
				filterType: 'lowpass',
				filterFrequency: 500,
				filterQ: 0.8,
				attack: 0.002,
				decay: 0.06,
				peak: 0.07,
			},
		],
	},
	/** A retro square-wave chirp jumping up an octave — 8-bit "select". */
	blip: {
		masterGain: 0.35,
		layers: [
			{
				kind: 'tone',
				waveform: 'square',
				frequency: 880,
				glideTo: 1760,
				glideTime: 0.035,
				attack: 0.001,
				decay: 0.06,
				peak: 0.035,
			},
		],
	},
	/** A soft low sine sagging slightly downward — blip's mellow opposite. */
	boop: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 260,
				glideTo: 200,
				glideTime: 0.09,
				attack: 0.004,
				decay: 0.11,
				peak: 0.11,
			},
		],
	},
	/** A four-note major arpeggio landing on a doubled octave — a small celebration. */
	tada: {
		masterGain: 0.5,
		layers: [
			{ kind: 'tone', waveform: 'sine', frequency: 523.25, attack: 0.005, decay: 0.1, peak: 0.055 },
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 659.25,
				offset: 0.07,
				attack: 0.005,
				decay: 0.1,
				peak: 0.055,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 783.99,
				offset: 0.14,
				attack: 0.005,
				decay: 0.12,
				peak: 0.06,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1046.5,
				offset: 0.21,
				attack: 0.005,
				decay: 0.3,
				peak: 0.075,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1046.5,
				detune: 10,
				offset: 0.21,
				attack: 0.005,
				decay: 0.32,
				peak: 0.05,
			},
		],
		shimmer: { delay: 0.12, feedback: 0.3, wet: 0.22, lowpass: 5000 },
	},
	/** A long glassy strike with inharmonic partials — the most musical, decorative option. */
	bell: {
		masterGain: 0.45,
		layers: [
			{ kind: 'tone', waveform: 'sine', frequency: 880, attack: 0.003, decay: 0.7, peak: 0.07 },
			{ kind: 'tone', waveform: 'sine', frequency: 1976, attack: 0.003, decay: 0.5, peak: 0.035 },
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 2495,
				detune: 8,
				attack: 0.003,
				decay: 0.35,
				peak: 0.02,
			},
		],
		shimmer: { delay: 0.16, feedback: 0.3, wet: 0.2, lowpass: 5000 },
	},
	/** A soft, deep landing bump — weight without brightness. */
	thud: {
		masterGain: 0.6,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 140,
				glideTo: 55,
				glideTime: 0.07,
				attack: 0.002,
				decay: 0.13,
				peak: 0.14,
			},
			{
				kind: 'noise',
				filterType: 'lowpass',
				filterFrequency: 240,
				filterQ: 0.7,
				attack: 0.001,
				decay: 0.045,
				peak: 0.09,
			},
		],
	},
	/** Two tight snaps around a low mirror-slap thump — a camera shutter. */
	shutter: {
		masterGain: 0.45,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 4500,
				filterQ: 1.6,
				attack: 0.001,
				decay: 0.012,
				peak: 0.12,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 180,
				glideTo: 110,
				glideTime: 0.04,
				attack: 0.002,
				decay: 0.05,
				peak: 0.09,
			},
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 2800,
				filterQ: 1.4,
				offset: 0.05,
				attack: 0.001,
				decay: 0.02,
				peak: 0.1,
			},
		],
	},
	/** A falling click pair settling into a low thud — a bolt sliding home. */
	lock: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 3200,
				filterQ: 1.8,
				attack: 0.001,
				decay: 0.015,
				peak: 0.1,
			},
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 2000,
				filterQ: 1.8,
				offset: 0.055,
				attack: 0.001,
				decay: 0.018,
				peak: 0.11,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 150,
				glideTo: 90,
				glideTime: 0.06,
				offset: 0.09,
				attack: 0.003,
				decay: 0.09,
				peak: 0.1,
			},
		],
	},
	/** A rising click pair opening into a bright ping — lock's mirror. */
	unlock: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 2000,
				filterQ: 1.8,
				attack: 0.001,
				decay: 0.015,
				peak: 0.11,
			},
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 3400,
				filterQ: 1.8,
				offset: 0.05,
				attack: 0.001,
				decay: 0.016,
				peak: 0.1,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1567.98,
				offset: 0.09,
				attack: 0.003,
				decay: 0.16,
				peak: 0.055,
			},
		],
		shimmer: { delay: 0.1, feedback: 0.2, wet: 0.12, lowpass: 4500 },
	},
	/** Two square-wave notes a fourth apart — the classic arcade coin. */
	coin: {
		masterGain: 0.35,
		layers: [
			{
				kind: 'tone',
				waveform: 'square',
				frequency: 987.77,
				attack: 0.002,
				decay: 0.07,
				peak: 0.03,
			},
			{
				kind: 'tone',
				waveform: 'square',
				frequency: 1318.51,
				offset: 0.08,
				attack: 0.002,
				decay: 0.22,
				peak: 0.032,
			},
		],
	},
	/** One sharp mid-band crack with a small low body — fingers snapping. */
	snap: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 2600,
				filterQ: 2.5,
				attack: 0.001,
				decay: 0.03,
				peak: 0.16,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 240,
				glideTo: 160,
				glideTime: 0.03,
				attack: 0.001,
				decay: 0.04,
				peak: 0.06,
			},
		],
	},
	/** A drooping minor two-note "uh-oh" — playful and forgiving, gentler than error. */
	oops: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 783.99,
				glideTo: 740,
				glideTime: 0.08,
				attack: 0.008,
				decay: 0.1,
				peak: 0.07,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 659.25,
				glideTo: 622.25,
				glideTime: 0.1,
				offset: 0.12,
				attack: 0.008,
				decay: 0.16,
				peak: 0.075,
			},
		],
	},
	/** A full major triad plus octave struck at once — tada's solemn sibling. */
	chord: {
		masterGain: 0.45,
		layers: [
			{ kind: 'tone', waveform: 'sine', frequency: 523.25, attack: 0.008, decay: 0.4, peak: 0.055 },
			{ kind: 'tone', waveform: 'sine', frequency: 659.25, attack: 0.008, decay: 0.38, peak: 0.05 },
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 783.99,
				attack: 0.008,
				decay: 0.36,
				peak: 0.045,
			},
			{ kind: 'tone', waveform: 'sine', frequency: 1046.5, attack: 0.008, decay: 0.42, peak: 0.04 },
		],
		shimmer: { delay: 0.13, feedback: 0.25, wet: 0.18, lowpass: 4500 },
	},
	/** One long pure ping trailing heavy echoes — a submarine sweep. */
	sonar: {
		masterGain: 0.5,
		layers: [
			{ kind: 'tone', waveform: 'sine', frequency: 1174.66, attack: 0.004, decay: 0.5, peak: 0.08 },
		],
		shimmer: { delay: 0.22, feedback: 0.45, wet: 0.3, lowpass: 3500 },
	},
	/** Two low thumps in a lub-dub rhythm, the second softer. */
	heartbeat: {
		masterGain: 0.6,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 110,
				glideTo: 55,
				glideTime: 0.09,
				attack: 0.004,
				decay: 0.12,
				peak: 0.14,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 95,
				glideTo: 50,
				glideTime: 0.08,
				offset: 0.16,
				attack: 0.004,
				decay: 0.1,
				peak: 0.11,
			},
		],
	},
	/** A woody mallet strike — triangle fundamental, bright partial, tiny contact noise. */
	marimba: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'tone',
				waveform: 'triangle',
				frequency: 523.25,
				attack: 0.002,
				decay: 0.18,
				peak: 0.09,
			},
			{ kind: 'tone', waveform: 'sine', frequency: 2093, attack: 0.002, decay: 0.06, peak: 0.03 },
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 1800,
				filterQ: 1,
				attack: 0.001,
				decay: 0.01,
				peak: 0.04,
			},
		],
	},
	/** A breath drawn in — long swell with the filter opening upward. */
	inhale: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'noise',
				filterType: 'lowpass',
				filterFrequency: 700,
				filterGlideTo: 1800,
				filterQ: 0.7,
				attack: 0.22,
				decay: 0.08,
				peak: 0.055,
			},
		],
	},
	/** A breath let out — quick swell fading as the filter closes downward. */
	exhale: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'noise',
				filterType: 'lowpass',
				filterFrequency: 1800,
				filterGlideTo: 600,
				filterQ: 0.7,
				attack: 0.05,
				decay: 0.28,
				peak: 0.055,
			},
		],
	},
	/**
	 * The quietest sound in the palette, built for rapid-fire repetition:
	 * a clean micro-thump — a heartbeat's lub shrunk to hover size. One
	 * pure sine gliding softly downward, no noise at all, so there is no
	 * grain to fatigue on. Gentle jitter keeps sweeps across a list from
	 * sounding robotic.
	 */
	hover: {
		masterGain: 0.3,
		jitter: { cents: 15, gain: 0.2 },
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 260,
				glideTo: 220,
				glideTime: 0.045,
				attack: 0.006,
				decay: 0.04,
				peak: 0.07,
			},
		],
	},
	/** An accelerating burst of rising ticks — a ratchet wheel spinning up. */
	ratchet: {
		masterGain: 0.45,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 3000,
				filterQ: 2,
				attack: 0.001,
				decay: 0.02,
				peak: 0.1,
				repeat: { count: 8, interval: 0.07, intervalFactor: 0.85, pitchStep: 1 },
			},
		],
	},
	/** Thuds settling like a dropped ball — quicker, quieter, slightly higher each time. */
	bounce: {
		masterGain: 0.55,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 180,
				glideTo: 90,
				glideTime: 0.05,
				attack: 0.002,
				decay: 0.08,
				peak: 0.12,
				repeat: { count: 5, interval: 0.28, intervalFactor: 0.55, pitchStep: 0.5, gainFactor: 0.7 },
			},
		],
	},
	/** A humanized keystroke — jitter makes rapid typing sound organic, not machine-gun. */
	type: {
		masterGain: 0.4,
		jitter: { cents: 60, gain: 0.35, time: 0.006 },
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 2400,
				filterQ: 1.2,
				attack: 0.001,
				decay: 0.018,
				peak: 0.1,
			},
			{
				kind: 'tone',
				waveform: 'triangle',
				frequency: 1400,
				attack: 0.001,
				decay: 0.02,
				peak: 0.02,
			},
		],
	},
	/** Two notes answering each other across the stereo field — left, then right. */
	pingpong: {
		masterGain: 0.45,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1046.5,
				pan: -0.8,
				attack: 0.002,
				decay: 0.1,
				peak: 0.07,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1318.51,
				pan: 0.8,
				offset: 0.14,
				attack: 0.002,
				decay: 0.14,
				peak: 0.07,
			},
		],
	},
	/**
	 * A rising swell of detuned tones and opening noise — designed for
	 * `hold()`: it sustains at full charge until released or cancelled.
	 */
	charge: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 220,
				glideTo: 880,
				glideTime: 1.2,
				attack: 0.8,
				decay: 0.25,
				peak: 0.07,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 220,
				detune: 10,
				glideTo: 880,
				glideTime: 1.2,
				attack: 0.8,
				decay: 0.25,
				peak: 0.05,
			},
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 500,
				filterGlideTo: 3000,
				filterGlideTime: 1.2,
				filterQ: 1.5,
				attack: 0.9,
				decay: 0.2,
				peak: 0.05,
			},
		],
	},
	/** Rapid rising micro-ticks, tighter and quieter than ratchet — a zip pulled fast. */
	zipper: {
		masterGain: 0.4,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 2400,
				filterQ: 3,
				attack: 0.001,
				decay: 0.012,
				peak: 0.07,
				repeat: { count: 14, interval: 0.022, pitchStep: 0.35 },
			},
		],
	},
	/** Two jittered bursts in different bands rattling against each other — dice in a cup. */
	clatter: {
		masterGain: 0.45,
		jitter: { cents: 80, gain: 0.4, time: 0.012 },
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 1900,
				filterQ: 1.1,
				attack: 0.001,
				decay: 0.03,
				peak: 0.09,
				repeat: { count: 6, interval: 0.05, gainFactor: 0.88 },
			},
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 3100,
				filterQ: 1.4,
				offset: 0.02,
				attack: 0.001,
				decay: 0.025,
				peak: 0.06,
				repeat: { count: 5, interval: 0.055, gainFactor: 0.85 },
			},
		],
	},
	/** Three steady ticks, then a bright octave "go". */
	countdown: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 659.25,
				attack: 0.003,
				decay: 0.08,
				peak: 0.07,
				repeat: { count: 3, interval: 0.4 },
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1318.51,
				offset: 1.2,
				attack: 0.003,
				decay: 0.3,
				peak: 0.09,
			},
		],
		shimmer: { delay: 0.1, feedback: 0.2, wet: 0.12, lowpass: 4000 },
	},
	/** A twinkle spilling downward, slowing and fading as it falls — sparkle's descent. */
	cascade: {
		masterGain: 0.45,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 2637,
				attack: 0.002,
				decay: 0.09,
				peak: 0.05,
				repeat: { count: 6, interval: 0.06, intervalFactor: 1.1, pitchStep: -2, gainFactor: 0.85 },
			},
		],
		shimmer: { delay: 0.08, feedback: 0.25, wet: 0.15, lowpass: 5000 },
	},
	/** A whoosh that travels the stereo field — rising on the left, answered falling on the right. */
	crossing: {
		masterGain: 0.55,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 600,
				filterGlideTo: 2200,
				filterQ: 2,
				pan: -0.9,
				attack: 0.04,
				decay: 0.16,
				peak: 0.11,
			},
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 2200,
				filterGlideTo: 900,
				filterQ: 2,
				pan: 0.9,
				offset: 0.12,
				attack: 0.05,
				decay: 0.2,
				peak: 0.11,
			},
		],
	},
	/**
	 * A calm beating pad on a low fifth — designed for `hold()` as a
	 * waiting/loading bed that sustains until released.
	 */
	drone: {
		masterGain: 0.4,
		layers: [
			{ kind: 'tone', waveform: 'sine', frequency: 220, attack: 0.4, decay: 0.6, peak: 0.05 },
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 220,
				detune: 7,
				attack: 0.4,
				decay: 0.65,
				peak: 0.04,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 330,
				detune: -5,
				attack: 0.5,
				decay: 0.55,
				peak: 0.025,
			},
			{
				kind: 'noise',
				filterType: 'lowpass',
				filterFrequency: 500,
				filterQ: 0.5,
				attack: 0.5,
				decay: 0.6,
				peak: 0.03,
			},
		],
	},
	/** A soft paper swish ending in a tiny snap — a page flicking over. */
	pageturn: {
		masterGain: 0.45,
		layers: [
			{
				kind: 'noise',
				filterType: 'bandpass',
				filterFrequency: 1100,
				filterGlideTo: 2600,
				filterGlideTime: 0.07,
				filterQ: 0.9,
				attack: 0.015,
				decay: 0.09,
				peak: 0.07,
			},
			{
				kind: 'noise',
				filterType: 'highpass',
				filterFrequency: 3500,
				filterQ: 0.7,
				offset: 0.05,
				attack: 0.004,
				decay: 0.03,
				peak: 0.03,
			},
		],
	},
	/** A tiny rising micro-blip, hover's inbound sibling — "you're in the field now". */
	focus: {
		masterGain: 0.35,
		jitter: { cents: 8, gain: 0.12 },
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 520,
				glideTo: 700,
				glideTime: 0.05,
				attack: 0.008,
				decay: 0.06,
				peak: 0.045,
			},
		],
	},
	/** A low double "dut-dut" — a gentle head shake for denied or disabled actions. */
	nope: {
		masterGain: 0.45,
		layers: [
			{
				kind: 'tone',
				waveform: 'triangle',
				frequency: 165,
				attack: 0.004,
				decay: 0.05,
				peak: 0.09,
				repeat: { count: 2, interval: 0.09 },
			},
		],
	},
	/** One note up, answered back down — a completed round trip for refresh and sync. */
	sync: {
		masterGain: 0.5,
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 620,
				glideTo: 1050,
				glideTime: 0.12,
				attack: 0.008,
				decay: 0.13,
				peak: 0.06,
			},
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 1050,
				glideTo: 660,
				glideTime: 0.12,
				offset: 0.14,
				attack: 0.008,
				decay: 0.15,
				peak: 0.06,
			},
		],
		shimmer: { delay: 0.09, feedback: 0.18, wet: 0.12, lowpass: 3500 },
	},
	/** A tiny wet drop — a softer, rounder detent than tick. */
	plip: {
		masterGain: 0.4,
		jitter: { cents: 25, gain: 0.2 },
		layers: [
			{
				kind: 'tone',
				waveform: 'sine',
				frequency: 950,
				glideTo: 550,
				glideTime: 0.035,
				attack: 0.002,
				decay: 0.04,
				peak: 0.05,
			},
		],
	},
} as const satisfies Record<string, SoundRecipe>

export type SoundName = keyof typeof RECIPES

export function isSoundName(value: unknown): value is SoundName {
	return typeof value === 'string' && Object.prototype.hasOwnProperty.call(RECIPES, value)
}

/** All available sound names, derived from the recipe palette. */
export const sounds = Object.keys(RECIPES) as readonly SoundName[]
