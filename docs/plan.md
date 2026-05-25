# Terminal Product Plan

## Goal
Build a desktop terminal in Rust with Tauri that feels easy to use and reliable.

## Feature Strategy

| Feature | Strategy | Alternative | Risk |
| --- | --- | --- | --- |
| Multiple tabs | Keep one `SessionManager` in Rust and map each tab to one session id. The UI tab bar only mirrors backend state. | Recreate sessions on every tab switch. | Session mix-ups if tab state and process state drift apart. |
| Per-tab shell | Spawn one PTY shell per tab with `portable-pty` and keep read/write/resize logic behind a small Rust wrapper. | Per-OS PTY code. | Platform shell quirks can leak into tab lifecycle. |
| Text selection | Let the terminal renderer own selection state and copy from the visible buffer range, not from shell output. | Copy raw screen text. | Wrapped lines and scrollback can make selection inaccurate. |
| `Ctrl+C` copy or interrupt | Intercept in the terminal viewport first. If selection exists, copy it. If not, send interrupt to the active session. | Always copy, with a separate interrupt shortcut. | Users can lose the expected shell interrupt behavior. |
| `Ctrl+V` paste | Paste plain text into the active session and normalize line endings before write. | Let the browser/webview handle paste by default. | Rich text or focus bugs can break shell input. |
| Tab switching | Switch only the active session pointer and keep inactive sessions alive. Promote the nearest tab on close. | Suspend inactive sessions. | Switching becomes slow or unstable if state is recreated often. |
| Tab close/open | Close only the selected session process tree, then remove its id from the tab order. New tabs open as active by default. | Delay close until process exit is observed. | Orphaned shells or stuck tabs if cleanup is not strict. |
| Tab rename | Store an optional title per tab in portable config and let the tab strip edit titles inline, then refresh from Rust after save. | Use a prompt-only rename flow. | UI and backend can drift if refresh is skipped after a rename. |
| Scrollback | Keep a bounded ring buffer per tab and render from that buffer for scroll and selection. | Unlimited history. | Memory growth and slower rendering over time. |
| Resize | Debounce window resize events and forward the final size to the active PTY immediately. | Resize only on focus changes. | The shell viewport can desync from the UI size. |
| Portable config | Store JSON next to the executable as `{exe}.config` and keep window, terminal, tab directories, and tab titles there. | Use app data folders or registry storage. | The app stops feeling portable if state is split across system locations. |
| Persistence | Save metadata only, not live process state. Restore layout and tab order, not running shells. | Try to restore live sessions. | Recovery becomes unreliable and hard to test. |

## Locked Rules
- Selection wins over interrupt inside the terminal viewport
- `Ctrl+C` copies when terminal selection exists
- `Ctrl+C` sends shell interrupt only when no terminal selection exists
- `Ctrl+V` pastes plain text into the active terminal session

## MVP Scope
- One window with a tab bar
- Create, close, and switch tabs
- Per-tab PTY session
- Scrollback buffer
- Clipboard integration
- Basic resize handling
- Keyboard shortcut routing
- Session manager commands for create, list, activate, resize, write, and close

## Out Of Scope For MVP
- SSH session manager
- Split panes
- Theme editor
- Sync across devices
- Plugins or macros

## Delivery Phases
1. Build the PTY backbone and render one terminal session.
2. Add tab state, session manager commands, and tab lifecycle.
3. Add selection, copy, paste, and shortcut routing.
4. Add scrollback, resize handling, and safe persistence.
5. Test on Windows, macOS, and Linux.

## Success Criteria
- A user can open several tabs without session mix-ups.
- A user can select text and copy it with `Ctrl+C`.
- A user can paste into the terminal with `Ctrl+V`.
- The terminal still supports interrupt behavior when needed.
- Tab switching stays stable under normal use.

## Main Risks
- `Ctrl+C` conflicts with shell interrupt behavior
- Clipboard handling can vary by platform
- Selection logic can break on wrapped lines
- PTY behavior differs across Windows, macOS, and Linux

## Recommended Build Order
1. Session manager and PTY shell spawn.
2. One terminal viewport with output streaming.
3. Tab bar and active-tab switching.
4. Selection and clipboard rules.
5. Scrollback, resize, and persistence.
