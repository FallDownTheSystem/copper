# Copper

A note and prompt capture panel for Windows. Summon it with a double-tap, capture selected text from any app, and file everything into spaces and sections.

Built with Tauri 2, Vue, and Tailwind CSS.

## Features

- **Double-tap summon** — a global shortcut slides the panel over whatever you're working in; press it again to hide. The panel stays always-on-top and out of your way.
- **Capture from anywhere** — grab the selected text in any app and drop it straight into a note, with an optional toast so you know the capture landed.
- **Spaces and sections** — organize notes into spaces, each with collapsible sections. Move notes between them from the context menu or the keyboard.
- **Markdown notes** — notes render Markdown, and links unfurl into preview cards.
- **Attachments** — drag or paste files, view images, and select attachments to copy images, files, or paths.
- **Search, filter, sort** — full-text search across a space, a done/todo filter, and sort modes. View state is remembered per list across restarts.
- **Keyboard-first** — every row is a Tab stop, actions have chords, and the summon shortcut is recordable in settings. A built-in reference lists them all.
- **Sharing between machines** — sync a space to another machine through an end-to-end encrypted relay.
- **Command-line access** — a bundled `copper` CLI reads and edits spaces from a terminal; the app picks up CLI edits within about a second.
- **Windows-native** — light/dark themes with adjustable translucency, a system tray, optional start-with-Windows, and a built-in updater.

## Copy notes and attachments

- Right-click a note and choose **Copy with attachments** to copy its text and local attachment paths.
- Click an attachment to select it. Use **Ctrl+click** to toggle items or **Shift+click** to select a range.
- Drag any selected attachment to another application to send all selected files. Keep the mouse button down while Copper prepares the files. Press **Escape** to cancel.
- Double-click an attachment to open it. **Enter** and **Space** also open the focused attachment.
- Press **Ctrl+C** or use the copy bar. One supported image copies as pixels; other selections copy as files.
- Choose **Copy as file** to preserve an image's original format or animation.
- Use **Copy paths** for terminals and applications that do not accept native file paste.
- **Copy**, **Copy as list**, and **Copy as Markdown** on notes remain text-only.

Each attachment is one file, stored under its own name in the `.copper.assets` folder beside the space. Drag-out, **Copy paths**, and file copies all point at that stored file. Two attachments with the same name get Explorer's ` (2)` suffix. Drag-out permits copying, not moving. These files require local file access; Copper does not upload them to remote sessions.

Some applications accept only one file from a clipboard paste but accept multiple files through drag-and-drop. Use drag-out for those applications. A drop back into Copper attaches a second copy.

Attachments made by versions before 0.2.16 keep their older hash names. Re-attach a file to store it under its name. Versions before 0.2.16 also left a copy of every copied or dragged attachment in `%LOCALAPPDATA%\io.github.falldownthesystem.copper\attachment-copies`. Copper no longer writes there, and you can remove that directory once no application needs those paths.

## Credits

- Copper is a Windows clone of [shadcn's Copper](https://shadcn.com/copper), which is the original idea and design this project recreates.
- Some Windows-centric ideas come from [Carbon](https://github.com/Himanshu-Singh-Chauhan/Carbon-ShadCN-Copper-for-windows), another Windows take on Copper.

## Development

```
pnpm install
pnpm tauri dev
```
