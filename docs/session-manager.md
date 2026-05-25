# Session Manager

## Purpose
Own tab sessions, shell processes, tab ordering, and the active tab.

## Responsibilities
- Create a new PTY-backed shell session
- Track tab order and active tab
- Route keyboard input to the correct session
- Resize the active PTY when the window changes
- Close one tab without disturbing the others
- Emit session events for output, exit, and errors

## Rust Surface
- `active_session`
- `create_session`
- `list_sessions`
- `set_active_session`
- `rename_session`
- `write_session`
- `resize_session`
- `close_session`

## State Model
- One session equals one PTY process tree
- The manager keeps a tab order list for browser-like navigation
- New tabs become active by default
- Closing the active tab should promote the nearest remaining tab
- The portable config mirrors the tab order as saved directories, saved titles, and the active tab index

## Scaffold Notes
- Output streaming is routed through a small event-sink trait so the frontend wiring can stay separate
- PTY output events are base64-encoded so the webview gets a compact JSON payload
- The session manager stays Tauri-friendly and testable
- Pure tab-order logic is kept separate so it can be unit tested without spawning a shell
- The frontend will listen on `termul:session-event` instead of polling for output
- The frontend can resync the selected tab with `active_session` after focus changes or reloads
- On startup, saved tab directories and titles are reopened in order when the paths still exist
