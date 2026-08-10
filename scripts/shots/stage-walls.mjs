// Stages the wallpaper backdrops the screenshot harness composites behind the
// panel. They are Windows' own wallpapers, copied from this machine rather than
// committed: the repo is public and the images are Microsoft's to distribute.
// Run once per checkout, before capturing desktop-mode shots.
import { copyFileSync, mkdirSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..')
const out = join(root, 'shots-walls')
mkdirSync(out, { recursive: true })

const web = 'C:/Windows/Web'
const walls = {
	'bloom-light.jpg': `${web}/Wallpaper/Windows/img0.jpg`,
	'bloom-dark.jpg': `${web}/Wallpaper/Windows/img19.jpg`,
	'theme-a.jpg': `${web}/Wallpaper/ThemeA/img20.jpg`,
	'theme-b.jpg': `${web}/Wallpaper/ThemeB/img24.jpg`,
	'theme-c.jpg': `${web}/Wallpaper/ThemeC/img28.jpg`,
	'theme-d.jpg': `${web}/Wallpaper/ThemeD/img32.jpg`,
	// Low-value on this machine (spot-1 duplicates bloom-light, spot-2 is
	// low-res) but kept so every wall= value in shots.html resolves.
	'spot-1.jpg': `${web}/Wallpaper/Spotlight/img14.jpg`,
	'spot-2.jpg': `${web}/Wallpaper/Spotlight/img50.jpg`,
}

for (const [name, source] of Object.entries(walls)) {
	try {
		copyFileSync(source, join(out, name))
		console.log(`staged ${name}`)
	} catch (err) {
		console.warn(`skipped ${name}: ${err.message}`)
	}
}
console.log('The mesh-copper and mesh-graphite walls are CSS gradients; no file needed.')
