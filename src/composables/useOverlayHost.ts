/**
 * Where overlays are teleported to, and what bounds them.
 *
 * Reka teleports to `document.body` by default, which lands outside the panel
 * root's `overflow: hidden`, outside its rounded rect and outside its
 * `contextmenu` policy — over a transparent region with no surface behind it.
 * `PanelShell` publishes its own in-clip host here once, rather than every menu
 * being drilled two components' worth of props to reach it.
 *
 * Both are plain elements rather than refs because that is what reka wants.
 */

const boundary = ref<HTMLElement | null>(null)
const portalTo = ref<HTMLElement | null>(null)

function setOverlayHost(root: HTMLElement | null, host: HTMLElement | null) {
	boundary.value = root
	portalTo.value = host
}

export function useOverlayHost() {
	// Not wrapped in `readonly()`: that would deep-wrap an `HTMLElement`, and reka
	// wants the node itself.
	return { boundary, portalTo, setOverlayHost }
}
