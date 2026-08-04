// `withGlobalTauri: true` in tauri.conf.json means the API is on window.__TAURI__,
// so the probe needs no npm install and runs straight from `cargo run`.
const { invoke } = window.__TAURI__.core
const { listen } = window.__TAURI__.event

let body
let empty

function addRow({ time, kind, detail, foreground, probeFocused, injected }) {
	empty.style.display = 'none'
	const tr = document.createElement('tr')
	tr.className = kind === 'trigger' ? 'trigger' : 'systemkey'
	for (const value of [time.slice(11, 23), kind, detail, foreground, probeFocused, injected]) {
		const td = document.createElement('td')
		td.textContent = String(value)
		tr.appendChild(td)
	}
	body.prepend(tr)
}

window.addEventListener('DOMContentLoaded', () => {
	body = document.querySelector('#log tbody')
	empty = document.querySelector('.empty')

	listen('copper://trigger', (event) => {
		const p = event.payload
		addRow({
			time: p.at,
			kind: 'trigger',
			detail: `double-tap Shift (${p.side})`,
			foreground: `${p.foreground_process} — ${p.foreground_title}`.slice(0, 60),
			probeFocused: p.probe_focused ? 'yes' : 'no',
			injected: p.injected ? 'yes' : 'no',
		})
	})

	listen('copper://system-key', (event) => {
		const p = event.payload
		addRow({
			time: p.at,
			kind: 'system key',
			detail: p.combination,
			foreground: p.foreground_process,
			probeFocused: '',
			injected: p.injected ? 'yes' : 'no',
		})
	})

	document.querySelector('#hide').addEventListener('click', () => {
		invoke('hide_for', { seconds: 12 })
	})

	document.querySelector('#clear').addEventListener('click', () => {
		body.replaceChildren()
		empty.style.display = ''
	})

	invoke('device_event_filter_setting').then((value) => {
		document.querySelector('#filter').textContent = value
	})
})
