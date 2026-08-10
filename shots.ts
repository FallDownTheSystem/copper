// Screenshot harness: boots the REAL app in a plain browser by stubbing
// `window.__TAURI_INTERNALS__`, the single funnel every `@tauri-apps/api` call
// goes through. Demo data is crafted to show every content type the panel
// renders. Not part of the app; deleted after the gallery is captured.

type Args = Record<string, unknown> | undefined

const params = new URLSearchParams(location.search)
const flag = (name: string) => params.get(name) === '1'

// --- demo document -----------------------------------------------------------

const day = (d: number, h = 9, m = 0) =>
	new Date(Date.UTC(2026, 7, d, h, m, 0)).toISOString()

type DemoNote = {
	id: string
	section: string
	order: number
	done: boolean
	body: string
	attachments?: unknown[]
	created: string
	updated: string
}

const notes: DemoNote[] = []
let order: Record<string, number> = {}
function note(
	section: string,
	body: string,
	opts: { done?: boolean; created?: string; attachments?: unknown[] } = {},
) {
	const at = (order[section] = (order[section] ?? -1) + 1)
	notes.push({
		id: `nte_${notes.length + 1}`,
		section,
		order: at,
		done: opts.done ?? false,
		body,
		...(opts.attachments ? { attachments: opts.attachments } : {}),
		created: opts.created ?? day(8, 10, 24),
		updated: opts.created ?? day(8, 10, 24),
	})
}

// Inbox — quick captures, the app's bread and butter.
note('sec_inbox', 'Call the landlord about the radiator — he said after four', {
	created: day(10, 8, 12),
})
note('sec_inbox', 'Book the Helsinki–Tallinn ferry for the 22nd', { created: day(9, 19, 3) })
note('sec_inbox', 'https://github.com/FallDownTheSystem/copper', { created: day(9, 14, 41) })
note('sec_inbox', 'Send the receipts to accounting@fastworks.io before Friday', {
	created: day(9, 9, 55),
})
note('sec_inbox', 'Groceries: eggs, oat milk, basil, parmesan, good bread', {
	done: true,
	created: day(8, 17, 30),
})
note('sec_inbox', 'Whiteboard from the sprint review — the middle column is the cut line', {
	created: day(10, 11, 5),
	attachments: [
		{
			id: 'att_wb',
			file: 'whiteboard.png',
			name: 'sprint-review-board.png',
			mime: 'image/png',
			bytes: 214_566,
			width: 1000,
			height: 700,
		},
	],
})
note('sec_inbox', 'Venue options for the offsite — leaning toward the lake one', {
	created: day(10, 9, 42),
	attachments: [
		{
			id: 'att_v1',
			file: 'venue-lake.png',
			name: 'venue-lakeside.png',
			mime: 'image/png',
			bytes: 158_902,
			width: 900,
			height: 600,
		},
		{
			id: 'att_v2',
			file: 'venue-city.png',
			name: 'venue-rooftop.png',
			mime: 'image/png',
			bytes: 171_339,
			width: 900,
			height: 600,
		},
	],
})
note('sec_inbox', 'Invoice from the print shop, file with the Q3 receipts', {
	created: day(9, 12, 8),
	attachments: [
		{
			id: 'att_pdf',
			file: 'invoice.pdf',
			name: 'print-shop-invoice.pdf',
			mime: 'application/pdf',
			bytes: 182_400,
		},
	],
})

// Copper — a project section with the technical content types.
note(
	'sec_copper',
	'**Ship 0.3**\n1. Write the changelog\n2. Tag `v0.3.0`\n3. Smoke-test the updater on the laptop',
	{ created: day(10, 7, 45) },
)
note(
	'sec_copper',
	'The corner seam was DWM’s radius, not ours — `--panel-radius` stays at 8px',
	{
		created: day(9, 21, 18),
		attachments: [
			{
				id: 'att_1',
				file: 'seam.png',
				name: 'corner-seam.png',
				mime: 'image/png',
				bytes: 48_213,
				width: 800,
				height: 500,
			},
		],
	},
)
note(
	'sec_copper',
	'Debounce for the resize writer:\n```rust\nfn debounced(ms: u64) -> impl FnMut() {\n    let mut last = Instant::now();\n    move || {\n        if last.elapsed() > Duration::from_millis(ms) {\n            last = Instant::now();\n        }\n    }\n}\n```',
	{ created: day(9, 16, 2) },
)
note(
	'sec_copper',
	'Build times after the mold split:\n\n| profile | before | after |\n| --- | --- | --- |\n| debug | 41 s | 12 s |\n| release | 3 m 10 s | 58 s |',
	{ created: day(8, 13, 27) },
)
note(
	'sec_copper',
	'Fix the white line on the focused row — Chromium keeps transitioning `outline-color`',
	{ done: true, created: day(7, 11, 9) },
)

// Reading — prose, headings, quotes, a previewed article.
note(
	'sec_read',
	'# Reading\n*Thinking in Systems* — Meadows\n*The Design of Everyday Things* — Norman\n\nFinish Meadows before the offsite',
	{ created: day(8, 20, 40) },
)
note('sec_read', 'https://interfacecraft.online/posts/quiet-software', {
	created: day(8, 8, 16),
})
note(
	'sec_read',
	'> A tool is quiet when the cost of checking it rounds to zero.\n\nKeep this as the bar for the panel: summon, glance, gone.',
	{ created: day(7, 22, 51) },
)

const SPACE = {
	id: 'spc_demo',
	name: 'Personal',
	activeSection: params.get('section') ?? 'sec_inbox',
	sections: [
		{ id: 'sec_inbox', name: 'Inbox', order: 0 },
		{ id: 'sec_copper', name: 'Copper', order: 1 },
		{ id: 'sec_read', name: 'Reading', order: 2 },
	],
	notes: flag('empty') ? [] : notes,
}
if (flag('empty')) SPACE.sections = [{ id: 'sec_inbox', name: 'Inbox', order: 0 }]

const STATUS = {
	path: 'C:\\Users\\sam\\Documents\\personal.copper',
	errored: false,
	watching: true,
	canUndo: flag('undo'),
	canRedo: false,
	startupNotice: null,
}

const SHORTCUTS = {
	capture: 'Shift Shift',
	summon: 'Ctrl+Shift+Space',
	defaults: { capture: 'Shift Shift', summon: 'Ctrl+Shift+Space' },
	summonRegistered: true,
	summonError: null,
	captureRegistered: true,
	captureError: null,
	captureFallback: null,
	summonFallback: null,
}

const SETTINGS: Record<string, unknown> = {
	recents: [
		'C:\\Users\\sam\\Documents\\personal.copper',
		'C:\\Users\\sam\\Documents\\work.copper',
	],
	activeSpace: 0,
	panelPosition: null,
	shortcuts: {},
	theme: params.get('theme') ?? 'light',
	sounds: false,
	motion: 'auto',
	insertionPoint: 'bottom',
	doubleClick: 'edit',
	alwaysOnTop: true,
	showCreated: flag('created'),
	captureNotifications: true,
	linkPreviews: true,
	translucent: flag('acrylic'),
	neutral: params.get('neutral') ?? 'warm',
	accent: params.get('accent') ?? 'copper',
	vibrancy: Number(params.get('vibrancy') ?? '1'),
	resizable: false,
	panelWidth: Number(params.get('w') ?? '440'),
	panelHeight: Number(params.get('h') ?? '760'),
}

const RECENTS = [
	{
		path: 'C:\\Users\\sam\\Documents\\personal.copper',
		displayPath: '~\\Documents\\personal.copper',
		key: 'personal',
		name: 'Personal',
		active: true,
		availability: { state: 'available' },
	},
	{
		path: 'C:\\Users\\sam\\Documents\\work.copper',
		displayPath: '~\\Documents\\work.copper',
		key: 'work',
		name: 'Work',
		active: false,
		availability: { state: 'available' },
	},
]

const SHARE_CONFIG = {
	enabled: true,
	relayUrl: 'https://relay.copper.example',
	role: 'first',
	tokenSet: true,
	secretSet: true,
	configured: true,
	lastError: null,
}

const UPDATE = flag('update')
	? {
			version: '0.4.0',
			currentVersion: '0.2.3',
			notes: 'Sections can be reordered by keyboard, and the composer keeps drafts per space.',
			date: '2026-08-10',
		}
	: null

// --- generated imagery -------------------------------------------------------
// Link-preview pictures and the attachment arrive as raw PNG bytes in the real
// app (`preview_image` / `attachment_thumb`), so the stub paints them with a
// canvas and hands back the encoded ArrayBuffer. No external assets.

function paint(
	w: number,
	h: number,
	draw: (g: CanvasRenderingContext2D, w: number, h: number) => void,
): Promise<ArrayBuffer> {
	const canvas = document.createElement('canvas')
	canvas.width = w
	canvas.height = h
	const g = canvas.getContext('2d')!
	draw(g, w, h)
	return new Promise((resolve) => {
		canvas.toBlob((blob) => {
			void blob!.arrayBuffer().then(resolve)
		}, 'image/png')
	})
}

function ghCard(): Promise<ArrayBuffer> {
	return paint(1200, 600, (g, w, h) => {
		const bg = g.createLinearGradient(0, 0, w, h)
		bg.addColorStop(0, '#1c1917')
		bg.addColorStop(1, '#292524')
		g.fillStyle = bg
		g.fillRect(0, 0, w, h)
		// faint grid
		g.strokeStyle = 'rgba(255,255,255,0.05)'
		g.lineWidth = 1
		for (let x = 0; x < w; x += 60) {
			g.beginPath(); g.moveTo(x, 0); g.lineTo(x, h); g.stroke()
		}
		for (let y = 0; y < h; y += 60) {
			g.beginPath(); g.moveTo(0, y); g.lineTo(w, y); g.stroke()
		}
		// copper coin
		const coin = g.createRadialGradient(w / 2 - 40, h / 2 - 80, 20, w / 2, h / 2 - 40, 190)
		coin.addColorStop(0, '#e8a06a')
		coin.addColorStop(0.6, '#b06a3c')
		coin.addColorStop(1, '#7c4526')
		g.fillStyle = coin
		g.beginPath()
		g.arc(w / 2, h / 2 - 40, 150, 0, Math.PI * 2)
		g.fill()
		g.fillStyle = 'rgba(255,244,235,0.92)'
		g.font = '600 96px Inter, system-ui, sans-serif'
		g.textAlign = 'center'
		g.textBaseline = 'middle'
		g.fillText('Cu', w / 2, h / 2 - 44)
		g.font = '500 44px Inter, system-ui, sans-serif'
		g.fillStyle = 'rgba(255,255,255,0.85)'
		g.fillText('copper', w / 2, h / 2 + 170)
	})
}

function articleCard(): Promise<ArrayBuffer> {
	return paint(1200, 630, (g, w, h) => {
		const bg = g.createLinearGradient(0, h, w, 0)
		bg.addColorStop(0, '#2d3a4a')
		bg.addColorStop(0.55, '#7a90a8')
		bg.addColorStop(1, '#e9d9c8')
		g.fillStyle = bg
		g.fillRect(0, 0, w, h)
		// dunes
		for (const [y0, amp, tone] of [
			[430, 60, 'rgba(38,48,62,0.55)'],
			[500, 40, 'rgba(30,38,50,0.7)'],
			[560, 26, 'rgba(22,28,38,0.85)'],
		] as const) {
			g.fillStyle = tone
			g.beginPath()
			g.moveTo(0, h)
			for (let x = 0; x <= w; x += 8) {
				g.lineTo(x, y0 + Math.sin(x / 160 + y0) * amp * 0.4 + Math.sin(x / 57) * 8)
			}
			g.lineTo(w, h)
			g.closePath()
			g.fill()
		}
		// sun
		g.fillStyle = 'rgba(255,236,214,0.9)'
		g.beginPath()
		g.arc(w * 0.72, 190, 58, 0, Math.PI * 2)
		g.fill()
	})
}

function seamShot(): Promise<ArrayBuffer> {
	return paint(800, 500, (g, w, h) => {
		const bg = g.createLinearGradient(0, 0, w, h)
		bg.addColorStop(0, '#3a4a63')
		bg.addColorStop(1, '#1d2433')
		g.fillStyle = bg
		g.fillRect(0, 0, w, h)
		// a window corner, zoomed: surface + rounded corner + a hot pink seam marker
		g.fillStyle = '#f6f4f2'
		g.beginPath()
		// @ts-expect-error roundRect exists in Chromium
		g.roundRect(140, 120, w, h, 24)
		g.fill()
		g.strokeStyle = '#ff2d78'
		g.lineWidth = 4
		g.beginPath()
		g.arc(164, 144, 34, Math.PI, Math.PI * 1.5)
		g.stroke()
		g.fillStyle = '#57534e'
		g.font = '500 26px Inter, system-ui, sans-serif'
		g.fillText('corner seam @ 200%', 190, 260)
	})
}

function whiteboardShot(): Promise<ArrayBuffer> {
	return paint(1000, 700, (g, w, h) => {
		g.fillStyle = '#fbfaf7'
		g.fillRect(0, 0, w, h)
		// three columns of marker boxes with a red cut line down the middle
		const marker = (x: number, y: number, bw: number, bh: number, tone: string) => {
			g.strokeStyle = tone
			g.lineWidth = 5
			g.lineJoin = 'round'
			g.strokeRect(x, y, bw, bh)
		}
		marker(60, 80, 230, 90, '#2563eb')
		marker(70, 220, 210, 70, '#2563eb')
		marker(60, 340, 240, 110, '#16a34a')
		marker(390, 90, 220, 80, '#d97706')
		marker(400, 230, 200, 90, '#d97706')
		marker(700, 100, 230, 100, '#16a34a')
		marker(710, 260, 210, 80, '#2563eb')
		marker(700, 400, 240, 90, '#dc2626')
		g.strokeStyle = '#dc2626'
		g.lineWidth = 7
		g.setLineDash([26, 18])
		g.beginPath()
		g.moveTo(345, 40)
		g.lineTo(355, h - 40)
		g.stroke()
		g.setLineDash([])
		// arrows between boxes
		g.strokeStyle = '#57534e'
		g.lineWidth = 4
		g.beginPath()
		g.moveTo(290, 125)
		g.quadraticCurveTo(340, 125, 388, 128)
		g.stroke()
		g.beginPath()
		g.moveTo(612, 130)
		g.quadraticCurveTo(660, 132, 698, 145)
		g.stroke()
	})
}

function venueLakeShot(): Promise<ArrayBuffer> {
	return paint(900, 600, (g, w, h) => {
		const sky = g.createLinearGradient(0, 0, 0, h * 0.55)
		sky.addColorStop(0, '#a7c7e7')
		sky.addColorStop(1, '#e8ded0')
		g.fillStyle = sky
		g.fillRect(0, 0, w, h * 0.55)
		const water = g.createLinearGradient(0, h * 0.55, 0, h)
		water.addColorStop(0, '#7da7c4')
		water.addColorStop(1, '#3f6b8a')
		g.fillStyle = water
		g.fillRect(0, h * 0.55, w, h * 0.45)
		// far shore + cabin
		g.fillStyle = '#5a7057'
		g.beginPath()
		g.moveTo(0, h * 0.55)
		g.lineTo(w * 0.45, h * 0.42)
		g.lineTo(w, h * 0.55)
		g.closePath()
		g.fill()
		g.fillStyle = '#8a5a3a'
		g.fillRect(w * 0.62, h * 0.47, 70, 44)
		g.fillStyle = '#4a3325'
		g.beginPath()
		g.moveTo(w * 0.62 - 8, h * 0.47)
		g.lineTo(w * 0.62 + 35, h * 0.41)
		g.lineTo(w * 0.62 + 78, h * 0.47)
		g.closePath()
		g.fill()
		// sun glint on the water
		g.fillStyle = 'rgba(255, 244, 214, 0.5)'
		for (let i = 0; i < 12; i++) {
			g.fillRect(w * 0.44 + (i % 3) * 14, h * 0.6 + i * 16, 60 - i * 3, 4)
		}
	})
}

function venueCityShot(): Promise<ArrayBuffer> {
	return paint(900, 600, (g, w, h) => {
		const dusk = g.createLinearGradient(0, 0, 0, h)
		dusk.addColorStop(0, '#3b2a52')
		dusk.addColorStop(0.55, '#a35a6e')
		dusk.addColorStop(0.8, '#e0985f')
		g.fillStyle = dusk
		g.fillRect(0, 0, w, h)
		// skyline
		g.fillStyle = '#241a30'
		const widths = [70, 50, 90, 60, 110, 45, 80, 65, 95, 55, 85]
		let x = -20
		for (const [i, bw] of widths.entries()) {
			const bh = 120 + ((i * 67) % 180)
			g.fillRect(x, h - bh, bw, bh)
			// lit windows
			g.fillStyle = 'rgba(255, 214, 140, 0.75)'
			for (let wy = h - bh + 14; wy < h - 20; wy += 26) {
				for (let wx = x + 10; wx < x + bw - 12; wx += 22) {
					if ((wx * wy) % 5 > 1) g.fillRect(wx, wy, 8, 11)
				}
			}
			g.fillStyle = '#241a30'
			x += bw + 14
		}
	})
}

const images: Record<string, () => Promise<ArrayBuffer>> = {
	'gh.png': ghCard,
	'ic.png': articleCard,
	'seam.png': seamShot,
	'whiteboard.png': whiteboardShot,
	'venue-lake.png': venueLakeShot,
	'venue-city.png': venueCityShot,
}

const PREVIEWS: Record<string, unknown> = {
	'https://github.com/FallDownTheSystem/copper': {
		url: 'https://github.com/FallDownTheSystem/copper',
		siteName: 'GitHub',
		title: 'FallDownTheSystem/copper',
		description:
			'A quick-capture notes panel for Windows. Summon it, drop a thought, get back to work.',
		image: 'gh.png',
	},
	'https://interfacecraft.online/posts/quiet-software': {
		url: 'https://interfacecraft.online/posts/quiet-software',
		siteName: 'Interface Craft',
		title: 'Quiet software',
		description:
			'Why the best tools feel like furniture: predictable, silent, and exactly where you left them.',
		image: 'ic.png',
	},
}

// --- the bridge --------------------------------------------------------------

function clone<T>(value: T): T {
	return JSON.parse(JSON.stringify(value)) as T
}

let callbackId = 0

async function invoke(cmd: string, args?: Args): Promise<unknown> {
	switch (cmd) {
		case 'get_active_space':
		case 'open_space':
			return clone(SPACE)
		case 'get_status':
			return clone(STATUS)
		case 'get_settings':
			return clone(SETTINGS)
		case 'update_settings': {
			// The real command takes { patch: Partial<Settings> }.
			Object.assign(SETTINGS, (args?.patch as object) ?? args ?? {})
			return clone(SETTINGS)
		}
		case 'set_theme_preference': {
			SETTINGS.theme = (args?.theme as string) ?? SETTINGS.theme
			return clone(SETTINGS)
		}
		case 'set_translucency': {
			SETTINGS.translucent = Boolean(args?.enabled)
			return clone(SETTINGS)
		}
		case 'set_always_on_top': {
			SETTINGS.alwaysOnTop = Boolean(args?.enabled)
			return clone(SETTINGS)
		}
		case 'set_resizable': {
			SETTINGS.resizable = Boolean(args?.enabled)
			return clone(SETTINGS)
		}
		case 'set_panel_size': {
			if (typeof args?.width === 'number') SETTINGS.panelWidth = args.width
			if (typeof args?.height === 'number') SETTINGS.panelHeight = args.height
			return clone(SETTINGS)
		}
		case 'get_shortcut_state':
			return clone(SHORTCUTS)
		case 'begin_shortcut_recording':
			return ++callbackId
		case 'commit_shortcut_recording':
		case 'cancel_shortcut_recording':
			return clone(SHORTCUTS)
		case 'get_autostart_enabled':
			return true
		case 'set_autostart_enabled':
			return Boolean(args?.enabled ?? args?.on)
		case 'get_share_config':
			return clone(SHARE_CONFIG)
		case 'set_share_config':
			return clone(SHARE_CONFIG)
		case 'generate_share_secret':
			return { secret: 'copper-demo-secret' }
		case 'share_test_relay':
			return { outcome: 'ok' }
		case 'share_send_notes':
			return { outcome: 'sent', count: 1 }
		case 'editor_handoffs':
			return []
		case 'editor_reconcile':
		case 'editor_stop_handoff':
			return null
		case 'list_recents':
			return clone(RECENTS)
		case 'refresh_recents':
		case 'remove_recent':
			return null
		case 'activate_space':
			return { changed: false, space: null }
		case 'pick_and_open_space':
		case 'create_space_interactive':
			return { changed: false, space: null }
		case 'get_app_version':
			return '0.2.3'
		case 'check_for_update':
			return clone(UPDATE)
		case 'install_update':
			return null
		case 'link_preview': {
			const url = String(args?.url ?? '')
			return clone(PREVIEWS[url] ?? null)
		}
		case 'preview_image':
		case 'attachment_thumb':
		case 'attachment_full': {
			const file = String(args?.file ?? '')
			const draw = images[file]
			return draw ? await draw() : new ArrayBuffer(0)
		}
		case 'attach_paste':
		case 'attach_paths':
			return []
		case 'attach_pick':
			// Feeds the composer's pending tray, so a shot can show it filled.
			return [
				{
					id: 'att_pick1',
					file: 'venue-lake.png',
					name: 'venue-lakeside.png',
					mime: 'image/png',
					bytes: 158_902,
					width: 900,
					height: 600,
				},
				{
					id: 'att_pick2',
					file: 'invoice.pdf',
					name: 'print-shop-invoice.pdf',
					mime: 'application/pdf',
					bytes: 182_400,
				},
			]
		case 'set_summon_shortcut':
		case 'set_capture_trigger':
			return clone(SHORTCUTS)
		case 'attachment_open':
		case 'attachment_reveal':
			return null
		case 'clipboard_write_text':
			return null
		case 'render_notes_markdown':
			return { text: '- demo', count: 1 }
		case 'submit_entry': {
			const text = String(args?.body ?? '')
			const section = SPACE.activeSection
			const at = SPACE.notes.filter((n) => n.section === section).length
			const created = new Date().toISOString()
			const id = `nte_${SPACE.notes.length + 100}`
			SPACE.notes.push({ id, section, order: at, done: false, body: text, created, updated: created })
			return { space: clone(SPACE), outcome: 'note', noteId: id, sectionId: section }
		}
		case 'add_note': {
			const body = String(args?.body ?? '')
			const section = String(args?.section ?? SPACE.activeSection)
			const at = SPACE.notes.filter((n) => n.section === section).length
			const created = new Date().toISOString()
			const id = `nte_${SPACE.notes.length + 100}`
			SPACE.notes.push({ id, section, order: at, done: false, body, created, updated: created })
			return { space: clone(SPACE), noteId: id }
		}
		case 'edit_note': {
			const target = SPACE.notes.find((n) => n.id === args?.id)
			if (target) {
				target.body = String(args?.body ?? target.body)
				target.updated = new Date().toISOString()
			}
			return clone(SPACE)
		}
		case 'set_active_section': {
			const id = (args?.sectionId ?? args?.id) as string | undefined
			if (id) SPACE.activeSection = id
			return clone(SPACE)
		}
		case 'set_notes_done': {
			const ids = (args?.ids as string[] | undefined) ?? []
			const done = args?.done as boolean | undefined
			for (const n of SPACE.notes) {
				if (ids.includes(n.id)) n.done = done ?? !n.done
			}
			return clone(SPACE)
		}
		case 'delete_notes': {
			const ids = (args?.ids as string[] | undefined) ?? []
			SPACE.notes = SPACE.notes.filter((n) => !ids.includes(n.id))
			return clone(SPACE)
		}
		case 'move_notes': {
			const ids = (args?.ids as string[] | undefined) ?? []
			const section = String(args?.section ?? SPACE.activeSection)
			for (const n of SPACE.notes) {
				if (ids.includes(n.id)) n.section = section
			}
			return clone(SPACE)
		}
		case 'merge_notes':
			return clone(SPACE)
		case 'reorder_note': {
			const target = SPACE.notes.find((n) => n.id === args?.id)
			if (target && typeof args?.index === 'number') target.order = args.index - 0.5
			SPACE.notes
				.filter((n) => n.section === target?.section)
				.sort((a, b) => a.order - b.order)
				.forEach((n, i) => (n.order = i))
			return clone(SPACE)
		}
		case 'add_section': {
			const name = String(args?.name ?? 'New section')
			const id = `sec_${SPACE.sections.length + 1}`
			SPACE.sections.push({ id, name, order: SPACE.sections.length })
			SPACE.activeSection = id
			return clone(SPACE)
		}
		case 'rename_section': {
			const section = SPACE.sections.find((s) => s.id === args?.id)
			if (section) section.name = String(args?.name ?? section.name)
			return clone(SPACE)
		}
		case 'delete_section': {
			SPACE.sections = SPACE.sections.filter((s) => s.id !== args?.id)
			SPACE.notes = SPACE.notes.filter((n) => n.section !== args?.id)
			if (!SPACE.sections.some((s) => s.id === SPACE.activeSection)) {
				SPACE.activeSection = SPACE.sections[0]?.id ?? ''
			}
			return clone(SPACE)
		}
		case 'reorder_section': {
			const at = SPACE.sections.findIndex((s) => s.id === args?.id)
			if (at !== -1 && typeof args?.index === 'number') {
				const [moved] = SPACE.sections.splice(at, 1)
				SPACE.sections.splice(args.index, 0, moved!)
				SPACE.sections.forEach((s, i) => (s.order = i))
			}
			return clone(SPACE)
		}
		case 'undo':
		case 'redo':
		case 'hide_panel':
			return null
		default:
			if (cmd.startsWith('plugin:')) return null
			console.warn('[shots] unhandled invoke:', cmd, args)
			return null
	}
}

;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
	invoke,
	transformCallback: () => ++callbackId,
	unregisterCallback: () => {},
	metadata: {
		currentWindow: { label: 'main' },
		currentWebview: { label: 'main', windowLabel: 'main' },
	},
	plugins: {},
}

// Booted only after the stub is in place — a static import would hoist above it.
void import('./src/main')
