use pisper_terminal::{
    close_registry, close_terminal, create_terminal, resize_terminal, write_terminal,
};
pub use pisper_terminal::{
    shutdown, DesktopTerminalState, TerminalCreateInput, TerminalCreated, TerminalEvent,
    TerminalProfile,
};
use std::sync::Arc;
use tauri::{ipc::Channel, State, WebviewWindow};

fn ensure_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("The terminal is available only in the main desktop window.".into())
    }
}

#[tauri::command]
pub fn desktop_terminal_profiles(window: WebviewWindow) -> Result<Vec<TerminalProfile>, String> {
    ensure_main(&window)?;
    pisper_terminal::profiles()
}

#[tauri::command]
pub fn desktop_terminal_create(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    input: TerminalCreateInput,
    on_event: Channel<TerminalEvent>,
) -> Result<TerminalCreated, String> {
    ensure_main(&window)?;
    create_terminal(
        &state,
        input,
        Arc::new(move |event| on_event.send(event).map_err(|error| error.to_string())),
    )
}

#[tauri::command]
pub fn desktop_terminal_write(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    terminal_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    ensure_main(&window)?;
    write_terminal(&state, &terminal_id, &data)
}

#[tauri::command]
pub fn desktop_terminal_resize(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    ensure_main(&window)?;
    resize_terminal(&state, &terminal_id, cols, rows)
}

#[tauri::command]
pub fn desktop_terminal_close(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    terminal_id: String,
) -> Result<bool, String> {
    ensure_main(&window)?;
    close_terminal(&state, &terminal_id)
}

#[tauri::command]
pub fn desktop_terminal_close_all(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
) -> Result<usize, String> {
    ensure_main(&window)?;
    close_registry(&state, false)
}
