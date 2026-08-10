// Drives headless Chrome over raw CDP and captures PNGs of the shots harness.
// Usage: node capture.mjs <shots.json> <outDir>
import { spawn } from 'node:child_process'
import { mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe'
const PORT = 9333
const [, , shotsFile, outDir] = process.argv
const shots = JSON.parse(readFileSync(shotsFile, 'utf8'))
mkdirSync(outDir, { recursive: true })

const profile = join(process.env.TEMP ?? '/tmp', 'copper-shots-profile')
rmSync(profile, { recursive: true, force: true })

const chrome = spawn(CHROME, [
	`--remote-debugging-port=${PORT}`,
	`--user-data-dir=${profile}`,
	'--headless=new',
	'--no-first-run',
	'--disable-extensions',
	'--force-color-profile=srgb',
	'--force-device-scale-factor=1',
	'--window-size=800,600',
	'about:blank',
])
chrome.on('exit', (code) => console.log('chrome exited', code))

async function waitForChrome() {
	for (let i = 0; i < 60; i++) {
		try {
			const res = await fetch(`http://127.0.0.1:${PORT}/json/version`)
			if (res.ok) return
		} catch {}
		await new Promise((r) => setTimeout(r, 250))
	}
	throw new Error('chrome debug port never came up')
}

class Session {
	constructor(ws) {
		this.ws = ws
		this.id = 0
		this.pending = new Map()
		this.events = []
		ws.onmessage = (event) => {
			const msg = JSON.parse(event.data)
			if (msg.id !== undefined) {
				const p = this.pending.get(msg.id)
				if (p) {
					this.pending.delete(msg.id)
					msg.error ? p.reject(new Error(`${p.method}: ${msg.error.message}`)) : p.resolve(msg.result)
				}
			} else {
				this.events.push(msg)
				const w = this.waiters?.get(msg.method)
				if (w) { this.waiters.delete(msg.method); w(msg.params) }
			}
		}
		this.waiters = new Map()
	}
	send(method, params = {}) {
		const id = ++this.id
		this.ws.send(JSON.stringify({ id, method, params }))
		return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject, method }))
	}
	waitEvent(method, timeoutMs = 15000) {
		return new Promise((resolve, reject) => {
			this.waiters.set(method, resolve)
			setTimeout(() => {
				if (this.waiters.get(method) === resolve) {
					this.waiters.delete(method)
					reject(new Error(`timeout waiting for ${method}`))
				}
			}, timeoutMs)
		})
	}
	async eval(expression, awaitPromise = true) {
		const r = await this.send('Runtime.evaluate', {
			expression,
			returnByValue: true,
			awaitPromise,
		})
		if (r.exceptionDetails) throw new Error(`eval failed: ${JSON.stringify(r.exceptionDetails)}`)
		return r.result?.value
	}
}

async function openTab(url) {
	const res = await fetch(`http://127.0.0.1:${PORT}/json/new?${encodeURIComponent(url)}`, {
		method: 'PUT',
	})
	const info = await res.json()
	const ws = new WebSocket(info.webSocketDebuggerUrl)
	await new Promise((resolve, reject) => {
		ws.onopen = resolve
		ws.onerror = () => reject(new Error('ws failed'))
	})
	return { session: new Session(ws), targetId: info.id, ws }
}

async function closeTab(targetId) {
	await fetch(`http://127.0.0.1:${PORT}/json/close/${targetId}`)
}

async function center(session, selector, text) {
	const rect = await session.eval(
		`(() => {
		  let el
		  const all = [...document.querySelectorAll(${JSON.stringify(selector)})]
		  ${text ? `el = all.find((e) => e.textContent.trim().toLowerCase().includes(${JSON.stringify(text.toLowerCase())}))` : `el = all[0]`}
		  if (!el) return null; const r = el.getBoundingClientRect();
		  return { x: r.x + r.width / 2, y: r.y + r.height / 2 } })()`,
		false,
	)
	if (!rect) throw new Error(`selector not found: ${selector} ${text ?? ''}`)
	return rect
}

async function mouse(session, type, x, y, opts = {}) {
	await session.send('Input.dispatchMouseEvent', {
		type,
		x,
		y,
		button: opts.button ?? 'left',
		buttons: opts.buttons ?? (type === 'mousePressed' || type === 'mouseReleased' ? 1 : 0),
		clickCount: opts.clickCount ?? (type === 'mousePressed' || type === 'mouseReleased' ? 1 : 0),
		...opts,
	})
}

async function runAction(session, action) {
	switch (action.do) {
		case 'wait':
			await new Promise((r) => setTimeout(r, action.ms))
			break
		case 'eval':
			await session.eval(action.js)
			break
		case 'scrollto': {
			await session.eval(
				`(() => {
				  const all = [...document.querySelectorAll(${JSON.stringify(action.selector)})]
				  const el = ${action.text ? `all.find((e) => e.textContent.trim().toLowerCase().includes(${JSON.stringify(action.text.toLowerCase())}))` : `all[0]`}
				  if (el) el.scrollIntoView({ block: ${JSON.stringify(action.block ?? 'center')}, behavior: 'instant' })
				})()`,
				false,
			)
			await new Promise((r) => setTimeout(r, 250))
			break
		}
		case 'hover': {
			const { x, y } = await center(session, action.selector, action.text)
			await mouse(session, 'mouseMoved', x, y)
			break
		}
		case 'click': {
			const { x, y } = await center(session, action.selector, action.text)
			await mouse(session, 'mouseMoved', x, y)
			await mouse(session, 'mousePressed', x, y)
			await mouse(session, 'mouseReleased', x, y)
			break
		}
		case 'rightclick': {
			const { x, y } = await center(session, action.selector, action.text)
			await mouse(session, 'mouseMoved', x, y)
			await mouse(session, 'mousePressed', x, y, { button: 'right', buttons: 2 })
			await mouse(session, 'mouseReleased', x, y, { button: 'right', buttons: 2 })
			break
		}
		case 'dblclick': {
			const { x, y } = await center(session, action.selector, action.text)
			await mouse(session, 'mouseMoved', x, y)
			await mouse(session, 'mousePressed', x, y)
			await mouse(session, 'mouseReleased', x, y)
			await mouse(session, 'mousePressed', x, y, { clickCount: 2 })
			await mouse(session, 'mouseReleased', x, y, { clickCount: 2 })
			break
		}
		case 'key': {
			// action.key like 'Escape', 'Enter', 'F2', 'k+ctrl'
			const [key, ...mods] = action.key.split('+')
			const modifiers =
				(mods.includes('alt') ? 1 : 0) |
				(mods.includes('ctrl') ? 2 : 0) |
				(mods.includes('meta') ? 4 : 0) |
				(mods.includes('shift') ? 8 : 0)
			const keyMap = {
				Escape: { windowsVirtualKeyCode: 27, code: 'Escape' },
				Enter: { windowsVirtualKeyCode: 13, code: 'Enter', text: '\r' },
				F2: { windowsVirtualKeyCode: 113, code: 'F2' },
				Tab: { windowsVirtualKeyCode: 9, code: 'Tab' },
				ArrowDown: { windowsVirtualKeyCode: 40, code: 'ArrowDown' },
				ArrowUp: { windowsVirtualKeyCode: 38, code: 'ArrowUp' },
				'/': { windowsVirtualKeyCode: 191, code: 'Slash', text: '/' },
				'?': { windowsVirtualKeyCode: 191, code: 'Slash', text: '?' },
			}
			const base = keyMap[key] ?? {
				windowsVirtualKeyCode: key.toUpperCase().charCodeAt(0),
				code: `Key${key.toUpperCase()}`,
				text: modifiers === 0 || modifiers === 8 ? key : undefined,
			}
			await session.send('Input.dispatchKeyEvent', {
				type: 'keyDown',
				key,
				modifiers,
				...base,
			})
			await session.send('Input.dispatchKeyEvent', {
				type: 'keyUp',
				key,
				modifiers,
				windowsVirtualKeyCode: base.windowsVirtualKeyCode,
				code: base.code,
			})
			break
		}
		case 'type':
			for (const ch of action.text) {
				await session.send('Input.dispatchKeyEvent', { type: 'char', text: ch })
			}
			break
		default:
			throw new Error(`unknown action: ${action.do}`)
	}
}

async function capture(shot) {
	const width = shot.viewport?.[0] ?? 1920
	const height = shot.viewport?.[1] ?? 1080
	const dsf = shot.scale ?? 2
	const { session, targetId } = await openTab('about:blank')
	try {
		await session.send('Page.enable')
		await session.send('Runtime.enable')
		await session.send('Emulation.setDeviceMetricsOverride', {
			width,
			height,
			deviceScaleFactor: dsf,
			mobile: false,
		})
		if (shot.transparent) {
			await session.send('Emulation.setDefaultBackgroundColorOverride', {
				color: { r: 0, g: 0, b: 0, a: 0 },
			})
		}
		const load = session.waitEvent('Page.loadEventFired')
		await session.send('Page.navigate', { url: shot.url })
		await load
		// Settle: fonts, then give Vue/motion/shiki/previews time to land.
		await session.eval(`document.fonts.ready.then(() => undefined)`)
		await new Promise((r) => setTimeout(r, shot.settle ?? 1800))
		for (const action of shot.actions ?? []) {
			await runAction(session, action)
		}
		await new Promise((r) => setTimeout(r, shot.after ?? 600))
		const warnings = await session.eval(
			`(() => undefined)()`,
			false,
		)
		const img = await session.send('Page.captureScreenshot', {
			format: 'png',
			captureBeyondViewport: false,
		})
		writeFileSync(join(outDir, `${shot.name}.png`), Buffer.from(img.data, 'base64'))
		console.log(`captured ${shot.name}`)
	} catch (err) {
		console.error(`FAILED ${shot.name}:`, err.message)
	} finally {
		await closeTab(targetId)
	}
}

await waitForChrome()
for (const shot of shots) {
	await capture(shot)
}
chrome.kill()
process.exit(0)
