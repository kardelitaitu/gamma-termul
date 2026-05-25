# Session Manager

## Purpose
Own tab sessions, shell processes, tab ordering, and the active tab.

## Responsibilities
- Create a new PTY-backed shell session
- Track tab order and active tab
- Route keyboard input to the correct session
- Resize the active PTY when the window changes
- Close one tab without disturbing the others

## Rust Surface
- `create_session`
- `list_sessions`
- `set_active_session`
- `write_session`
- `resize_session`
- `close_session`

## State Model
- One session equals one PTY process tree
- The manager keeps a tab order list for browser-like navigation
- New tabs become active by default
- Closing the active tab should promote the nearest remaining tab

## Scaffold Notes
- Output streaming is routed through a small event-sink trait so the frontend wiring can stay separate
- The session manager stays Tauri-friendly and testable
- Pure tab-order logic is kept separate so it can be unit tested without spawning a shell

