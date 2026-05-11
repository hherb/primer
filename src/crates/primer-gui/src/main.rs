//! The Primer GUI — a Tauri desktop UI for testing and monitoring
//! the Socratic dialogue engine.
//!
//! Step 2 (this commit) scaffolds an empty window. Step 3 wires the
//! session lifecycle commands; step 4 streams chat. See the plan at
//! `~/.claude/plans/we-need-a-basic-abstract-wigderson.md` for the full
//! roadmap.

// On Windows, suppress the console-subsystem prompt when running the
// release build (the Tauri default — kept here so a future cargo run
// --release doesn't open a stray console window alongside the UI).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running primer-gui");
}
