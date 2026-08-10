# Screenshot pipeline

Generates the readme gallery: 4K screenshots of the real Copper frontend,
captured in headless Chrome. Nothing is mocked visually — the harness boots the
production `src/main.ts` with the Tauri bridge stubbed, so every pixel is the
shipping Vue code, CSS, and fonts.

## Parts

| File | Role |
| --- | --- |
| `/shots.html` (repo root) | Harness page. Fake desktop: wallpaper, window shadow, 1px window border, and a fake Acrylic layer. Hides the Vue DevTools overlay. Contains the app in a 440×760 `.window` whose `transform` keeps the app's `position: fixed` overlays inside it. |
| `/shots.ts` (repo root) | Stubs `window.__TAURI_INTERNALS__` with every command the frontend invokes, fills a demo space with every content type, then imports the real `./src/main`. Link-preview images and attachments are painted at runtime with a canvas — no fetches, no binary assets. |
| `stage-walls.mjs` | Copies the Windows wallpapers from `C:\Windows\Web` into `/shots-walls/` (git-ignored). The images are Microsoft's, so they stay out of the repo. |
| `capture.mjs` | Raw-CDP driver. Launches headless Chrome, navigates shot URLs, plays real input events, saves PNGs. No npm dependencies. |
| `shots-final.json` | The canonical shot list — reproduces the full gallery. |

## Procedure

```sh
pnpm dev                                   # Vite on http://localhost:1420
node scripts/shots/stage-walls.mjs         # once per checkout
node scripts/shots/capture.mjs scripts/shots/shots-final.json shots-gallery
```

Each shot takes a few seconds. Output lands in `shots-gallery/` (git-ignored).

## Harness URL parameters

`shots.html` reads these from the query string:

- `mode` — `desktop` (wallpaper + centered window, the default) or `plain`
  (panel fills the viewport; pair with the capture option `"transparent": true`
  for an alpha PNG of just the panel).
- `wall` — `bloom-light`, `bloom-dark`, `theme-a`…`theme-d`, `spot-1`,
  `spot-2`, or the CSS-painted `mesh-copper` / `mesh-graphite`.
- `theme` — `light` or `dark`. Also pre-seeds `localStorage['color-scheme']`.
- `acrylic=1` — the translucent setting; the harness paints a blurred wallpaper
  stand-in for the native Acrylic the app cannot have in a browser.
- `accent`, `neutral`, `vibrancy` — palette dials (`lib/palette.ts` names).
- `section` — active section id (`sec_inbox`, `sec_copper`, `sec_read`).
- `created=1` — show note timestamps. `empty=1` — empty space (EmptyState).
- `update=1` — `check_for_update` returns a fake 0.4.0.
- `undo=1` — status reports undo available. `w`/`h` — panel size.

## Shot list format

Each entry in the JSON: `name` (output filename), `url`, optional
`viewport: [w, h]` (default 1920×1080), `scale` (device pixel ratio, default 2 —
so the default output is 3840×2160), `transparent` (capture with alpha),
`settle` (ms to wait after load, default 1800), `after` (ms after the last
action, default 600), and `actions`:

- `{ "do": "click" | "dblclick" | "rightclick" | "hover" | "scrollto", "selector": "...", "text": "optional substring filter" }`
- `{ "do": "key", "key": "k+ctrl" }` · `{ "do": "type", "text": "..." }`
- `{ "do": "eval", "js": "..." }` · `{ "do": "wait", "ms": 400 }`

Useful selectors: rows are `[data-row-id='n:nte_1']` (`s:` prefix for section
headings), the list scroller is `[data-scroll-region]`, the menu button is
`button[aria-label='More actions']`, the attach button is
`button[aria-label='Attach files']`.

## Gotchas the current setup already solves

Do not re-learn these:

- The attachment image viewer opens on **double**-click; a single click only
  selects the row (matches Explorer).
- Nothing checks for updates unasked — the updater shot must click
  "Check for updates" first.
- The list opens scrolled to the newest note; shots that want the top must set
  `scrollTop = 0`.
- The stub's command argument shapes were verified against the composables:
  `update_settings` takes `{ patch }`, the `set_*` window toggles take
  `{ enabled }`, list mutations take `{ ids, ... }`, `submit_entry` takes
  `{ body }`.
- Chromium refuses an alpha screenshot if anything paints the full viewport —
  which is why plain mode force-clears the body background.

## Cleanup before a release build

`shots.html` and `shots.ts` are inert in production builds (`index.html` is the
only Vite entry, and nothing imports `shots.ts`), and `shots-walls/` /
`shots-gallery/` live outside `public/`, so nothing here can leak into `dist/`.
Deleting the two root files anyway before a release keeps the shipped source
tree clean, but it is hygiene, not a correctness requirement.
