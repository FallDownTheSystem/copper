/**
 * The sound engine's public surface, copied from the reference app minus its
 * `bind` export — Copper's `data-sound-*` delegated binding is not ported at all
 * (task-012 ruling OQ6). Every sound point here is an explicit `play()` call
 * from `useSounds`, which is what keeps the seven of them enumerable.
 *
 * Prefer `useSounds()` to importing this directly: `play` is the whole palette
 * and the composable is the seven interactions Copper has actually decided to
 * sound.
 */
export { hold, holdRecipe, play, playRecipe, setEnabled } from './engine'
export type { PlayOptions, SoundHandle } from './engine'
export { RECIPES, isSoundName, sounds } from './recipes'
export type {
	Jitter,
	LayerRepeat,
	NoiseLayer,
	Shimmer,
	SoundLayer,
	SoundName,
	SoundRecipe,
	ToneLayer,
} from './recipes'
