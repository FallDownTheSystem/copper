import { createApp } from 'vue'
import App from './App.vue'
import 'unfonts.css'
/**
 * Inter's italic is a **separate face**, and `unfonts.css` above carries only the
 * upright one — so `font-style: italic` had nothing to draw with. `body` sets
 * `font-synthesis: none` deliberately (a missing weight must not silently render
 * as a fake one), which turns that gap into markdown emphasis rendering upright
 * rather than into a passable oblique.
 *
 * Imported here rather than configured in `Unfonts`. The plugin does accept
 * `variable: { wght: true, ital: true }`, but reading its fontsource loader shows
 * that branch emitting `wght-italic.css` *instead of* `index.css`, not alongside
 * it — the config that looks like "upright plus italic" is the one that loses the
 * upright face.
 */
import '@fontsource-variable/inter/wght-italic.css'
import './assets/main.css'

createApp(App).mount('#app')
