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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
    pub active: bool,
    pub running: bool,
    pub process_id: Option<u32>,
    pub exit: Option<ExitInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    Output { session_id: SessionId, data: Vec<u8> },
    Exit { session_id: SessionId, exit: ExitInfo },
    Error { session_id: SessionId, message: String },
}

pub trait SessionEventSink: Send + Sync {
    fn emit(&self, event: SessionEvent);
}

pub struct NoopSessionSink;

impl SessionEventSink for NoopSessionSink {
    fn emit(&self, _event: SessionEvent) {}
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
}

pub struct SessionManager {
    next_id: u64,
    active: Option<SessionId>,
    order: Vec<SessionId>,
    sessions: HashMap<SessionId, SessionHandle>,
    sink: Arc<dyn SessionEventSink>,
}

impl SessionManager {
    pub fn new(sink: Arc<dyn SessionEventSink>) -> Self {
        Self {
            next_id: 1,
            active: None,
            order: Vec::new(),
            sessions: HashMap::new(),
            sink,
        }
    }

    pub fn create_session(&mut self, config: SessionConfig) -> Result<SessionSnapshot, SessionError> {
        let session_id = SessionId(self.next_id);
        self.next_id += 1;

        let handle = SessionHandle::spawn(session_id, config, Arc::clone(&self.sink))?;
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

    pub fn set_active_session(&mut self, session_id: SessionId) -> Result<SessionSnapshot, SessionError> {
        if !self.sessions.contains_key(&session_id) {
            return Err(SessionError::NotFound(session_id));
        }

        self.active = Some(session_id);
        self.snapshot_for(session_id, true)
    }

    pub fn write_session(&mut self, session_id: SessionId, input: &[u8]) -> Result<(), SessionError> {
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

        self.order
            .get(index + 1)
            .copied()
            .or_else(|| index.checked_sub(1).and_then(|prev| self.order.get(prev).copied()))
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
        let mut command = CommandBuilder::new(shell);
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
        let reader = master
            .try_clone_reader()
            .map_err(|err| {
                let _ = child.kill();
                SessionError::Pty(err.to_string())
            })?;
        let writer = master
            .take_writer()
            .map_err(|err| {
                let _ = child.kill();
                SessionError::Pty(err.to_string())
            })?;

        let state = Arc::new(Mutex::new(SessionState {
            title: title.unwrap_or_else(|| default_title(session_id)),
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

    fn snapshot(&self, session_id: SessionId, active: bool) -> SessionSnapshot {
        let state = self.state.lock().map(|state| state.clone()).unwrap_or_else(|_| SessionState {
            title: default_title(session_id),
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
        killer.kill().map_err(|err| SessionError::Pty(err.to_string()))
    }

    fn is_running(&self) -> bool {
        self.state.lock().map(|state| state.running).unwrap_or(false)
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

    #[test]
    fn active_tab_moves_to_next_neighbor_when_closing_current() {
        let order = [SessionId(1), SessionId(2), SessionId(3)];
        let active = Some(SessionId(2));
        let next = choose_active_after_close(&order, active, SessionId(2));

        assert_eq!(next, Some(SessionId(3)));
    }

    #[test]
    fn active_tab_moves_back_when_no_right_neighbor_exists() {
        let order = [SessionId(1), SessionId(2)];
        let active = Some(SessionId(2));
        let next = choose_active_after_close(&order, active, SessionId(2));

        assert_eq!(next, Some(SessionId(1)));
    }

    #[test]
    fn closing_last_tab_clears_active_session() {
        let order = [SessionId(1)];
        let active = Some(SessionId(1));
        let next = choose_active_after_close(&order, active, SessionId(1));

        assert_eq!(next, None);
    }

    #[test]
    fn non_active_close_keeps_current_active_session() {
        let order = [SessionId(1), SessionId(2), SessionId(3)];
        let active = Some(SessionId(1));
        let next = choose_active_after_close(&order, active, SessionId(3));

        assert_eq!(next, Some(SessionId(1)));
    }

    fn choose_active_after_close(
        order: &[SessionId],
        active: Option<SessionId>,
        closing: SessionId,
    ) -> Option<SessionId> {
        if active != Some(closing) {
            return active;
        }

        let index = order.iter().position(|id| *id == closing)?;
        order
            .get(index + 1)
            .copied()
            .or_else(|| index.checked_sub(1).and_then(|prev| order.get(prev).copied()))
    }
}
