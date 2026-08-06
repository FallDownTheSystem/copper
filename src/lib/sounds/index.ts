/**
 * The two entry points Copper uses, and deliberately no more. The reference app's
 * `bind` export and its `data-sound-*` delegated binding are not ported at all
 * (task-012 ruling OQ6); `hold`, `playRecipe` and the recipe table stay reachable
 * through `./engine` and `./recipes` but are not offered here.
 *
 * That narrowness is the point. Every sound point is an explicit `play()` call
 * from `useSounds`, which is what keeps the seven of them enumerable — and a
 * barrel handing out the whole 51-recipe palette is exactly the escape hatch that
 * would stop being true.
 */
export { play, setEnabled } from './engine'
export type { SoundName } from './recipes'
