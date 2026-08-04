// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
	// Before run(), so it also covers panics raised during Tauri's own startup —
	// including the one Tauri raises when the setup() closure returns an error.
	// Without this a release build fails silently, because the line above leaves
	// the process with no console to print to.
	copper_lib::install_panic_dialog();
	copper_lib::run();
}
