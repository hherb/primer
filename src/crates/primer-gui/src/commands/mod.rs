//! Tauri commands exposed to the frontend.
//!
//! Each sub-module groups commands by the part of the app they
//! manage. The free function [`register`] mounts every command on a
//! Tauri builder in one place so `main.rs` doesn't accumulate one
//! `.invoke_handler` line per new command.

pub mod session;
pub mod settings;

use tauri::Wry;

/// Register every command from every sub-module on a Tauri builder.
///
/// Adding a new command means appending it inside `tauri::generate_handler!`
/// — the function signature stays generic so `main.rs` doesn't need to
/// import the per-module command symbols.
pub fn register(builder: tauri::Builder<Wry>) -> tauri::Builder<Wry> {
    builder.invoke_handler(tauri::generate_handler![
        settings::get_settings,
        settings::update_settings,
        session::start_session,
        session::close_session,
        session::current_session_info,
        session::send_message,
        session::get_turn_signals,
        session::get_learner_state,
        session::list_session_turns,
    ])
}
