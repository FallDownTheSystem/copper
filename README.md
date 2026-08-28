# Copper

A note and prompt capture panel for Windows. Summon it with a double-tap, capture selected text from any app, and file everything into spaces and sections.

Built with Tauri 2, Vue, and Tailwind CSS.

## Features

- **Double-tap summon** — a global shortcut slides the panel over whatever you're working in; press it again to hide. The panel stays always-on-top and out of your way.
- **Capture from anywhere** — grab the selected text in any app and drop it straight into a note, with an optional toast so you know the capture landed.
- **Spaces and sections** — organize notes into spaces, each with collapsible sections. Move notes between them from the context menu or the keyboard.
- **Markdown notes** — notes render Markdown, and links unfurl into preview cards.
- **Attachments** — drag files onto the panel or paste them; images open in a built-in viewer.
- **Search, filter, sort** — full-text search across a space, a done/todo filter, and sort modes. View state is remembered per list across restarts.
- **Keyboard-first** — every row is a Tab stop, actions have chords, and the summon shortcut is recordable in settings. A built-in reference lists them all.
- **Sharing between machines** — sync a space to another machine through an end-to-end encrypted relay.
- **Command-line access** — a bundled `copper` CLI reads and edits spaces from a terminal; the app picks up CLI edits within about a second.
- **Windows-native** — light/dark themes with adjustable translucency, a system tray, optional start-with-Windows, and a built-in updater.

## Credits

- Copper is a Windows clone of [shadcn's Copper](https://shadcn.com/copper), which is the original idea and design this project recreates.
- Some Windows-centric ideas come from [Carbon](https://github.com/Himanshu-Singh-Chauhan/Carbon-ShadCN-Copper-for-windows), another Windows take on Copper.

## Development

```
pnpm install
pnpm tauri dev
```
