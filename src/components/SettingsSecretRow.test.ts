import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vite-plus/test'

import SettingsSecretRow from './SettingsSecretRow.vue'

/**
 * The write-only credential field.
 *
 * Three claims, and each of them is the kind that rots silently. The component
 * must have no way at all to render a stored secret — that is the property the
 * whole "no command reads a secret back" design rests on, and one added prop
 * would end it. Blurring an untouched field must emit nothing, because the field
 * is empty by construction and a blur-clears rule would wipe a stored credential
 * every time somebody tabbed past it. And **Clear** must emit `null` rather than
 * `''`, because Rust reads the two differently: absent leaves, `null` clears.
 */

function row(set = false) {
	return mount(SettingsSecretRow, {
		props: { set, label: 'Relay token', placeholder: 'Paste the relay token' },
	})
}

describe('SettingsSecretRow', () => {
	it('renders whether a value is stored and never the value', () => {
		expect(row(true).text()).toContain('Set')

		const empty = row(false)
		expect(empty.text()).toContain('Not set')
		expect(empty.get('input').element.value).toBe('')
	})

	/** The input starts empty on every mount, so there is no state in which a
	 *  stored secret could be shown. There is no prop that could supply one. */
	it('has no prop that could carry a value', () => {
		const wrapper = row(true)
		expect(Object.keys(wrapper.props())).not.toContain('value')
		expect(Object.keys(wrapper.props())).not.toContain('modelValue')
	})

	it('masks what is typed', () => {
		expect(row().get('input').attributes('type')).toBe('password')
	})

	it('emits the typed value on Enter, then empties the field', async () => {
		const wrapper = row()
		const input = wrapper.get('input')

		await input.setValue('  a-long-random-token  ')
		await input.trigger('keydown.enter')

		expect(wrapper.emitted('commit')).toEqual([['a-long-random-token']])
		// Not left in the DOM after it has been handed over: it is a credential and
		// it is one keystroke away from being retyped.
		expect(input.element.value).toBe('')
	})

	it('emits the typed value on blur', async () => {
		const wrapper = row()
		const input = wrapper.get('input')

		await input.setValue('typed')
		await input.trigger('blur')

		expect(wrapper.emitted('commit')).toEqual([['typed']])
	})

	/** The dirty flag's whole job. Without it, tabbing through the settings view
	 *  would clear a stored token nobody meant to touch. */
	it('emits nothing when blurred without an edit', async () => {
		const wrapper = row(true)
		await wrapper.get('input').trigger('blur')
		expect(wrapper.emitted('commit')).toBeUndefined()
	})

	/** An edit that ends up empty is an abandoned edit, not a request to clear.
	 *  Clearing has its own button. */
	it('emits nothing when an edit is erased before the blur', async () => {
		const wrapper = row(true)
		const input = wrapper.get('input')

		await input.setValue('half a token')
		await input.setValue('')
		await input.trigger('blur')

		expect(wrapper.emitted('commit')).toBeUndefined()
	})

	it('emits null on Clear, which is what Rust reads as "forget this"', async () => {
		const wrapper = row(true)
		const clear = wrapper.get('button')
		expect(clear.text()).toBe('Clear')

		await clear.trigger('click')
		expect(wrapper.emitted('commit')).toEqual([[null]])
	})

	it('offers no Clear when there is nothing stored', () => {
		expect(row(false).find('button').exists()).toBe(false)
	})

	/** A second blur after a commit must not re-send the value: the field is empty
	 *  and the flag was reset, so this is the untouched case again. */
	it('does not re-emit after a commit', async () => {
		const wrapper = row()
		const input = wrapper.get('input')

		await input.setValue('once')
		await input.trigger('keydown.enter')
		await input.trigger('blur')

		expect(wrapper.emitted('commit')).toHaveLength(1)
	})
})
