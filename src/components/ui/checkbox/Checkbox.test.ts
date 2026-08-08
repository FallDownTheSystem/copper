import { enableAutoUnmount, mount } from '@vue/test-utils'
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import Checkbox from './Checkbox.vue'
import { clearReducedMotion, setReducedMotion } from '@/testing/matchMedia'

/**
 * The completion control, ported from the reference app. These pin the details
 * that make it look right rather than the animation itself — happy-dom has no
 * Web Animations API, and `motion-v` drives WAAPI, so the interpolation is not
 * observable here. What *is* observable is every decision taken around it, and
 * each of those exists because getting it wrong produces a specific visible
 * artefact.
 */

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
	emit: vi.fn(),
	listen: async () => () => {},
}))

// `motion-v` calls `element.animate`, which happy-dom does not implement. The
// animation is real product behaviour; only the environment is missing.
//
// Torn down again below. `restoreMocks` does not reach a plain assignment to a
// host prototype, so a stub left in place would outlive this file and hand every
// later suite in the worker a fake WAAPI they never asked for — which is exactly
// the kind of environment difference that makes one suite pass only when another
// ran first.
const elementPrototype = Element.prototype as unknown as Record<string, unknown>
const stubbedAnimate = elementPrototype.animate === undefined
if (stubbedAnimate) {
	elementPrototype.animate = () => ({
		playState: 'finished',
		finished: Promise.resolve(),
		cancel: () => {},
		play: () => {},
		pause: () => {},
		finish: () => {},
		commitStyles: () => {},
		addEventListener: () => {},
		removeEventListener: () => {},
	})
}

afterAll(() => {
	if (stubbedAnimate) Reflect.deleteProperty(elementPrototype, 'animate')
})

/**
 * Required, not hygiene. `useReducedMotion` is a `createSharedComposable`, so one
 * instance is built on first use and kept alive while any consumer holds it —
 * and it captures `matchMedia` at that moment. Without unmounting between cases
 * the first mounted checkbox pins the preference for the whole file, and
 * `setReducedMotion(true)` below would swap a global that nothing reads again.
 *
 * That sharing is still correct in the app: there is one real `matchMedia` there,
 * and the live change listener inside it keeps every consumer in step with the
 * OS. Only a suite that swaps the global mid-file has to care.
 */
enableAutoUnmount(afterEach)

beforeEach(() => {
	mocks.invoke.mockReset()
	mocks.invoke.mockRejectedValue({ kind: 'invalid', message: 'not wired in this suite' })
	setReducedMotion(false)
})

afterEach(() => {
	clearReducedMotion()
})

function checkPath(wrapper: ReturnType<typeof mount>) {
	// The only path — the indeterminate dash is deliberately not ported.
	return wrapper.find('path')
}

describe('the completion control', () => {
	/**
	 * Task-012 AC5's "no stray dot is painted at zero stroke length". A round cap
	 * on a zero-length stroke still paints a dot, which would leave a speck
	 * floating in an empty box — so the cap is only worn while there is a stroke
	 * to cap.
	 */
	it('wears no round cap while the box is empty', () => {
		const wrapper = mount(Checkbox, { props: { modelValue: false } })

		expect(checkPath(wrapper).attributes('stroke-linecap')).toBe('butt')
	})

	/**
	 * Task-012 AC6. The motion value is seeded from the state at mount, so a note
	 * that is already done on load shows its mark instead of drawing it in — a
	 * panel opening on twenty completed notes must not play twenty animations.
	 */
	it('shows an already-checked mark without animating it in', () => {
		const wrapper = mount(Checkbox, { props: { modelValue: true } })

		// A seeded length of 1 is a stroke, and a stroke wears its cap.
		expect(checkPath(wrapper).attributes('stroke-linecap')).toBe('round')
	})

	/**
	 * The `force-mount` half, and the reason task-004's objection to an entrance
	 * animation does not apply to this control: the mark is never mounted or
	 * unmounted, so there is no entrance to replay. The control it replaced used
	 * `v-if="note.done"` and did unmount.
	 */
	it('keeps the mark mounted when unchecked, so unchecking can retract it', () => {
		const wrapper = mount(Checkbox, { props: { modelValue: false } })

		expect(wrapper.find('svg').exists()).toBe(true)
		expect(wrapper.findAll('path')).toHaveLength(1)
	})

	/**
	 * The keyboard-repeat case task-004 recorded when it declined an entrance
	 * animation: the toggle is bound to Space and repeats. Nothing may mount,
	 * unmount or accumulate across a held key — a repeat has to retarget the
	 * element already on screen.
	 */
	it('mounts and unmounts nothing across a held Space repeat', async () => {
		const wrapper = mount(Checkbox, { props: { modelValue: false } })
		const svgBefore = wrapper.find('svg').element

		for (let i = 0; i < 12; i++) {
			// Cast because `setProps` infers from the raw component rather than from
			// the reka props the component actually forwards.
			await wrapper.setProps({ modelValue: i % 2 === 0 } as Record<string, unknown>)
		}

		expect(wrapper.find('svg').element).toBe(svgBefore)
		expect(wrapper.findAll('path')).toHaveLength(1)
		expect(wrapper.findAll('button')).toHaveLength(1)
	})

	/**
	 * Task-004 makes the grid a single Tab stop with a roving `tabindex`, so every
	 * interactive descendant of a row carries `tabindex="-1"` until F2 interaction
	 * mode. `motion.button` reaches the DOM through `as-child`, and this is the
	 * assertion that it merged onto the real control rather than wrapping it in a
	 * second focusable element.
	 */
	it('renders one button and lets the row govern its tabindex', () => {
		const wrapper = mount(Checkbox, {
			props: { modelValue: false },
			attrs: { tabindex: -1, 'aria-label': 'Mark as done' },
		})

		const buttons = wrapper.findAll('button')
		expect(buttons).toHaveLength(1)
		expect(buttons[0].attributes('tabindex')).toBe('-1')
		expect(buttons[0].attributes('aria-label')).toBe('Mark as done')
	})

	it('reports its state to the row through the toggle event', async () => {
		const wrapper = mount(Checkbox, { props: { modelValue: false } })

		await wrapper.find('button').trigger('click')

		expect(wrapper.emitted('update:modelValue')?.[0]).toEqual([true])
	})

	/**
	 * Task-012 AC7's press dip survives, but its old observable is deliberately
	 * gone: `will-change` used to ride on every enabled box, which at the list's
	 * design size was hundreds of permanent compositor layers against a ~10-layer
	 * budget, promoted for a 150ms dip motion-v's WAAPI animation promotes on its
	 * own. The invariant worth pinning is the absence — in every state, since a
	 * "some states may hold a layer" rule is how the leak comes back one branch
	 * at a time. (The dip's own gating lives in `pressState`, whose reduced-motion
	 * half `CheckboxIcon.test.ts` covers through the same composable.)
	 */
	it('never holds a permanent compositor layer, whatever its state', () => {
		for (const [props, reduced] of [
			[{ modelValue: false }, false],
			[{ modelValue: false }, true],
			[{ modelValue: false, disabled: true }, false],
		] as const) {
			setReducedMotion(reduced)
			const wrapper = mount(Checkbox, { props })

			expect(wrapper.find('button').attributes('style') ?? '').not.toContain('will-change')
		}
	})
})
