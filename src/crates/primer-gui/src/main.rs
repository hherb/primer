//! The Primer GUI — a Tauri desktop UI for testing and monitoring
//! the Socratic dialogue engine.
//!
//! Thin shim around [`primer_gui::run`]. Everything interesting lives
//! in the library so commands, state, and persistence can be
//! unit-tested without the Tauri WebView in the loop.

// On Windows, suppress the console-subsystem prompt when running the
// release build (the Tauri default — kept here so a future cargo run
// --release doesn't open a stray console window alongside the UI).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(e) = primer_gui::run() {
        eprintln!("primer-gui exited with error: {e}");
        std::process::exit(1);
    }
}
