/**
 * Which of the panel's two views is on screen, and which way the last change
 * went.
 *
 * Module scope, like `useSpace` and `useSelection`: the tray's `open-settings`
 * listener lives in `App.vue` and the `...` menu's entry lives four components
 * down, and both have to move the same view. A ref held inside a component
 * cannot be one of them.
 *
 * `direction` exists only so the transition can reverse — a view that arrives
 * from the right must leave to the right when it is dismissed, or the motion
 * says the opposite of what happened.
 */

export type View = 'list' | 'settings'
export type Direction = 'forward' | 'back'

const view = ref<View>('list')
const direction = ref<Direction>('forward')

function showSettings() {
	direction.value = 'forward'
	view.value = 'settings'
}

function showList() {
	direction.value = 'back'
	view.value = 'list'
}

export function useView() {
	return {
		view: readonly(view),
		direction: readonly(direction),
		showSettings,
		showList,
	}
}
