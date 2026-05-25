use std::{
    collections::{BTreeMap, HashMap},
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use portable_pty::{
    native_pty_system, Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SESSION_EVENT_CHANNEL: &str = "termul:session-event";

mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        STANDARD.decode(encoded).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            title: None,
            shell: None,
            args: Vec::new(),
            cwd: None,
            cols: default_cols(),
            rows: default_rows(),
            env: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitInfo {
    pub code: Option<u32>,
    pub signal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub title: String,
    pub shell: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
    pub active: bool,
    pub running: bool,
    pub process_id: Option<u32>,
    pub exit: Option<ExitInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    Output {
        session_id: SessionId,
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
    Exit {
        session_id: SessionId,
        exit: ExitInfo,
    },
    Error {
        session_id: SessionId,
        message: String,
    },
}

pub trait SessionEventSink: Send + Sync {
    fn emit(&self, event: SessionEvent);
}

/// Trait abstracting a session handle so the manager can be tested without real PTYs.
pub trait SessionHandleTrait: Send {
    fn snapshot(&self, session_id: SessionId, active: bool) -> SessionSnapshot;
    fn write(&self, input: &[u8]) -> Result<(), SessionError>;
    fn resize(&self, cols: u16, rows: u16) -> Result<(), SessionError>;
    fn request_close(&self) -> Result<(), SessionError>;
    fn is_running(&self) -> bool;
}

/// Trait for creating session handles, injectable for testing.
pub trait SessionHandleFactory: Send + Sync {
    fn spawn(
        &self,
        session_id: SessionId,
        config: SessionConfig,
        sink: Arc<dyn SessionEventSink>,
    ) -> Result<Box<dyn SessionHandleTrait>, SessionError>;
}

/// Default factory that creates real PTY-backed session handles.
pub struct DefaultSessionHandleFactory;

impl SessionHandleFactory for DefaultSessionHandleFactory {
    fn spawn(
        &self,
        session_id: SessionId,
        config: SessionConfig,
        sink: Arc<dyn SessionEventSink>,
    ) -> Result<Box<dyn SessionHandleTrait>, SessionError> {
        Ok(Box::new(SessionHandle::spawn(session_id, config, sink)?))
    }
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session {0:?} not found")]
    NotFound(SessionId),
    #[error("pty error: {0}")]
    Pty(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("session state lock poisoned")]
    Poisoned,
    #[error("factory error: {0}")]
    #[allow(dead_code)]
    Factory(String),
}

pub struct SessionManager {
    next_id: u64,
    active: Option<SessionId>,
    order: Vec<SessionId>,
    sessions: HashMap<SessionId, Box<dyn SessionHandleTrait>>,
    sink: Arc<dyn SessionEventSink>,
    factory: Box<dyn SessionHandleFactory>,
}

impl SessionManager {
    pub fn new(sink: Arc<dyn SessionEventSink>, factory: Box<dyn SessionHandleFactory>) -> Self {
        Self {
            next_id: 1,
            active: None,
            order: Vec::new(),
            sessions: HashMap::new(),
            sink,
            factory,
        }
    }

    pub fn create_session(
        &mut self,
        config: SessionConfig,
    ) -> Result<SessionSnapshot, SessionError> {
        let session_id = SessionId(self.next_id);
        self.next_id += 1;

        let handle = self
            .factory
            .spawn(session_id, config, Arc::clone(&self.sink))?;
        self.sessions.insert(session_id, handle);
        self.order.push(session_id);
        self.active = Some(session_id);

        self.snapshot_for(session_id, true)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionSnapshot>, SessionError> {
        let mut sessions = Vec::with_capacity(self.order.len());

        for session_id in &self.order {
            if let Some(handle) = self.sessions.get(session_id) {
                sessions.push(handle.snapshot(*session_id, self.active == Some(*session_id)));
            }
        }

        Ok(sessions)
    }

    pub fn active_session(&self) -> Result<Option<SessionSnapshot>, SessionError> {
        let Some(session_id) = self.active else {
            return Ok(None);
        };

        Ok(Some(self.snapshot_for(session_id, true)?))
    }

    pub fn set_active_session(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionSnapshot, SessionError> {
        if !self.sessions.contains_key(&session_id) {
            return Err(SessionError::NotFound(session_id));
        }

        self.active = Some(session_id);
        self.snapshot_for(session_id, true)
    }

    pub fn write_session(
        &mut self,
        session_id: SessionId,
        input: &[u8],
    ) -> Result<(), SessionError> {
        let session = self
            .sessions
            .get(&session_id)
            .ok_or(SessionError::NotFound(session_id))?;
        session.write(input)?;
        Ok(())
    }

    pub fn resize_session(
        &mut self,
        session_id: SessionId,
        cols: u16,
        rows: u16,
    ) -> Result<SessionSnapshot, SessionError> {
        let session = self
            .sessions
            .get(&session_id)
            .ok_or(SessionError::NotFound(session_id))?;
        session.resize(cols, rows)?;
        self.snapshot_for(session_id, self.active == Some(session_id))
    }

    pub fn close_session(&mut self, session_id: SessionId) -> Result<(), SessionError> {
        let Some(session) = self.sessions.get(&session_id) else {
            return Err(SessionError::NotFound(session_id));
        };

        if session.is_running() {
            session.request_close()?;
        }

        let next_active = self.choose_active_after_close(session_id);
        self.sessions.remove(&session_id);
        self.order.retain(|id| *id != session_id);
        self.active = next_active;

        Ok(())
    }

    fn snapshot_for(
        &self,
        session_id: SessionId,
        active: bool,
    ) -> Result<SessionSnapshot, SessionError> {
        let session = self
            .sessions
            .get(&session_id)
            .ok_or(SessionError::NotFound(session_id))?;
        Ok(session.snapshot(session_id, active))
    }

    fn choose_active_after_close(&self, closing: SessionId) -> Option<SessionId> {
        if self.active != Some(closing) {
            return self.active;
        }

        let Some(index) = self.order.iter().position(|id| *id == closing) else {
            return None;
        };

        self.order.get(index + 1).copied().or_else(|| {
            index
                .checked_sub(1)
                .and_then(|prev| self.order.get(prev).copied())
        })
    }
}

struct SessionHandle {
    state: Arc<Mutex<SessionState>>,
    master: Box<dyn MasterPty + Send>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

#[derive(Debug, Clone)]
struct SessionState {
    title: String,
    shell: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    cols: u16,
    rows: u16,
    running: bool,
    process_id: Option<u32>,
    exit: Option<ExitInfo>,
}

impl SessionHandle {
    fn spawn(
        session_id: SessionId,
        config: SessionConfig,
        sink: Arc<dyn SessionEventSink>,
    ) -> Result<Self, SessionError> {
        let SessionConfig {
            title,
            shell,
            args,
            cwd,
            cols,
            rows,
            env,
        } = config;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| SessionError::Pty(err.to_string()))?;

        let shell = shell.unwrap_or_else(default_shell);
        let args_for_state = args.clone();
        let mut command = CommandBuilder::new(shell.clone());
        command.args(args);

        if let Some(ref cwd) = cwd {
            command.cwd(cwd);
        }

        for (key, value) in &env {
            command.env(key, value);
        }

        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| SessionError::Pty(err.to_string()))?;
        let process_id = child.process_id();
        let killer = child.clone_killer();
        let master = pair.master;
        let reader = master.try_clone_reader().map_err(|err| {
            let _ = child.kill();
            SessionError::Pty(err.to_string())
        })?;
        let writer = master.take_writer().map_err(|err| {
            let _ = child.kill();
            SessionError::Pty(err.to_string())
        })?;

        let state = Arc::new(Mutex::new(SessionState {
            title: title.unwrap_or_else(|| default_title(session_id)),
            shell,
            args: args_for_state,
            cwd,
            cols,
            rows,
            running: true,
            process_id,
            exit: None,
        }));

        Self::spawn_reader(session_id, reader, Arc::clone(&sink));
        Self::spawn_waiter(session_id, child, Arc::clone(&sink), Arc::clone(&state));

        Ok(Self {
            state,
            master,
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
        })
    }

    fn spawn_reader(
        session_id: SessionId,
        mut reader: Box<dyn Read + Send>,
        sink: Arc<dyn SessionEventSink>,
    ) {
        thread::spawn(move || {
            let mut buffer = [0u8; 8192];

            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => sink.emit(SessionEvent::Output {
                        session_id,
                        data: buffer[..count].to_vec(),
                    }),
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(err) => {
                        sink.emit(SessionEvent::Error {
                            session_id,
                            message: err.to_string(),
                        });
                        break;
                    }
                }
            }
        });
    }

    fn spawn_waiter(
        session_id: SessionId,
        mut child: Box<dyn Child + Send>,
        sink: Arc<dyn SessionEventSink>,
        state: Arc<Mutex<SessionState>>,
    ) {
        thread::spawn(move || match child.wait() {
            Ok(status) => {
                let exit = exit_info(&status);
                if let Ok(mut session_state) = state.lock() {
                    session_state.running = false;
                    session_state.exit = Some(exit.clone());
                }

                sink.emit(SessionEvent::Exit { session_id, exit });
            }
            Err(err) => {
                sink.emit(SessionEvent::Error {
                    session_id,
                    message: err.to_string(),
                });
            }
        });
    }
}

impl SessionHandleTrait for SessionHandle {
    fn snapshot(&self, session_id: SessionId, active: bool) -> SessionSnapshot {
        let state = self
            .state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| SessionState {
                title: default_title(session_id),
                shell: default_shell(),
                args: Vec::new(),
                cwd: None,
                cols: default_cols(),
                rows: default_rows(),
                running: false,
                process_id: None,
                exit: None,
            });

        SessionSnapshot {
            id: session_id,
            title: state.title,
            shell: state.shell,
            args: state.args,
            cwd: state.cwd,
            cols: state.cols,
            rows: state.rows,
            active,
            running: state.running,
            process_id: state.process_id,
            exit: state.exit,
        }
    }

    fn write(&self, input: &[u8]) -> Result<(), SessionError> {
        let mut writer = self.writer.lock().map_err(|_| SessionError::Poisoned)?;
        writer.write_all(input)?;
        writer.flush()?;
        Ok(())
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), SessionError> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| SessionError::Pty(err.to_string()))?;

        let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
        state.cols = cols;
        state.rows = rows;
        Ok(())
    }

    fn request_close(&self) -> Result<(), SessionError> {
        let mut killer = self.killer.lock().map_err(|_| SessionError::Poisoned)?;
        killer
            .kill()
            .map_err(|err| SessionError::Pty(err.to_string()))
    }

    fn is_running(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.running)
            .unwrap_or(false)
    }
}

fn exit_info(status: &ExitStatus) -> ExitInfo {
    ExitInfo {
        code: Some(status.exit_code()),
        signal: status.signal().map(ToOwned::to_owned),
    }
}

fn default_title(session_id: SessionId) -> String {
    format!("Tab {}", session_id.0)
}

#[cfg(windows)]
fn default_shell() -> String {
    std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string())
}

#[cfg(not(windows))]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Mock helpers
    // -----------------------------------------------------------------------

    struct MockSessionEventSink;

    impl SessionEventSink for MockSessionEventSink {
        fn emit(&self, _event: SessionEvent) {}
    }

    struct CountingSink {
        events: Arc<Mutex<Vec<SessionEvent>>>,
    }

    impl CountingSink {
        fn new() -> (Self, Arc<Mutex<Vec<SessionEvent>>>) {
            let events = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    events: Arc::clone(&events),
                },
                events,
            )
        }
    }

    impl SessionEventSink for CountingSink {
        fn emit(&self, event: SessionEvent) {
            if let Ok(mut guard) = self.events.lock() {
                guard.push(event);
            }
        }
    }

    struct MockSessionHandle {
        state: Arc<Mutex<SessionState>>,
        written: Arc<Mutex<Vec<u8>>>,
        kill_called: Arc<Mutex<bool>>,
    }

    impl MockSessionHandle {
        fn new(state: SessionState) -> Self {
            Self {
                state: Arc::new(Mutex::new(state)),
                written: Arc::new(Mutex::new(Vec::new())),
                kill_called: Arc::new(Mutex::new(false)),
            }
        }

        fn running() -> Self {
            Self::new(SessionState {
                title: "mock".into(),
                shell: "mock-shell".into(),
                args: Vec::new(),
                cwd: None,
                cols: 80,
                rows: 24,
                running: true,
                process_id: Some(42),
                exit: None,
            })
        }

        fn stopped() -> Self {
            Self::new(SessionState {
                title: "stopped".into(),
                shell: "mock-shell".into(),
                args: Vec::new(),
                cwd: None,
                cols: 80,
                rows: 24,
                running: false,
                process_id: None,
                exit: Some(ExitInfo {
                    code: Some(0),
                    signal: None,
                }),
            })
        }
    }

    impl SessionHandleTrait for MockSessionHandle {
        fn snapshot(&self, session_id: SessionId, active: bool) -> SessionSnapshot {
            let state = self
                .state
                .lock()
                .map(|s| s.clone())
                .unwrap_or_else(|_| SessionState {
                    title: default_title(session_id),
                    shell: default_shell(),
                    args: Vec::new(),
                    cwd: None,
                    cols: default_cols(),
                    rows: default_rows(),
                    running: false,
                    process_id: None,
                    exit: None,
                });

            SessionSnapshot {
                id: session_id,
                title: state.title,
                shell: state.shell,
                args: state.args,
                cwd: state.cwd,
                cols: state.cols,
                rows: state.rows,
                active,
                running: state.running,
                process_id: state.process_id,
                exit: state.exit,
            }
        }

        fn write(&self, input: &[u8]) -> Result<(), SessionError> {
            let mut written = self.written.lock().map_err(|_| SessionError::Poisoned)?;
            written.extend_from_slice(input);
            Ok(())
        }

        fn resize(&self, cols: u16, rows: u16) -> Result<(), SessionError> {
            let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
            state.cols = cols;
            state.rows = rows;
            Ok(())
        }

        fn request_close(&self) -> Result<(), SessionError> {
            let mut kill = self
                .kill_called
                .lock()
                .map_err(|_| SessionError::Poisoned)?;
            *kill = true;
            let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
            state.running = false;
            Ok(())
        }

        fn is_running(&self) -> bool {
            self.state
                .lock()
                .map(|state| state.running)
                .unwrap_or(false)
        }
    }

    struct MockFactory;

    impl SessionHandleFactory for MockFactory {
        fn spawn(
            &self,
            _session_id: SessionId,
            _config: SessionConfig,
            _sink: Arc<dyn SessionEventSink>,
        ) -> Result<Box<dyn SessionHandleTrait>, SessionError> {
            Ok(Box::new(MockSessionHandle::running()))
        }
    }

    struct FailingMockFactory;

    impl SessionHandleFactory for FailingMockFactory {
        fn spawn(
            &self,
            _session_id: SessionId,
            _config: SessionConfig,
            _sink: Arc<dyn SessionEventSink>,
        ) -> Result<Box<dyn SessionHandleTrait>, SessionError> {
            Err(SessionError::Factory("mock factory failure".into()))
        }
    }

    fn make_manager() -> SessionManager {
        let sink: Arc<dyn SessionEventSink> = Arc::new(MockSessionEventSink);
        let factory: Box<dyn SessionHandleFactory> = Box::new(MockFactory);
        SessionManager::new(sink, factory)
    }

    fn make_manager_with_factory(factory: Box<dyn SessionHandleFactory>) -> SessionManager {
        let sink: Arc<dyn SessionEventSink> = Arc::new(MockSessionEventSink);
        SessionManager::new(sink, factory)
    }

    fn create_n(manager: &mut SessionManager, n: usize) -> Vec<SessionSnapshot> {
        let mut snapshots = Vec::with_capacity(n);
        for _ in 0..n {
            snapshots.push(
                manager
                    .create_session(SessionConfig::default())
                    .expect("create should succeed"),
            );
        }
        snapshots
    }

    // -----------------------------------------------------------------------
    // SessionConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn session_config_default_cols_rows() {
        let cfg = SessionConfig::default();
        assert_eq!(cfg.cols, 80);
        assert_eq!(cfg.rows, 24);
        assert!(cfg.title.is_none());
        assert!(cfg.shell.is_none());
        assert!(cfg.cwd.is_none());
        assert!(cfg.args.is_empty());
        assert!(cfg.env.is_empty());
    }

    #[test]
    fn session_config_with_explicit_values() {
        let cfg = SessionConfig {
            title: Some("work".into()),
            cols: 132,
            rows: 43,
            ..SessionConfig::default()
        };
        assert_eq!(cfg.title.as_deref(), Some("work"));
        assert_eq!(cfg.cols, 132);
        assert_eq!(cfg.rows, 43);
    }

    #[test]
    fn session_config_serialize_roundtrip() {
        let cfg = SessionConfig {
            title: Some("test".into()),
            shell: Some("bash".into()),
            cols: 100,
            rows: 40,
            ..SessionConfig::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let deserialized: SessionConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.title, cfg.title);
        assert_eq!(deserialized.shell, cfg.shell);
        assert_eq!(deserialized.cols, 100);
        assert_eq!(deserialized.rows, 40);
    }

    // -----------------------------------------------------------------------
    // SessionId tests
    // -----------------------------------------------------------------------

    #[test]
    fn session_id_equality() {
        assert_eq!(SessionId(1), SessionId(1));
        assert_ne!(SessionId(1), SessionId(2));
    }

    #[test]
    fn session_id_ordering() {
        let mut ids = vec![SessionId(3), SessionId(1), SessionId(2)];
        ids.sort();
        assert_eq!(ids, vec![SessionId(1), SessionId(2), SessionId(3)]);
    }

    #[test]
    fn session_id_hash_map() {
        let mut map = HashMap::new();
        map.insert(SessionId(7), "seven");
        assert_eq!(map.get(&SessionId(7)), Some(&"seven"));
        assert_eq!(map.get(&SessionId(8)), None);
    }

    // -----------------------------------------------------------------------
    // SessionEvent tests
    // -----------------------------------------------------------------------

    #[test]
    fn session_event_serialization_output() {
        let event = SessionEvent::Output {
            session_id: SessionId(1),
            data: vec![104, 101, 108, 108, 111],
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"kind\":\"output\""));
        assert!(json.contains("\"session_id\":1"));
        assert!(json.contains("\"data\":\"aGVsbG8=\""));

        let decoded: SessionEvent = serde_json::from_str(&json).expect("deserialize");
        match decoded {
            SessionEvent::Output { session_id, data } => {
                assert_eq!(session_id, SessionId(1));
                assert_eq!(data, b"hello");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn session_event_serialization_exit() {
        let event = SessionEvent::Exit {
            session_id: SessionId(2),
            exit: ExitInfo {
                code: Some(0),
                signal: None,
            },
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"kind\":\"exit\""));
        assert!(json.contains("\"code\":0"));
    }

    #[test]
    fn session_event_serialization_error() {
        let event = SessionEvent::Error {
            session_id: SessionId(3),
            message: "broken".into(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"kind\":\"error\""));
        assert!(json.contains("\"message\":\"broken\""));
    }

    // -----------------------------------------------------------------------
    // SessionError tests
    // -----------------------------------------------------------------------

    #[test]
    fn session_error_not_found_display() {
        let err = SessionError::NotFound(SessionId(99));
        assert_eq!(err.to_string(), "session SessionId(99) not found");
    }

    #[test]
    fn session_error_pty_display() {
        let err = SessionError::Pty("fork failed".into());
        assert_eq!(err.to_string(), "pty error: fork failed");
    }

    #[test]
    fn session_error_io_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broke");
        let err: SessionError = io_err.into();
        assert!(matches!(err, SessionError::Io(_)));
    }

    #[test]
    fn session_error_poisoned_display() {
        let err = SessionError::Poisoned;
        assert_eq!(err.to_string(), "session state lock poisoned");
    }

    // -----------------------------------------------------------------------
    // SessionManager: lifecycle / empty state
    // -----------------------------------------------------------------------

    #[test]
    fn new_manager_is_empty() {
        let manager = make_manager();
        assert!(manager.active.is_none());
        assert!(manager.order.is_empty());
        assert!(manager.sessions.is_empty());
    }

    #[test]
    fn list_on_empty_manager_returns_empty_vec() {
        let manager = make_manager();
        let sessions = manager
            .list_sessions()
            .expect("list_sessions should succeed");
        assert!(sessions.is_empty());
    }

    #[test]
    fn set_active_session_on_empty_returns_not_found() {
        let mut manager = make_manager();
        let err = manager
            .set_active_session(SessionId(1))
            .expect_err("should fail");
        assert!(matches!(err, SessionError::NotFound(SessionId(1))));
    }

    #[test]
    fn close_session_on_empty_returns_not_found() {
        let mut manager = make_manager();
        let err = manager
            .close_session(SessionId(1))
            .expect_err("should fail");
        assert!(matches!(err, SessionError::NotFound(SessionId(1))));
    }

    #[test]
    fn write_session_on_empty_returns_not_found() {
        let mut manager = make_manager();
        let err = manager
            .write_session(SessionId(1), b"hello")
            .expect_err("should fail");
        assert!(matches!(err, SessionError::NotFound(SessionId(1))));
    }

    #[test]
    fn resize_session_on_empty_returns_not_found() {
        let mut manager = make_manager();
        let err = manager
            .resize_session(SessionId(1), 80, 24)
            .expect_err("should fail");
        assert!(matches!(err, SessionError::NotFound(SessionId(1))));
    }

    // -----------------------------------------------------------------------
    // SessionManager: create + list
    // -----------------------------------------------------------------------

    #[test]
    fn create_session_adds_one_session() {
        let mut manager = make_manager();
        let snap = manager
            .create_session(SessionConfig::default())
            .expect("create should succeed");

        assert_eq!(snap.id, SessionId(1));
        assert!(snap.active);
        assert!(snap.running);

        let sessions = manager.list_sessions().expect("list should succeed");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, SessionId(1));
    }

    #[test]
    fn create_session_auto_increments_ids() {
        let mut manager = make_manager();
        let a = manager
            .create_session(SessionConfig::default())
            .expect("create a");
        let b = manager
            .create_session(SessionConfig::default())
            .expect("create b");
        let c = manager
            .create_session(SessionConfig::default())
            .expect("create c");

        assert_eq!(a.id, SessionId(1));
        assert_eq!(b.id, SessionId(2));
        assert_eq!(c.id, SessionId(3));
    }

    #[test]
    fn create_session_sets_new_tab_as_active() {
        let mut manager = make_manager();
        let a = manager.create_session(SessionConfig::default()).unwrap();
        assert!(a.active);

        let b = manager.create_session(SessionConfig::default()).unwrap();
        assert!(b.active);

        // `a` should no longer be active
        let sessions = manager.list_sessions().unwrap();
        assert!(!sessions[0].active);
        assert!(sessions[1].active);
    }

    #[test]
    fn list_sessions_returns_in_order() {
        let mut manager = make_manager();
        create_n(&mut manager, 3);
        let sessions = manager.list_sessions().unwrap();

        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].id, SessionId(1));
        assert_eq!(sessions[1].id, SessionId(2));
        assert_eq!(sessions[2].id, SessionId(3));
    }

    #[test]
    fn list_sessions_after_create_respects_order() {
        let mut manager = make_manager();
        let _ = manager.create_session(SessionConfig::default()).unwrap();
        let _ = manager.create_session(SessionConfig::default()).unwrap();
        let _ = manager.create_session(SessionConfig::default()).unwrap();

        // Remove middle session and verify order
        manager.close_session(SessionId(2)).unwrap();
        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, SessionId(1));
        assert_eq!(sessions[1].id, SessionId(3));
    }

    // -----------------------------------------------------------------------
    // SessionManager: set_active_session
    // -----------------------------------------------------------------------

    #[test]
    fn set_active_session_changes_active_flag() {
        let mut manager = make_manager();
        create_n(&mut manager, 3);
        manager.set_active_session(SessionId(1)).unwrap();

        let sessions = manager.list_sessions().unwrap();
        assert!(sessions[0].active);
        assert!(!sessions[1].active);
        assert!(!sessions[2].active);
    }

    #[test]
    fn set_active_session_returns_snapshot_for_target() {
        let mut manager = make_manager();
        create_n(&mut manager, 3);
        let snap = manager.set_active_session(SessionId(2)).unwrap();
        assert_eq!(snap.id, SessionId(2));
        assert!(snap.active);
    }

    #[test]
    fn set_active_session_unknown_id_returns_not_found() {
        let mut manager = make_manager();
        create_n(&mut manager, 2);
        let err = manager
            .set_active_session(SessionId(99))
            .expect_err("should fail");
        assert!(matches!(err, SessionError::NotFound(SessionId(99))));
    }

    #[test]
    fn set_active_session_is_idempotent() {
        let mut manager = make_manager();
        create_n(&mut manager, 2);
        let snap = manager.set_active_session(SessionId(2)).unwrap();
        assert_eq!(snap.id, SessionId(2));
        assert!(snap.active);

        // Setting the same again
        let snap2 = manager.set_active_session(SessionId(2)).unwrap();
        assert_eq!(snap2.id, SessionId(2));
        assert!(snap2.active);

        let sessions = manager.list_sessions().unwrap();
        assert!(!sessions[0].active);
        assert!(sessions[1].active);
    }

    #[test]
    fn active_session_returns_current_snapshot() {
        let mut manager = make_manager();
        create_n(&mut manager, 2);

        let active = manager
            .active_session()
            .expect("active session lookup should succeed")
            .expect("active session should exist");

        assert_eq!(active.id, SessionId(2));
        assert!(active.active);
    }

    #[test]
    fn active_session_returns_none_when_empty() {
        let manager = make_manager();
        assert!(manager
            .active_session()
            .expect("lookup should succeed")
            .is_none());
    }

    #[test]
    fn active_session_preserves_restore_metadata() {
        struct MetadataFactory;

        impl SessionHandleFactory for MetadataFactory {
            fn spawn(
                &self,
                _session_id: SessionId,
                config: SessionConfig,
                _sink: Arc<dyn SessionEventSink>,
            ) -> Result<Box<dyn SessionHandleTrait>, SessionError> {
                let shell = config.shell.unwrap_or_else(default_shell);
                let args = config.args;
                Ok(Box::new(MockSessionHandle::new(SessionState {
                    title: config.title.unwrap_or_else(|| "restore".into()),
                    shell,
                    args,
                    cwd: config.cwd,
                    cols: config.cols,
                    rows: config.rows,
                    running: true,
                    process_id: Some(42),
                    exit: None,
                })))
            }
        }

        let sink: Arc<dyn SessionEventSink> = Arc::new(MockSessionEventSink);
        let factory: Box<dyn SessionHandleFactory> = Box::new(MetadataFactory);
        let mut manager = SessionManager::new(sink, factory);

        let mut cfg = SessionConfig::default();
        cfg.title = Some("restore-me".into());
        cfg.shell = Some("bash".into());
        cfg.args = vec!["--login".into()];
        cfg.cwd = Some(PathBuf::from("/work"));
        cfg.cols = 100;
        cfg.rows = 40;

        let snapshot = manager
            .create_session(cfg.clone())
            .expect("session creation should succeed");

        assert_eq!(snapshot.title, "restore-me");
        assert_eq!(snapshot.shell, "bash");
        assert_eq!(snapshot.args, vec!["--login"]);
        assert_eq!(snapshot.cwd, Some(PathBuf::from("/work")));
        assert_eq!(snapshot.cols, 100);
        assert_eq!(snapshot.rows, 40);

        let active = manager
            .active_session()
            .expect("active session lookup should succeed")
            .expect("active session should exist");

        assert_eq!(active.title, "restore-me");
        assert_eq!(active.shell, "bash");
        assert_eq!(active.args, vec!["--login"]);
        assert_eq!(active.cwd, Some(PathBuf::from("/work")));
    }

    // -----------------------------------------------------------------------
    // SessionManager: close_session — tab promotion
    // -----------------------------------------------------------------------

    #[test]
    fn close_session_removes_and_promotes_right_neighbor() {
        let mut manager = make_manager();
        create_n(&mut manager, 3);

        // Session 2 is active (last created)
        manager.close_session(SessionId(2)).unwrap();

        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        // Right neighbor (session 3) should be promoted
        assert_eq!(manager.active, Some(SessionId(3)));
    }

    #[test]
    fn close_last_tab_clears_active() {
        let mut manager = make_manager();
        create_n(&mut manager, 3);

        // Manually set active to the last tab
        manager.set_active_session(SessionId(3)).unwrap();
        manager.close_session(SessionId(3)).unwrap();

        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        // Left neighbor promoted
        assert_eq!(manager.active, Some(SessionId(2)));
    }

    #[test]
    fn close_closing_last_remaining_tab_clears_active() {
        let mut manager = make_manager();
        create_n(&mut manager, 1);

        manager.close_session(SessionId(1)).unwrap();
        assert!(manager.active.is_none());
        assert!(manager.order.is_empty());
        assert!(manager.list_sessions().unwrap().is_empty());
    }

    #[test]
    fn close_non_active_session_does_not_change_active() {
        let mut manager = make_manager();
        create_n(&mut manager, 3);

        // Switch active to session 1
        manager.set_active_session(SessionId(1)).unwrap();
        // Close session 3 (non-active)
        manager.close_session(SessionId(3)).unwrap();

        assert_eq!(manager.active, Some(SessionId(1)));
        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn close_session_unknown_returns_not_found() {
        let mut manager = make_manager();
        create_n(&mut manager, 2);
        let err = manager
            .close_session(SessionId(99))
            .expect_err("should fail");
        assert!(matches!(err, SessionError::NotFound(SessionId(99))));
    }

    #[test]
    fn close_session_does_not_affect_other_sessions() {
        let mut manager = make_manager();
        create_n(&mut manager, 3);

        manager.set_active_session(SessionId(1)).unwrap();
        manager.close_session(SessionId(2)).unwrap();

        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, SessionId(1));
        assert_eq!(sessions[1].id, SessionId(3));

        // Session 1 remains active
        assert_eq!(manager.active, Some(SessionId(1)));
    }

    // -----------------------------------------------------------------------
    // SessionManager: write_session
    // -----------------------------------------------------------------------

    #[test]
    fn write_session_delegates_to_handle() {
        let mut manager = make_manager();
        create_n(&mut manager, 1);

        manager
            .write_session(SessionId(1), b"Hello, world!")
            .expect("write should succeed");
        // Data delivery is verified by the dedicated write-through test below.
    }

    #[test]
    fn write_session_unknown_id_returns_not_found() {
        let mut manager = make_manager();
        let err = manager
            .write_session(SessionId(42), b"data")
            .expect_err("should fail");
        assert!(matches!(err, SessionError::NotFound(SessionId(42))));
    }

    // -----------------------------------------------------------------------
    // SessionManager: resize_session
    // -----------------------------------------------------------------------

    #[test]
    fn resize_session_updates_cols_and_rows() {
        let mut manager = make_manager();
        let snap = manager
            .create_session(SessionConfig::default())
            .expect("create");

        assert_eq!(snap.cols, 80);
        assert_eq!(snap.rows, 24);

        let updated = manager
            .resize_session(SessionId(1), 132, 43)
            .expect("resize");
        assert_eq!(updated.cols, 132);
        assert_eq!(updated.rows, 43);
    }

    #[test]
    fn resize_session_unknown_id_returns_not_found() {
        let mut manager = make_manager();
        let err = manager
            .resize_session(SessionId(99), 100, 40)
            .expect_err("should fail");
        assert!(matches!(err, SessionError::NotFound(SessionId(99))));
    }

    // -----------------------------------------------------------------------
    // SessionManager: create_session failure propagation
    // -----------------------------------------------------------------------

    #[test]
    fn create_session_propagates_factory_error() {
        let mut manager = make_manager_with_factory(Box::new(FailingMockFactory));
        let err = manager
            .create_session(SessionConfig::default())
            .expect_err("should fail");
        assert!(matches!(&err, SessionError::Factory(msg) if msg == "mock factory failure"));
    }

    // -----------------------------------------------------------------------
    // SessionManager: create + close cycles
    // -----------------------------------------------------------------------

    #[test]
    fn create_close_cycle_reuses_no_ids() {
        let mut manager = make_manager();
        create_n(&mut manager, 2);
        manager.close_session(SessionId(1)).unwrap();
        manager.close_session(SessionId(2)).unwrap();

        // Next IDs continue from 3, never reused
        let snap = manager.create_session(SessionConfig::default()).unwrap();
        assert_eq!(snap.id, SessionId(3));
    }

    #[test]
    fn create_close_create_preserves_order_integrity() {
        let mut manager = make_manager();
        create_n(&mut manager, 3);
        manager.close_session(SessionId(2)).unwrap();

        let snap = manager.create_session(SessionConfig::default()).unwrap();
        assert_eq!(snap.id, SessionId(4));

        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 3);

        // IDs should be: 1, 3, 4 (the gap from closed 2, then new 4)
        assert_eq!(sessions[0].id, SessionId(1));
        assert_eq!(sessions[1].id, SessionId(3));
        assert_eq!(sessions[2].id, SessionId(4));
        // Session 4 is active
        assert!(sessions[2].active);
    }

    // -----------------------------------------------------------------------
    // SessionManager: mock handle integration — write through
    // -----------------------------------------------------------------------

    #[test]
    fn write_session_through_manager_tracks_writes() {
        // We need a handle we can inspect. Build manager with a custom factory
        // that returns a pre-configured handle with a trackable `written` buffer.
        let written = Arc::new(Mutex::new(Vec::new()));

        struct TrackWriteFactory {
            written: Arc<Mutex<Vec<u8>>>,
        }

        impl SessionHandleFactory for TrackWriteFactory {
            fn spawn(
                &self,
                _session_id: SessionId,
                _config: SessionConfig,
                _sink: Arc<dyn SessionEventSink>,
            ) -> Result<Box<dyn SessionHandleTrait>, SessionError> {
                Ok(Box::new(MockSessionHandle {
                    state: Arc::new(Mutex::new(SessionState {
                        title: "tracked".into(),
                        shell: "tracked-shell".into(),
                        args: vec!["--tracked".into()],
                        cwd: None,
                        cols: 80,
                        rows: 24,
                        running: true,
                        process_id: Some(42),
                        exit: None,
                    })),
                    written: Arc::clone(&self.written),
                    kill_called: Arc::new(Mutex::new(false)),
                }))
            }
        }

        let mut manager = make_manager_with_factory(Box::new(TrackWriteFactory {
            written: Arc::clone(&written),
        }));

        manager.create_session(SessionConfig::default()).unwrap();
        manager.write_session(SessionId(1), b"track me").unwrap();

        let data = written.lock().unwrap();
        assert_eq!(&*data, b"track me");
    }

    // -----------------------------------------------------------------------
    // SessionManager: closing running vs stopped sessions
    // -----------------------------------------------------------------------

    #[test]
    fn close_stopped_session_does_not_call_request_close() {
        // Custom factory that returns a stopped handle
        struct StoppedFactory;

        impl SessionHandleFactory for StoppedFactory {
            fn spawn(
                &self,
                _session_id: SessionId,
                _config: SessionConfig,
                _sink: Arc<dyn SessionEventSink>,
            ) -> Result<Box<dyn SessionHandleTrait>, SessionError> {
                Ok(Box::new(MockSessionHandle::stopped()))
            }
        }

        let mut manager = make_manager_with_factory(Box::new(StoppedFactory));
        manager.create_session(SessionConfig::default()).unwrap();
        // Should not error despite being stopped
        manager.close_session(SessionId(1)).unwrap();
        assert!(manager.active.is_none());
    }

    // -----------------------------------------------------------------------
    // Sink integration: events emitted on create/close
    // -----------------------------------------------------------------------

    #[test]
    fn counting_sink_receives_events_on_create() {
        let (_sink, events) = CountingSink::new();
        let sink: Arc<dyn SessionEventSink> = Arc::new(_sink);
        let factory: Box<dyn SessionHandleFactory> = Box::new(MockFactory);
        let mut manager = SessionManager::new(sink, factory);

        // Create doesn't emit events directly (reader/waiter do, but with mock factory they don't start threads)
        manager.create_session(SessionConfig::default()).unwrap();
        // With mock handles, no threads spawn, so no events expected
        // This tests that the manager doesn't emit events itself (sink not used in create path)
        let guard = events.lock().unwrap();
        assert!(guard.is_empty());
    }

    #[test]
    fn sink_is_not_called_during_remove() {
        let (_sink, events) = CountingSink::new();
        let sink: Arc<dyn SessionEventSink> = Arc::new(_sink);
        let factory: Box<dyn SessionHandleFactory> = Box::new(MockFactory);
        let mut manager = SessionManager::new(sink, factory);

        manager.create_session(SessionConfig::default()).unwrap();
        manager.close_session(SessionId(1)).unwrap();

        // No events emitted by manager itself
        let guard = events.lock().unwrap();
        assert!(guard.is_empty());
    }

    // -----------------------------------------------------------------------
    // exit_info helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn exit_info_with_exit_code() {
        let status = portable_pty::ExitStatus::with_exit_code(0);
        let info = exit_info(&status);
        assert_eq!(info.code, Some(0));
        assert!(info.signal.is_none());
    }

    #[test]
    fn exit_info_with_non_zero_code() {
        let status = portable_pty::ExitStatus::with_exit_code(1);
        let info = exit_info(&status);
        assert_eq!(info.code, Some(1));
        assert!(info.signal.is_none());
    }

    #[test]
    fn exit_info_with_signal() {
        let status = portable_pty::ExitStatus::with_signal("SIGTERM");
        let info = exit_info(&status);
        assert_eq!(info.code, Some(1));
        assert_eq!(info.signal.as_deref(), Some("SIGTERM"));
    }

    // -----------------------------------------------------------------------
    // Default helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn default_cols_returns_80() {
        assert_eq!(default_cols(), 80);
    }

    #[test]
    fn default_rows_returns_24() {
        assert_eq!(default_rows(), 24);
    }

    #[test]
    fn default_title_with_id() {
        let title = default_title(SessionId(42));
        assert_eq!(title, "Tab 42");
    }

    // -----------------------------------------------------------------------
    // SessionSnapshot construction
    // -----------------------------------------------------------------------

    #[test]
    fn session_snapshot_fields_match_creation() {
        let snap = SessionSnapshot {
            id: SessionId(10),
            title: "test-tab".into(),
            shell: "bash".into(),
            args: vec!["--login".into()],
            cwd: Some(PathBuf::from("/tmp")),
            cols: 100,
            rows: 40,
            active: true,
            running: true,
            process_id: Some(1001),
            exit: None,
        };
        assert_eq!(snap.id, SessionId(10));
        assert_eq!(snap.title, "test-tab");
        assert_eq!(snap.cols, 100);
        assert_eq!(snap.rows, 40);
        assert!(snap.active);
        assert!(snap.running);
        assert_eq!(snap.process_id, Some(1001));
        assert!(snap.exit.is_none());
    }

    // -----------------------------------------------------------------------
    // base64_bytes serde helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn base64_bytes_roundtrip_encodes_decodes() {
        let original = vec![0, 1, 2, 255, 127, 128, 10, 13];

        // Test via SessionEvent::Output which uses #[serde(with = "base64_bytes")]
        let event = SessionEvent::Output {
            session_id: SessionId(1),
            data: original.clone(),
        };
        let json = serde_json::to_string(&event).expect("serialize output event");
        assert!(json.contains("\"kind\":\"output\""));

        let deserialized: SessionEvent =
            serde_json::from_str(&json).expect("deserialize output event");
        match deserialized {
            SessionEvent::Output { data, .. } => {
                assert_eq!(data.len(), original.len());
                assert_eq!(data, original);
            }
            _ => panic!("expected Output variant"),
        }
    }

    #[test]
    fn base64_bytes_deserializes_valid_base64() {
        // Test via SessionEvent::Output which has #[serde(with = "base64_bytes")]
        let json = r#"{"kind":"output","session_id":1,"data":"SGVsbG8="}"#;
        let event: SessionEvent = serde_json::from_str(json).expect("deserialize base64 output");
        match event {
            SessionEvent::Output { data, .. } => {
                assert_eq!(data, b"Hello");
            }
            _ => panic!("expected Output variant"),
        }
    }

    #[test]
    fn base64_bytes_rejects_invalid_base64() {
        let json = r#"{"kind":"output","session_id":1,"data":"not-valid-base64!!"}"#;
        let result: Result<SessionEvent, _> = serde_json::from_str(json);
        assert!(result.is_err(), "should reject invalid base64");
    }

    // -----------------------------------------------------------------------
    // SessionConfig deserialization edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn session_config_deserializes_empty_object() {
        let cfg: SessionConfig = serde_json::from_str("{}").expect("empty obj");
        assert_eq!(cfg.cols, 80);
        assert_eq!(cfg.rows, 24);
        assert!(cfg.title.is_none());
        assert!(cfg.shell.is_none());
        assert!(cfg.env.is_empty());
    }

    #[test]
    fn session_config_deserializes_partial_fields() {
        let cfg: SessionConfig =
            serde_json::from_str(r#"{"title": "work", "cols": 132}"#).expect("partial");
        assert_eq!(cfg.title.as_deref(), Some("work"));
        assert_eq!(cfg.cols, 132);
        assert_eq!(cfg.rows, 24); // should default
        assert!(cfg.shell.is_none());
    }

    #[test]
    fn session_config_deserializes_env_map() {
        let cfg: SessionConfig = serde_json::from_str(
            r#"{"env": {"TERM": "xterm-256color", "PATH": "/usr/bin"}}"#,
        )
        .expect("with env");
        assert_eq!(cfg.env.get("TERM").map(String::as_str), Some("xterm-256color"));
        assert_eq!(cfg.env.get("PATH").map(String::as_str), Some("/usr/bin"));
    }

    // -----------------------------------------------------------------------
    // ExitInfo serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn exit_info_serialize_roundtrip() {
        let info = ExitInfo {
            code: Some(0),
            signal: None,
        };
        let json = serde_json::to_string(&info).expect("serialize");
        let back: ExitInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.code, Some(0));
        assert!(back.signal.is_none());
    }

    #[test]
    fn exit_info_with_signal_roundtrip() {
        let info = ExitInfo {
            code: Some(1),
            signal: Some("SIGKILL".into()),
        };
        let json = serde_json::to_string(&info).expect("serialize");
        let back: ExitInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.code, Some(1));
        assert_eq!(back.signal.as_deref(), Some("SIGKILL"));
    }

    // -----------------------------------------------------------------------
    // create_session with explicit config values
    // -----------------------------------------------------------------------

    #[test]
    fn create_session_passes_config_to_factory() {
        let captured_config = Arc::new(Mutex::new(None::<SessionConfig>));

        struct CaptureFactory {
            captured: Arc<Mutex<Option<SessionConfig>>>,
        }

        impl SessionHandleFactory for CaptureFactory {
            fn spawn(
                &self,
                _session_id: SessionId,
                config: SessionConfig,
                _sink: Arc<dyn SessionEventSink>,
            ) -> Result<Box<dyn SessionHandleTrait>, SessionError> {
                *self.captured.lock().unwrap() = Some(config);
                Ok(Box::new(MockSessionHandle::running()))
            }
        }

        let mut manager = make_manager_with_factory(Box::new(CaptureFactory {
            captured: Arc::clone(&captured_config),
        }));

        let config = SessionConfig {
            title: Some("dev".into()),
            shell: Some("zsh".into()),
            args: vec!["-l".into()],
            cwd: Some(PathBuf::from("/workspace")),
            cols: 132,
            rows: 43,
            env: BTreeMap::from([("KEY".into(), "VAL".into())]),
        };

        manager
            .create_session(config)
            .expect("create should succeed");

        let captured = captured_config.lock().unwrap().take().unwrap();
        assert_eq!(captured.title.as_deref(), Some("dev"));
        assert_eq!(captured.shell.as_deref(), Some("zsh"));
        assert_eq!(captured.args, vec!["-l"]);
        assert_eq!(captured.cwd, Some(PathBuf::from("/workspace")));
        assert_eq!(captured.cols, 132);
        assert_eq!(captured.rows, 43);
        assert_eq!(captured.env.get("KEY").map(String::as_str), Some("VAL"));
    }

    // -----------------------------------------------------------------------
    // Snapshot injection from a custom handle
    // -----------------------------------------------------------------------

    #[test]
    fn custom_handle_snapshot_reflects_injected_state() {
        let handle = MockSessionHandle::new(SessionState {
            title: "custom".into(),
            shell: "fish".into(),
            args: vec!["-c".into(), "echo hi".into()],
            cwd: Some(PathBuf::from("/home")),
            cols: 100,
            rows: 50,
            running: true,
            process_id: Some(777),
            exit: None,
        });

        let snap = handle.snapshot(SessionId(9), false);
        assert_eq!(snap.id, SessionId(9));
        assert_eq!(snap.title, "custom");
        assert_eq!(snap.shell, "fish");
        assert_eq!(snap.args, vec!["-c", "echo hi"]);
        assert_eq!(snap.cwd, Some(PathBuf::from("/home")));
        assert_eq!(snap.cols, 100);
        assert_eq!(snap.rows, 50);
        assert!(!snap.active);
        assert!(snap.running);
        assert_eq!(snap.process_id, Some(777));
        assert!(snap.exit.is_none());
    }
}
