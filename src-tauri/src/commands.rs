//! The one invoke handler.
//!
//! Command *wrappers* still live next to the module they serve — the store's
//! twenty in `store/commands.rs`, the editor's four in `editor.rs`, the
//! clipboard's one in `clipboard.rs`. Only the registration is here, because
//! Tauri accepts exactly one `invoke_handler` and `generate_handler!` consumes
//! the `Invoke` it is given, so two handlers cannot be chained.
//!
//! None of these need capability entries: app-defined commands are allowed for
//! every window by default, and `build.removeUnusedCommands` only prunes
//! commands opted into the ACL through `AppManifest::commands` in `build.rs`,
//! which this project does not do.

use crate::{autostart, clipboard, editor, panel, shortcuts, spaces, store, theme};

pub fn handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
	tauri::generate_handler![
		store::commands::get_settings,
		store::commands::update_settings,
		store::commands::get_status,
		store::commands::get_active_space,
		store::commands::open_space,
		store::commands::create_space,
		store::commands::add_note,
		store::commands::submit_entry,
		store::commands::edit_note,
		store::commands::set_notes_done,
		store::commands::delete_notes,
		store::commands::reorder_note,
		store::commands::move_notes,
		store::commands::merge_notes,
		store::commands::add_section,
		store::commands::rename_section,
		store::commands::delete_section,
		store::commands::reorder_section,
		store::commands::set_active_section,
		store::commands::undo,
		store::commands::redo,
		clipboard::clipboard_write_text,
		editor::editor_handoffs,
		editor::editor_open_note,
		editor::editor_stop_handoff,
		editor::editor_reconcile,
		spaces::list_recents,
		spaces::refresh_recents,
		spaces::activate_space,
		spaces::pick_and_open_space,
		spaces::create_space_interactive,
		spaces::remove_recent,
		theme::set_theme_preference,
		shortcuts::get_shortcut_state,
		shortcuts::set_summon_shortcut,
		shortcuts::set_capture_trigger,
		shortcuts::begin_shortcut_recording,
		shortcuts::commit_shortcut_recording,
		shortcuts::cancel_shortcut_recording,
		autostart::get_autostart_enabled,
		autostart::set_autostart_enabled,
		panel::hide_panel
	]
}
