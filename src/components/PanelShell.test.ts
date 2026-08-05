import { mount } from '@vue/test-utils'
import axe from 'axe-core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'

import PanelShell from './PanelShell.vue'
import type { Space, StoreStatus } from '@/composables/useSpace'

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), openUrl: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: mocks.openUrl }))

const SPACE: Space = {
	id: 'spc_1',
	name: 'development',
	activeSection: 'sec_a',
	sections: [
		{ id: 'sec_a', name: 'Research', order: 0 },
		{ id: 'sec_b', name: 'Inbox', order: 1 },
	],
	notes: [
		{
			id: 'nte_1',
			section: 'sec_a',
			order: 0,
			done: false,
			body: 'first note',
			created: '2026-08-05T00:00:00Z',
			updated: '2026-08-05T00:00:00Z',
		},
		{
			id: 'nte_2',
			section: 'sec_a',
			order: 1,
			done: true,
			body: 'second note',
			created: '2026-08-05T00:00:00Z',
			updated: '2026-08-05T00:00:00Z',
		},
	],
}

const STATUS: StoreStatus = {
	path: 'C:\\notes.copper',
	errored: false,
	watching: true,
	canUndo: false,
	canRedo: false,
	startupNotice: null,
}

beforeEach(() => {
	vi.resetModules()
	mocks.invoke.mockReset()
	mocks.invoke.mockImplementation(async (command: string) => {
		if (command === 'get_active_space') return SPACE
		if (command === 'get_status') return STATUS
		if (command === 'get_settings') {
			return { recents: [], activeSpace: 0, panelPosition: null, shortcuts: {}, theme: 'system' }
		}
		throw { kind: 'invalid', message: command }
	})
})

afterEach(() => {
	document.body.innerHTML = ''
})

async function mountPanel() {
	const wrapper = mount(PanelShell, { attachTo: document.body })
	// Let the mount pull, reconciliation and the post-nextTick restore settle.
	for (let i = 0; i < 6; i++) await new Promise((resolve) => setTimeout(resolve, 0))
	return wrapper
}

describe('the grid structure', () => {
	it('is one grid spanning every section, with headers as rows', async () => {
		const wrapper = await mountPanel()

		// One composite widget, not one per section: a Shift range has to extend
		// across section boundaries.
		expect(wrapper.findAll('[role="grid"]')).toHaveLength(1)
		expect(wrapper.findAll('[role="rowgroup"]')).toHaveLength(2)

		// `grid` may own only row/rowgroup and `rowgroup` only row, so an <h2>
		// between rowgroups would violate aria-required-children.
		for (const rowgroup of wrapper.findAll('[role="rowgroup"]')) {
			for (const child of rowgroup.element.children) {
				expect(child.getAttribute('role')).toBe('row')
			}
		}

		for (const row of wrapper.findAll('[role="row"]')) {
			expect(row.element.children).toHaveLength(1)
			expect(row.element.children[0]?.getAttribute('role')).toBe('gridcell')
		}
	})

	it('labels each rowgroup by its section heading', async () => {
		const wrapper = await mountPanel()

		for (const rowgroup of wrapper.findAll('[role="rowgroup"]')) {
			const id = rowgroup.attributes('aria-labelledby')
			expect(id).toBeTruthy()
			expect(wrapper.find(`#${id}`).exists()).toBe(true)
		}
	})

	it('marks note rows selectable and header rows not', async () => {
		const wrapper = await mountPanel()

		const noteRows = wrapper.findAll('[data-row-id^="n:"]')
		expect(noteRows).toHaveLength(2)
		for (const row of noteRows) expect(row.attributes('aria-selected')).toBeDefined()

		for (const row of wrapper.findAll('[data-row-id^="s:"]')) {
			expect(row.attributes('aria-selected')).toBeUndefined()
		}
	})
})

describe('the roving tabindex', () => {
	it('leaves exactly one row and no descendant in the tab order', async () => {
		const wrapper = await mountPanel()

		const rows = wrapper.findAll('[data-row-id]')
		const tabbable = rows.filter((row) => row.attributes('tabindex') === '0')
		expect(tabbable).toHaveLength(1)

		// The one-Tab-stop claim only holds if every interactive descendant is out
		// of the tab order too.
		for (const button of wrapper.find('[role="grid"]').findAll('button')) {
			expect(button.attributes('tabindex')).toBe('-1')
		}
	})
})

describe('the composer', () => {
	it('reads its placeholder from the active space name', async () => {
		const wrapper = await mountPanel()

		expect(wrapper.find('#composer').attributes('placeholder')).toBe(
			'Add a note or a prompt (development)',
		)
	})

	it('is labelled and describes its own key bindings', async () => {
		const wrapper = await mountPanel()
		const composer = wrapper.find('#composer')

		expect(wrapper.find('label[for="composer"]').exists()).toBe(true)
		const describedBy = composer.attributes('aria-describedby')
		expect(wrapper.find(`#${describedBy}`).text()).toContain('Enter to add')
	})
})

describe('live regions', () => {
	it('pre-renders both, empty, so a later text change actually announces', async () => {
		const wrapper = await mountPanel()

		// Injecting the element and its text together does not announce.
		expect(wrapper.find('[role="alert"]').exists()).toBe(true)
		expect(wrapper.find('[role="status"]').exists()).toBe(true)
		expect(wrapper.find('[role="alert"]').text()).toBe('')
	})
})

describe('axe', () => {
	it('reports no violations', async () => {
		await mountPanel()

		const results = await axe.run(document.body, {
			// Colour contrast needs a real layout and paint; it is verified by hand
			// over a black and a white desktop, because translucency shifts every
			// ratio with whatever is behind the panel.
			rules: { 'color-contrast': { enabled: false } },
		})

		expect(
			results.violations.map((violation) => `${violation.id}: ${violation.nodes.length} node(s)`),
		).toEqual([])
	}, 30_000)
})
