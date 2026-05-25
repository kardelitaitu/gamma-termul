# Architecture

## Recommended Shape
- Tauri app shell
- Rust backend for process and PTY control
- Web UI for tabs, toolbar, selection, and terminal viewport
- `portable-pty` for the cross-platform PTY layer

## Core Runtime Pieces
- `AppState`: window-level state and tab registry
- `TabSession`: one shell process plus its PTY and metadata
- `TerminalView`: frontend renderer for output, selection, and focus
- `ClipboardBridge`: copy/paste bridge between UI and OS clipboard
- `SessionManager`: tab order, active tab, and session lifecycle
- `active_session` command: frontend resync point for the currently focused tab
- `termul:session-event`: global event channel for PTY output, exit, and errors
- PTY output is base64-encoded on the event channel so the payload stays JSON-safe

## Session Model
- One tab equals one shell session.
- Each session owns its own working directory, scrollback, and resize state.
- Closing a tab should cleanly terminate only that tab's process tree.

## Input Rules
- If text is selected, `Ctrl+C` copies the selection.
- If nothing is selected, `Ctrl+C` sends interrupt to the active shell.
- `Ctrl+V` pastes clipboard text into the focused terminal input path.
- When browser/webview shortcuts conflict, use a clear fallback path in the UI.

## Selection Rules
- Selection must be tracked in the terminal renderer, not in the shell.
- Selection should survive normal scrolling and tab switching.
- Copy should use the current selection range, not raw screen text.

## Storage Rules
- Persist tab metadata only.
- Do not persist live PTY process state.
- Restore the last known layout and reopen sessions only if recovery is safe.

## Platform Notes
- Use a PTY layer that supports Windows, macOS, and Linux.
- Keep platform-specific code behind a small Rust abstraction.
- Handle resize events from the frontend and forward them to the active PTY.
- Use Tauri events for background session updates so the UI can subscribe without polling.
