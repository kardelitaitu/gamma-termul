use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

mod config;
mod paths;
mod session;

use config::Config;
use session::{
    DefaultSessionHandleFactory, SessionConfig, SessionError, SessionEvent, SessionEventSink,
    SessionId, SessionManager, SessionSnapshot, SESSION_EVENT_CHANNEL,
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow, WindowEvent};

type SharedConfig = Arc<Mutex<Config>>;
type SharedSessionManager = Mutex<SessionManager>;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = paths::verify();
    let config = Arc::new(Mutex::new(config::load()));

    tauri::Builder::default()
        .manage(Arc::clone(&config))
        .setup(|app| {
            let sink = Arc::new(TauriSessionSink::new(app.handle().clone()));
            app.manage(Mutex::new(SessionManager::new(
                sink,
                Box::new(DefaultSessionHandleFactory),
            )));

            let Some(window) = app.get_webview_window("main") else {
                return Ok(());
            };

            let config_state = app.state::<SharedConfig>().inner().clone();
            let config_snapshot = config_state
                .lock()
                .map_err(|_| "config lock poisoned".to_string())?
                .clone();

            {
                let session_state = app.state::<SharedSessionManager>();
                let mut manager = session_state
                    .lock()
                    .map_err(|_| "session manager lock poisoned".to_string())?;
                restore_sessions_from_config(&mut manager, &config_snapshot)?;
            }

            apply_window_config(&window, &config_snapshot);
            attach_window_persistence(&window, config_state);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_name,
            load_config,
            save_config,
            create_session,
            active_session,
            list_sessions,
            set_active_session,
            rename_session,
            write_session,
            resize_session,
            close_session
        ])
        .run(tauri::generate_context!())
        .expect("failed to run gamma-termul");
}

struct TauriSessionSink {
    app: AppHandle,
}

impl TauriSessionSink {
    fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl SessionEventSink for TauriSessionSink {
    fn emit(&self, event: SessionEvent) {
        if let Err(err) = self.app.emit(SESSION_EVENT_CHANNEL, event) {
            eprintln!("failed to emit session event: {err}");
        }
    }
}

#[tauri::command]
fn get_app_name() -> String {
    paths::exe_stem()
}

#[tauri::command]
fn load_config(state: State<'_, SharedConfig>) -> Result<Config, String> {
    let config = state
        .lock()
        .map_err(|_| "config lock poisoned".to_string())?
        .clone();
    Ok(config)
}

#[tauri::command]
fn save_config(
    session_state: State<'_, SharedSessionManager>,
    config_state: State<'_, SharedConfig>,
    config: Config,
) -> Result<(), String> {
    let manager = session_state
        .lock()
        .map_err(|_| "session manager lock poisoned".to_string())?;
    let mut guard = config_state
        .lock()
        .map_err(|_| "config lock poisoned".to_string())?;

    let mut merged = config;
    merged.window = guard.window.clone();
    refresh_tabs_from_manager(&manager, &mut merged)?;
    *guard = merged;
    config::save(&guard)
}

#[tauri::command]
fn active_session(
    state: State<'_, SharedSessionManager>,
) -> Result<Option<SessionSnapshot>, String> {
    with_manager(state, |manager| manager.active_session())
}

#[tauri::command]
fn create_session(
    session_state: State<'_, SharedSessionManager>,
    config_state: State<'_, SharedConfig>,
    config: SessionConfig,
) -> Result<SessionSnapshot, String> {
    let mut manager = session_state
        .lock()
        .map_err(|_| "session manager lock poisoned".to_string())?;
    let snapshot = manager
        .create_session(config)
        .map_err(|err| err.to_string())?;
    sync_tabs_config(&manager, &config_state)?;
    Ok(snapshot)
}

#[tauri::command]
fn list_sessions(state: State<'_, SharedSessionManager>) -> Result<Vec<SessionSnapshot>, String> {
    with_manager(state, |manager| manager.list_sessions())
}

#[tauri::command]
fn set_active_session(
    session_state: State<'_, SharedSessionManager>,
    config_state: State<'_, SharedConfig>,
    session_id: SessionId,
) -> Result<SessionSnapshot, String> {
    let mut manager = session_state
        .lock()
        .map_err(|_| "session manager lock poisoned".to_string())?;
    let snapshot = manager
        .set_active_session(session_id)
        .map_err(|err| err.to_string())?;
    sync_tabs_config(&manager, &config_state)?;
    Ok(snapshot)
}

#[tauri::command]
fn rename_session(
    session_state: State<'_, SharedSessionManager>,
    config_state: State<'_, SharedConfig>,
    session_id: SessionId,
    title: String,
) -> Result<SessionSnapshot, String> {
    let mut manager = session_state
        .lock()
        .map_err(|_| "session manager lock poisoned".to_string())?;
    let snapshot = manager
        .rename_session(session_id, title)
        .map_err(|err| err.to_string())?;
    sync_tabs_config(&manager, &config_state)?;
    Ok(snapshot)
}

#[tauri::command]
fn write_session(
    state: State<'_, SharedSessionManager>,
    session_id: SessionId,
    input: String,
) -> Result<(), String> {
    with_manager(state, |manager| {
        manager.write_session(session_id, input.as_bytes())
    })
}

#[tauri::command]
fn resize_session(
    state: State<'_, SharedSessionManager>,
    session_id: SessionId,
    cols: u16,
    rows: u16,
) -> Result<SessionSnapshot, String> {
    with_manager(state, |manager| {
        manager.resize_session(session_id, cols, rows)
    })
}

#[tauri::command]
fn close_session(
    session_state: State<'_, SharedSessionManager>,
    config_state: State<'_, SharedConfig>,
    session_id: SessionId,
) -> Result<(), String> {
    let mut manager = session_state
        .lock()
        .map_err(|_| "session manager lock poisoned".to_string())?;
    manager
        .close_session(session_id)
        .map_err(|err| err.to_string())?;
    sync_tabs_config(&manager, &config_state)
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

fn apply_window_config(window: &WebviewWindow, config: &Config) {
    let position = tauri::PhysicalPosition::new(config.window.left, config.window.top);
    let size = tauri::PhysicalSize::new(config.window.width, config.window.height);

    if config::exists() {
        let _ = window.set_position(position);
        let _ = window.set_size(size);
    } else {
        let _ = window.set_size(size);
        let _ = window.center();
    }

    if config.window.maximized {
        let _ = window.maximize();
    }

    let _ = window.set_title(&paths::exe_stem());
    let _ = window.show();
    let _ = window.set_focus();
}

fn attach_window_persistence(window: &WebviewWindow, config: SharedConfig) {
    let listener_window = window.clone();
    let state_window = window.clone();
    listener_window.on_window_event(move |event| match event {
        WindowEvent::Moved(position) => {
            persist_window_state(&state_window, &config, Some((position.x, position.y)), None);
        }
        WindowEvent::Resized(size) => {
            persist_window_state(
                &state_window,
                &config,
                None,
                Some((size.width, size.height)),
            );
        }
        WindowEvent::CloseRequested { .. } => {
            persist_window_state(&state_window, &config, None, None);
        }
        _ => {}
    });
}

fn persist_window_state(
    window: &WebviewWindow,
    config: &SharedConfig,
    position: Option<(i32, i32)>,
    size: Option<(u32, u32)>,
) {
    let mut guard = match config.lock() {
        Ok(guard) => guard,
        Err(_) => {
            eprintln!("failed to lock config while persisting window state");
            return;
        }
    };

    if let Some((left, top)) = position {
        guard.window.left = left;
        guard.window.top = top;
    }

    if let Some((width, height)) = size {
        guard.window.width = width;
        guard.window.height = height;
    }

    if let Ok(maximized) = window.is_maximized() {
        guard.window.maximized = maximized;
    }

    if let Err(err) = config::save(&guard) {
        eprintln!("failed to save config: {err}");
    }
}

fn default_session_directory(config: &Config) -> PathBuf {
    config
        .terminal
        .startup_directory
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(paths::exe_dir)
}

fn resolve_tab_directory(snapshot: &SessionSnapshot, config: &Config) -> PathBuf {
    snapshot
        .cwd
        .clone()
        .unwrap_or_else(|| default_session_directory(config))
}

fn refresh_tabs_from_manager(manager: &SessionManager, config: &mut Config) -> Result<(), String> {
    let sessions = manager.list_sessions().map_err(|err| err.to_string())?;
    let tab_directories: Vec<PathBuf> = sessions
        .iter()
        .map(|session| resolve_tab_directory(session, config))
        .collect();
    let tab_titles: Vec<Option<String>> = sessions
        .iter()
        .map(|session| {
            if session.title == session::default_title(session.id) {
                None
            } else {
                Some(session.title.clone())
            }
        })
        .collect();
    config.tabs.tab_directories = tab_directories;
    config.tabs.tab_titles = tab_titles;
    config.tabs.active_index = sessions.iter().position(|session| session.active);
    Ok(())
}

fn sync_tabs_config(manager: &SessionManager, config_state: &SharedConfig) -> Result<(), String> {
    let mut guard = config_state
        .lock()
        .map_err(|_| "config lock poisoned".to_string())?;
    refresh_tabs_from_manager(manager, &mut guard)?;
    config::save(&guard)
}

fn build_session_config(
    config: &Config,
    cwd: Option<PathBuf>,
    title: Option<String>,
) -> SessionConfig {
    let mut session_config = SessionConfig::default();
    session_config.title = title;
    session_config.cwd = cwd.or_else(|| config.terminal.startup_directory.clone());
    session_config.shell = config.terminal.shell.clone();
    session_config.args = config.terminal.shell_args.clone();
    session_config
}

fn restore_sessions_from_config(
    manager: &mut SessionManager,
    config: &Config,
) -> Result<(), String> {
    let mut restored_ids = Vec::new();

    if config.tabs.restore_tabs_on_startup && !config.tabs.tab_directories.is_empty() {
        for (index, directory) in config.tabs.tab_directories.iter().enumerate() {
            let title = config.tabs.tab_titles.get(index).cloned().flatten();
            let restore_cwd = if directory.is_dir() {
                Some(directory.clone())
            } else {
                Some(default_session_directory(config))
            };

            let session_config = build_session_config(config, restore_cwd, title);
            match manager.create_session(session_config) {
                Ok(snapshot) => restored_ids.push(snapshot.id),
                Err(err) => eprintln!("failed to restore tab at {}: {}", directory.display(), err),
            }
        }

        if restored_ids.is_empty() {
            manager
                .create_session(build_session_config(
                    config,
                    Some(default_session_directory(config)),
                    None,
                ))
                .map_err(|err| err.to_string())?;
            return Ok(());
        }

        if config.tabs.restore_last_active_tab {
            if let Some(active_index) = config.tabs.active_index {
                if let Some(session_id) = restored_ids.get(active_index) {
                    let _ = manager.set_active_session(*session_id);
                }
            }
        }

        return Ok(());
    }

    manager
        .create_session(build_session_config(
            config,
            Some(default_session_directory(config)),
            None,
        ))
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{default_title, SessionHandleFactory, SessionHandleTrait};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    #[test]
    fn get_app_name_matches_paths_exe_stem() {
        let name = get_app_name();
        assert_eq!(name, paths::exe_stem());
        assert!(!name.is_empty());
    }

    struct NoopSink;

    impl SessionEventSink for NoopSink {
        fn emit(&self, _event: SessionEvent) {}
    }

    struct SnapshotHandle {
        title: String,
        shell: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
    }

    impl SnapshotHandle {
        fn from_config(session_id: SessionId, config: &SessionConfig) -> Self {
            Self {
                title: config
                    .title
                    .clone()
                    .unwrap_or_else(|| default_title(session_id)),
                shell: config.shell.clone().unwrap_or_else(|| "sh".to_string()),
                args: config.args.clone(),
                cwd: config.cwd.clone(),
                cols: config.cols,
                rows: config.rows,
            }
        }
    }

    impl SessionHandleTrait for SnapshotHandle {
        fn snapshot(&self, session_id: SessionId, active: bool) -> SessionSnapshot {
            SessionSnapshot {
                id: session_id,
                title: self.title.clone(),
                shell: self.shell.clone(),
                args: self.args.clone(),
                cwd: self.cwd.clone(),
                cols: self.cols,
                rows: self.rows,
                active,
                running: true,
                process_id: Some(session_id.0 as u32),
                exit: None,
            }
        }

        fn write(&self, _input: &[u8]) -> Result<(), SessionError> {
            Ok(())
        }

        fn resize(&self, _cols: u16, _rows: u16) -> Result<(), SessionError> {
            Ok(())
        }

        fn request_close(&self) -> Result<(), SessionError> {
            Ok(())
        }

        fn is_running(&self) -> bool {
            true
        }
    }

    struct CaptureFactory {
        seen_cwds: Arc<Mutex<Vec<Option<PathBuf>>>>,
    }

    impl SessionHandleFactory for CaptureFactory {
        fn spawn(
            &self,
            session_id: SessionId,
            config: SessionConfig,
            _sink: Arc<dyn SessionEventSink>,
        ) -> Result<Box<dyn SessionHandleTrait>, SessionError> {
            self.seen_cwds
                .lock()
                .map_err(|_| SessionError::Poisoned)?
                .push(config.cwd.clone());
            Ok(Box::new(SnapshotHandle::from_config(session_id, &config)))
        }
    }

    fn make_test_manager(seen_cwds: Arc<Mutex<Vec<Option<PathBuf>>>>) -> SessionManager {
        SessionManager::new(Arc::new(NoopSink), Box::new(CaptureFactory { seen_cwds }))
    }

    fn make_temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gamma-termul-restore-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[test]
    fn refresh_tabs_from_manager_tracks_tab_directories_and_active_index() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));

        let dir_a = make_temp_dir("refresh-a");
        let dir_b = make_temp_dir("refresh-b");

        manager
            .create_session(SessionConfig {
                cwd: Some(dir_a.clone()),
                ..SessionConfig::default()
            })
            .expect("create first session");
        manager
            .create_session(SessionConfig {
                cwd: Some(dir_b.clone()),
                ..SessionConfig::default()
            })
            .expect("create second session");
        manager
            .set_active_session(SessionId(1))
            .expect("activate first session");

        let mut config = Config::default();
        refresh_tabs_from_manager(&manager, &mut config).expect("refresh tabs");

        assert_eq!(
            seen_cwds.lock().unwrap().clone(),
            vec![Some(dir_a.clone()), Some(dir_b.clone())]
        );
        assert_eq!(
            config.tabs.tab_directories,
            vec![dir_a.clone(), dir_b.clone()]
        );
        assert_eq!(config.tabs.tab_titles, vec![None, None]);
        assert_eq!(config.tabs.active_index, Some(0));

        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn restore_sessions_from_config_restores_saved_directories_and_active_tab() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));

        let dir_a = make_temp_dir("restore-a");
        let dir_b = make_temp_dir("restore-b");
        let fallback = make_temp_dir("restore-fallback");

        let config = Config {
            terminal: config::TerminalConfig {
                startup_directory: Some(fallback.clone()),
                ..config::TerminalConfig::default()
            },
            tabs: config::TabsConfig {
                tab_directories: vec![dir_a.clone(), dir_b.clone()],
                tab_titles: vec![Some("Alpha".into()), None],
                active_index: Some(1),
                restore_last_active_tab: true,
                restore_tabs_on_startup: true,
            },
            ..Config::default()
        };

        restore_sessions_from_config(&mut manager, &config).expect("restore tabs");

        let sessions = manager.list_sessions().expect("list restored sessions");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].cwd, Some(dir_a.clone()));
        assert_eq!(sessions[1].cwd, Some(dir_b.clone()));
        assert_eq!(sessions[0].title, "Alpha");
        assert_eq!(sessions[1].title, default_title(SessionId(2)));
        assert_eq!(
            seen_cwds.lock().unwrap().clone(),
            vec![Some(dir_a.clone()), Some(dir_b.clone())]
        );

        let active = manager.active_session().expect("active session").unwrap();
        assert_eq!(active.cwd, Some(dir_b.clone()));

        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
        let _ = std::fs::remove_dir_all(&fallback);
    }

    #[test]
    fn restore_sessions_from_config_falls_back_when_saved_directory_is_missing() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));

        let missing_dir = std::env::temp_dir().join(format!(
            "gamma-termul-restore-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&missing_dir);
        let fallback = make_temp_dir("restore-missing-fallback");

        let config = Config {
            terminal: config::TerminalConfig {
                startup_directory: Some(fallback.clone()),
                ..config::TerminalConfig::default()
            },
            tabs: config::TabsConfig {
                tab_directories: vec![missing_dir.clone()],
                tab_titles: vec![Some("Recovered".into())],
                active_index: Some(0),
                restore_last_active_tab: true,
                restore_tabs_on_startup: true,
            },
            ..Config::default()
        };

        restore_sessions_from_config(&mut manager, &config).expect("restore fallback tab");

        let sessions = manager.list_sessions().expect("list fallback session");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].cwd, Some(fallback.clone()));
        assert_eq!(sessions[0].title, "Recovered");
        assert_eq!(
            seen_cwds.lock().unwrap().clone(),
            vec![Some(fallback.clone())]
        );

        let _ = std::fs::remove_dir_all(&fallback);
    }

    // -----------------------------------------------------------------------
    // default_session_directory
    // -----------------------------------------------------------------------

    #[test]
    fn default_session_directory_uses_terminal_startup_directory_when_set() {
        let cfg = Config {
            terminal: config::TerminalConfig {
                startup_directory: Some(PathBuf::from("/my/startup")),
                ..config::TerminalConfig::default()
            },
            ..Config::default()
        };
        let dir = default_session_directory(&cfg);
        assert_eq!(dir, PathBuf::from("/my/startup"));
    }

    #[test]
    fn default_session_directory_falls_back_when_not_configured() {
        let cfg = Config {
            terminal: config::TerminalConfig {
                startup_directory: None,
                ..config::TerminalConfig::default()
            },
            ..Config::default()
        };
        let dir = default_session_directory(&cfg);
        // Should fall back to current_dir or exe_dir — both non-empty
        assert!(!dir.as_os_str().is_empty());
    }

    // -----------------------------------------------------------------------
    // resolve_tab_directory
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_tab_directory_uses_snapshot_cwd_when_present() {
        let snap = SessionSnapshot {
            id: SessionId(1),
            title: "tab".into(),
            shell: "sh".into(),
            args: Vec::new(),
            cwd: Some(PathBuf::from("/tab/cwd")),
            cols: 80,
            rows: 24,
            active: true,
            running: true,
            process_id: Some(1),
            exit: None,
        };
        let cfg = Config::default();
        let dir = resolve_tab_directory(&snap, &cfg);
        assert_eq!(dir, PathBuf::from("/tab/cwd"));
    }

    #[test]
    fn resolve_tab_directory_falls_back_when_no_cwd_in_snapshot() {
        let snap = SessionSnapshot {
            id: SessionId(1),
            title: "tab".into(),
            shell: "sh".into(),
            args: Vec::new(),
            cwd: None,
            cols: 80,
            rows: 24,
            active: true,
            running: true,
            process_id: Some(1),
            exit: None,
        };
        let cfg = Config {
            terminal: config::TerminalConfig {
                startup_directory: Some(PathBuf::from("/fallback")),
                ..config::TerminalConfig::default()
            },
            ..Config::default()
        };
        let dir = resolve_tab_directory(&snap, &cfg);
        assert_eq!(dir, PathBuf::from("/fallback"));
    }

    // -----------------------------------------------------------------------
    // build_session_config
    // -----------------------------------------------------------------------

    #[test]
    fn build_session_config_uses_cwd_from_config_when_not_overridden() {
        let cfg = Config {
            terminal: config::TerminalConfig {
                startup_directory: Some(PathBuf::from("/startup")),
                shell: Some("zsh".into()),
                shell_args: vec!["-l".into()],
                ..config::TerminalConfig::default()
            },
            ..Config::default()
        };
        let session_cfg = build_session_config(&cfg, None, None);
        assert_eq!(session_cfg.cwd, Some(PathBuf::from("/startup")));
        assert_eq!(session_cfg.shell, Some("zsh".into()));
        assert_eq!(session_cfg.args, vec!["-l"]);
    }

    #[test]
    fn build_session_config_explicit_cwd_overrides_config() {
        let cfg = Config {
            terminal: config::TerminalConfig {
                startup_directory: Some(PathBuf::from("/startup")),
                ..config::TerminalConfig::default()
            },
            ..Config::default()
        };
        let session_cfg = build_session_config(&cfg, Some(PathBuf::from("/explicit")), None);
        assert_eq!(session_cfg.cwd, Some(PathBuf::from("/explicit")));
    }

    #[test]
    fn build_session_config_empty_shell_not_overridden() {
        let cfg = Config {
            terminal: config::TerminalConfig {
                shell: None,
                shell_args: Vec::new(),
                ..config::TerminalConfig::default()
            },
            ..Config::default()
        };
        let session_cfg = build_session_config(&cfg, None, None);
        assert!(session_cfg.shell.is_none());
        assert!(session_cfg.args.is_empty());
    }

    // -----------------------------------------------------------------------
    // refresh_tabs_from_manager with empty sessions
    // -----------------------------------------------------------------------

    #[test]
    fn refresh_tabs_from_manager_empty_sessions_clears_directories() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let manager = make_test_manager(Arc::clone(&seen_cwds));

        let mut config = Config::default();
        config.tabs.tab_directories = vec![PathBuf::from("/stale")];
        config.tabs.active_index = Some(0);

        refresh_tabs_from_manager(&manager, &mut config).expect("refresh empty");

        assert!(config.tabs.tab_directories.is_empty());
        assert!(config.tabs.active_index.is_none());
    }

    // -----------------------------------------------------------------------
    // restore_sessions_from_config edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn restore_sessions_from_config_skips_when_restore_tabs_disabled() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));

        let config = Config {
            tabs: config::TabsConfig {
                tab_directories: vec![PathBuf::from("/whatever")],
                restore_tabs_on_startup: false,
                ..config::TabsConfig::default()
            },
            ..Config::default()
        };

        restore_sessions_from_config(&mut manager, &config).expect("restore disabled");

        // Should create exactly one default session
        let sessions = manager.list_sessions().expect("list sessions");
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn restore_sessions_from_config_creates_one_when_tab_dirs_empty() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));

        let config = Config {
            tabs: config::TabsConfig {
                tab_directories: Vec::new(),
                restore_tabs_on_startup: true,
                ..config::TabsConfig::default()
            },
            ..Config::default()
        };

        restore_sessions_from_config(&mut manager, &config).expect("empty dirs");
        let sessions = manager.list_sessions().expect("list");
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn restore_sessions_from_config_no_active_restore_when_flag_false() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));
        let dir = make_temp_dir("restore-noactive");

        let config = Config {
            tabs: config::TabsConfig {
                tab_directories: vec![dir.clone(), dir.clone()],
                tab_titles: vec![None, None],
                active_index: Some(0),
                restore_last_active_tab: false,
                restore_tabs_on_startup: true,
            },
            ..Config::default()
        };

        restore_sessions_from_config(&mut manager, &config).expect("restore");
        // Both tabs restored, but last (session 2) should be active (default)
        let active = manager.active_session().unwrap().unwrap();
        assert_eq!(active.id, SessionId(2));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_sessions_from_config_all_sessions_fail_fallback_to_default() {
        // Factory that always fails
        struct FailingFactory;
        impl SessionHandleFactory for FailingFactory {
            fn spawn(
                &self,
                _session_id: SessionId,
                _config: SessionConfig,
                _sink: Arc<dyn SessionEventSink>,
            ) -> Result<Box<dyn SessionHandleTrait>, SessionError> {
                Err(SessionError::Factory("intentional fail".into()))
            }
        }

        let mut manager = SessionManager::new(Arc::new(NoopSink), Box::new(FailingFactory));

        let config = Config {
            tabs: config::TabsConfig {
                tab_directories: vec![PathBuf::from("/fail1"), PathBuf::from("/fail2")],
                restore_tabs_on_startup: true,
                ..config::TabsConfig::default()
            },
            ..Config::default()
        };

        // The factory always fails, so the fallback should also fail
        let result = restore_sessions_from_config(&mut manager, &config);
        assert!(result.is_err(), "should fail when factory always fails");
    }

    // -----------------------------------------------------------------------
    // build_session_config with title override
    // -----------------------------------------------------------------------

    #[test]
    fn build_session_config_uses_explicit_title() {
        let cfg = Config::default();
        let session_cfg = build_session_config(&cfg, None, Some("my-title".into()));
        assert_eq!(session_cfg.title.as_deref(), Some("my-title"));
    }

    #[test]
    fn build_session_config_title_can_be_none() {
        let cfg = Config::default();
        let session_cfg = build_session_config(&cfg, None, None);
        assert!(session_cfg.title.is_none());
    }

    // -----------------------------------------------------------------------
    // refresh_tabs_from_manager tracks tab titles from snapshots
    // -----------------------------------------------------------------------

    #[test]
    fn refresh_tabs_from_manager_tracks_tab_titles() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));

        let dir = make_temp_dir("titles-test");

        manager
            .create_session(SessionConfig {
                title: Some("Editor".into()),
                cwd: Some(dir.clone()),
                ..SessionConfig::default()
            })
            .expect("create first");
        manager
            .create_session(SessionConfig {
                title: Some("Terminal".into()),
                cwd: Some(dir.clone()),
                ..SessionConfig::default()
            })
            .expect("create second");

        let mut config = Config::default();
        refresh_tabs_from_manager(&manager, &mut config).expect("refresh");

        assert_eq!(
            config.tabs.tab_titles,
            vec![Some("Editor".into()), Some("Terminal".into())]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_tabs_from_manager_titles_fallback_to_none() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));

        let dir = make_temp_dir("titles-none");

        manager
            .create_session(SessionConfig {
                title: None,
                cwd: Some(dir.clone()),
                ..SessionConfig::default()
            })
            .expect("create");

        let mut config = Config::default();
        refresh_tabs_from_manager(&manager, &mut config).expect("refresh");

        assert_eq!(config.tabs.tab_titles, vec![None]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // sync_tabs_config — end-to-end through SharedConfig
    // -----------------------------------------------------------------------

    #[test]
    fn sync_tabs_config_updates_config_and_persists() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));
        let dir = make_temp_dir("sync-test");

        manager
            .create_session(SessionConfig {
                title: Some("sync-tab".into()),
                cwd: Some(dir.clone()),
                ..SessionConfig::default()
            })
            .expect("create");

        let config_state: SharedConfig = Arc::new(Mutex::new(Config::default()));
        sync_tabs_config(&manager, &config_state).expect("sync");

        let guard = config_state.lock().unwrap();
        assert_eq!(guard.tabs.tab_directories, vec![dir.clone()]);
        assert_eq!(guard.tabs.tab_titles, vec![Some("sync-tab".into())]);
        assert_eq!(guard.tabs.active_index, Some(0));

        // Cleanup the .config file written next to the exe
        let _ = std::fs::remove_file(config::path());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_tabs_config_with_empty_manager_clears_config() {
        let manager = make_test_manager(Arc::new(Mutex::new(Vec::new())));

        let config_state: SharedConfig = Arc::new(Mutex::new(Config {
            tabs: config::TabsConfig {
                tab_directories: vec![PathBuf::from("/stale")],
                tab_titles: vec![Some("stale".into())],
                active_index: Some(0),
                ..config::TabsConfig::default()
            },
            ..Config::default()
        }));

        sync_tabs_config(&manager, &config_state).expect("sync empty");

        let guard = config_state.lock().unwrap();
        assert!(guard.tabs.tab_directories.is_empty());
        assert!(guard.tabs.tab_titles.is_empty());
        assert!(guard.tabs.active_index.is_none());

        let _ = std::fs::remove_file(config::path());
    }

    // -----------------------------------------------------------------------
    // restore_sessions_from_config edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn restore_sessions_active_index_out_of_bounds_ignored() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));
        let dir = make_temp_dir("restore-oob");

        let config = Config {
            tabs: config::TabsConfig {
                tab_directories: vec![dir.clone()],
                tab_titles: vec![Some("OOB".into())],
                active_index: Some(5), // out of bounds — only 1 entry
                restore_last_active_tab: true,
                restore_tabs_on_startup: true,
            },
            ..Config::default()
        };

        restore_sessions_from_config(&mut manager, &config).expect("restore oob");

        let sessions = manager.list_sessions().expect("list");
        assert_eq!(sessions.len(), 1);
        // Since active_index(5) is out of bounds for 1 entry, nothing happens
        // but the session should still be created
        assert_eq!(sessions[0].cwd, Some(dir.clone()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_sessions_mixed_success_partial_fallback() {
        let seen_cwds = Arc::new(Mutex::new(Vec::new()));
        let dir_ok = make_temp_dir("restore-partial-ok");
        let dir_fail = PathBuf::from("/nonexistent/path/that/will/never/exist");

        let config = Config {
            tabs: config::TabsConfig {
                tab_directories: vec![dir_ok.clone(), dir_fail],
                tab_titles: vec![Some("OK".into()), None],
                restore_tabs_on_startup: true,
                restore_last_active_tab: false,
                ..config::TabsConfig::default()
            },
            ..Config::default()
        };

        // Use a factory that succeeds for all (the default MockFactory)
        let mut manager = make_test_manager(Arc::clone(&seen_cwds));
        restore_sessions_from_config(&mut manager, &config).expect("restore mixed");

        // Both sessions created — the missing dir falls back to default session dir
        let sessions = manager.list_sessions().expect("list");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].cwd, Some(dir_ok.clone()));
        // Second session gets fallback dir since dir_fail doesn't exist

        let _ = std::fs::remove_dir_all(&dir_ok);
    }
}
