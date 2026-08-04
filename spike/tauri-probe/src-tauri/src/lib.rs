//! Tauri probe for `tauri-apps/tauri#13919`.
//!
//! One process, not two: the hook is installed **inside** the Tauri process,
//! which is what the upstream report actually did. The single question this
//! exists to answer is whether a `WH_KEYBOARD_LL` hook still fires while
//! Tauri's event loop is running, in three window states.
//!
//! The upstream issue is closed — opened and closed the same day (2025-07-30) —
//! and the keys that failed were OS-reserved system combinations (Win, Alt+Tab,
//! Ctrl+Shift+Esc, Win+L, Win+D, Win+R) while the Tauri window had focus. Bare
//! Shift was never part of the report and is not an OS-reserved combination. So
//! this probe logs those system keys too, as a control: that distinguishes "our
//! case is fine and theirs was real" from "the issue no longer reproduces".
//!
//! Set `COPPER_DEVICE_EVENT_FILTER` to `never`, `unfocused` or `always` to
//! compare `Builder::device_event_filter` settings without recompiling. That is
//! the setting a Tauri maintainer pointed at, and which the reporter confirmed
//! resolved their problem.

use std::collections::BTreeSet;
use std::sync::mpsc;
use std::thread;

use capture::foreground::Foreground;
use capture::hook::{self, DoubleTapConfig, RawKey, TriggerKey};
use serde::Serialize;
use tauri::{DeviceEventFilter, Emitter, Manager};

const VK_TAB: u32 = 0x09;
const VK_ESCAPE: u32 = 0x1B;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_D: u32 = 0x44;
const VK_L: u32 = 0x4C;
const VK_R: u32 = 0x52;

#[derive(Clone, Serialize)]
struct TriggerEvent {
    at: String,
    side: String,
    injected: bool,
    foreground_process: String,
    foreground_title: String,
    /// True when the probe's own window was the foreground window. This is the
    /// distinction the whole probe turns on.
    probe_focused: bool,
}

#[derive(Clone, Serialize)]
struct SystemKeyEvent {
    at: String,
    combination: String,
    injected: bool,
    foreground_process: String,
}

/// Hide the window for a few seconds and bring it back on its own.
///
/// The third test state is "probe window hidden", and a window hidden with no
/// way back would strand the tester.
#[tauri::command]
fn hide_for(app: tauri::AppHandle, seconds: u64) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.hide();
    thread::spawn(move || {
        thread::sleep(std::time::Duration::from_secs(seconds));
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    });
}

#[tauri::command]
fn device_event_filter_setting() -> String {
    std::env::var("COPPER_DEVICE_EVENT_FILTER").unwrap_or_else(|_| "unfocused (Tauri default)".into())
}

/// Recognise the OS-reserved combinations from the upstream report.
///
/// Tracked from the raw event stream rather than `GetAsyncKeyState`, so the
/// injected flag travels with the observation.
struct SystemKeyWatcher {
    down: BTreeSet<u32>,
}

impl SystemKeyWatcher {
    fn new() -> Self {
        Self {
            down: BTreeSet::new(),
        }
    }

    fn observe(&mut self, key: &RawKey) -> Option<&'static str> {
        if key.is_up {
            self.down.remove(&key.vk);
            return None;
        }
        // Auto-repeat would otherwise log the same chord dozens of times.
        let already_down = !self.down.insert(key.vk);
        if already_down {
            return None;
        }

        let alt = self.held(hook::VK_MENU, hook::VK_LMENU, hook::VK_RMENU);
        let ctrl = self.held(hook::VK_CONTROL, hook::VK_LCONTROL, hook::VK_RCONTROL);
        let shift = self.held(hook::VK_SHIFT, hook::VK_LSHIFT, hook::VK_RSHIFT);
        let win = self.down.contains(&VK_LWIN) || self.down.contains(&VK_RWIN);

        Some(match key.vk {
            VK_LWIN | VK_RWIN => "Win",
            VK_TAB if alt => "Alt+Tab",
            VK_ESCAPE if ctrl && shift => "Ctrl+Shift+Esc",
            VK_L if win => "Win+L",
            VK_D if win => "Win+D",
            VK_R if win => "Win+R",
            _ => return None,
        })
    }

    fn held(&self, generic: u32, left: u32, right: u32) -> bool {
        self.down.contains(&generic) || self.down.contains(&left) || self.down.contains(&right)
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let requested = std::env::var("COPPER_DEVICE_EVENT_FILTER").unwrap_or_default();
    let filter = match requested.to_ascii_lowercase().as_str() {
        "never" => DeviceEventFilter::Never,
        "always" => DeviceEventFilter::Always,
        _ => DeviceEventFilter::Unfocused,
    };
    println!("device_event_filter = {filter:?} (COPPER_DEVICE_EVENT_FILTER={requested:?})");

    tauri::Builder::default()
        .device_event_filter(filter)
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![hide_for, device_event_filter_setting])
        .setup(|app| {
            let (trigger_tx, trigger_rx) = mpsc::channel();
            let (raw_tx, raw_rx) = mpsc::channel();

            let handle = hook::install_with_raw(
                TriggerKey::SHIFT,
                DoubleTapConfig::default(),
                trigger_tx,
                Some(raw_tx),
            )?;
            println!("WH_KEYBOARD_LL installed on thread {}", handle.thread_id());

            // Retain the handle for the app's lifetime. A handle bound to a
            // local in `setup()` drops at the end of the function and uninstalls
            // the hook immediately, which presents as "the hook does not work
            // under Tauri" — a false negative on the one question this probe
            // exists to answer.
            app.manage(handle);

            let probe_pid = capture::foreground::our_pid();

            let emitter = app.handle().clone();
            thread::spawn(move || {
                while let Ok(trigger) = trigger_rx.recv() {
                    let fg = Foreground::current();
                    let event = TriggerEvent {
                        at: capture::findings::iso8601_utc_now(),
                        side: trigger.side.to_string(),
                        injected: trigger.injected,
                        foreground_process: fg
                            .as_ref()
                            .map(|f| f.process.clone())
                            .unwrap_or_else(|| "<none>".into()),
                        foreground_title: fg
                            .as_ref()
                            .map(|f| f.title.clone())
                            .unwrap_or_default(),
                        probe_focused: fg.as_ref().is_some_and(|f| f.pid == probe_pid),
                    };
                    println!(
                        "TRIGGER  side={} injected={} foreground={} probe_focused={}",
                        event.side,
                        event.injected,
                        event.foreground_process,
                        event.probe_focused
                    );
                    let _ = emitter.emit("copper://trigger", event);
                }
            });

            let emitter = app.handle().clone();
            thread::spawn(move || {
                let mut watcher = SystemKeyWatcher::new();
                while let Ok(key) = raw_rx.recv() {
                    let Some(combination) = watcher.observe(&key) else {
                        continue;
                    };
                    let fg = Foreground::current();
                    let event = SystemKeyEvent {
                        at: capture::findings::iso8601_utc_now(),
                        combination: combination.to_owned(),
                        injected: key.injected,
                        foreground_process: fg
                            .map(|f| f.process)
                            .unwrap_or_else(|| "<none>".into()),
                    };
                    println!(
                        "SYSTEMKEY {} injected={} foreground={}",
                        event.combination, event.injected, event.foreground_process
                    );
                    let _ = emitter.emit("copper://system-key", event);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
