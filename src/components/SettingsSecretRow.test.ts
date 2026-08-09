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
 *
 * The two states are mutually exclusive: a stored value renders mask dots and
 * **Clear** with no input at all, an absent one renders the input and **Set**.
 */

function row(set = false) {
	return mount(SettingsSecretRow, {
		props: { set, label: 'Relay token', placeholder: 'Paste the relay token' },
	})
}

describe('SettingsSecretRow', () => {
	it('renders mask dots and no input while a value is stored', () => {
		const stored = row(true)
		expect(stored.find('input').exists()).toBe(false)
		expect(stored.text()).toContain('••••')
		expect(stored.text()).toContain('Relay token is set')
	})

	it('renders an empty input and no mask while nothing is stored', () => {
		const empty = row(false)
		expect(empty.get('input').element.value).toBe('')
		expect(empty.text()).not.toContain('••••')
	})

	/** The mask is a literal with a fixed length, so it cannot echo the stored
	 *  value's length — the one thing a mask could still leak. */
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

	it('emits the typed value when Set is pressed', async () => {
		const wrapper = row()
		const set = wrapper.get('button')
		expect(set.text()).toBe('Set')
		expect(set.attributes('disabled')).toBeDefined()

		await wrapper.get('input').setValue('a-token')
		expect(set.attributes('disabled')).toBeUndefined()

		await set.trigger('click')
		expect(wrapper.emitted('commit')).toEqual([['a-token']])
	})

	/** The dirty flag's whole job. Without it, tabbing through the settings view
	 *  would clear a stored token nobody meant to touch. */
	it('emits nothing when blurred without an edit', async () => {
		const wrapper = row(false)
		await wrapper.get('input').trigger('blur')
		expect(wrapper.emitted('commit')).toBeUndefined()
	})

	/** An edit that ends up empty is an abandoned edit, not a request to clear.
	 *  Clearing has its own button. */
	it('emits nothing when an edit is erased before the blur', async () => {
		const wrapper = row(false)
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
		const buttons = row(false).findAll('button')
		expect(buttons).toHaveLength(1)
		expect(buttons[0]!.text()).toBe('Set')
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
