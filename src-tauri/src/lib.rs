mod session;

use std::sync::{Arc, Mutex};

use session::{
    NoopSessionSink, SessionConfig, SessionError, SessionId, SessionManager, SessionSnapshot,
};
use tauri::State;

type SharedSessionManager = Mutex<SessionManager>;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let sink = Arc::new(NoopSessionSink);
    tauri::Builder::default()
        .manage(Mutex::new(SessionManager::new(sink)))
        .invoke_handler(tauri::generate_handler![
            create_session,
            list_sessions,
            set_active_session,
            write_session,
            resize_session,
            close_session
        ])
        .run(tauri::generate_context!())
        .expect("failed to run termul");
}

#[tauri::command]
fn create_session(
    state: State<'_, SharedSessionManager>,
    config: SessionConfig,
) -> Result<SessionSnapshot, String> {
    with_manager(state, |manager| manager.create_session(config))
}

#[tauri::command]
fn list_sessions(state: State<'_, SharedSessionManager>) -> Result<Vec<SessionSnapshot>, String> {
    with_manager(state, |manager| manager.list_sessions())
}

#[tauri::command]
fn set_active_session(
    state: State<'_, SharedSessionManager>,
    session_id: SessionId,
) -> Result<SessionSnapshot, String> {
    with_manager(state, |manager| manager.set_active_session(session_id))
}

#[tauri::command]
fn write_session(
    state: State<'_, SharedSessionManager>,
    session_id: SessionId,
    input: String,
) -> Result<(), String> {
    with_manager(state, |manager| manager.write_session(session_id, input.as_bytes()))
}

#[tauri::command]
fn resize_session(
    state: State<'_, SharedSessionManager>,
    session_id: SessionId,
    cols: u16,
    rows: u16,
) -> Result<SessionSnapshot, String> {
    with_manager(state, |manager| manager.resize_session(session_id, cols, rows))
}

#[tauri::command]
fn close_session(
    state: State<'_, SharedSessionManager>,
    session_id: SessionId,
) -> Result<(), String> {
    with_manager(state, |manager| manager.close_session(session_id))
}

fn with_manager<T, F>(state: State<'_, SharedSessionManager>, action: F) -> Result<T, String>
where
    F: FnOnce(&mut SessionManager) -> Result<T, SessionError>,
{
    let mut manager = state
        .lock()
        .map_err(|_| "session manager lock poisoned".to_string())?;
    action(&mut manager).map_err(|err| err.to_string())
}
