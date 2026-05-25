# Alternatives

## Terminal Renderer Options

| Option | Pros | Cons | Best Use |
| --- | --- | --- | --- |
| Web terminal renderer in the Tauri webview | Fast to build, strong selection support, good clipboard integration, easy tab UI | More frontend weight, needs careful event routing | Best MVP choice |
| Custom Rust-native renderer | More control, potentially leaner UI path | High complexity, slower delivery, more edge cases | Later only |
| Native OS widget wrapper | Feels native, can integrate with platform APIs | Hard to keep cross-platform, expensive to maintain | Not ideal for MVP |

## PTY Layer Options

| Option | Pros | Cons | Best Use |
| --- | --- | --- | --- |
| `portable-pty` | One shared code path, easier to test, fits Tauri well | Still needs platform-specific handling under the hood | Best MVP choice |
| Hand-written per-OS PTY code | Maximum control | More code, more bugs, slower maintenance | Only if a crate cannot fit |

## Recommendation
- Use the webview terminal renderer for MVP.
- Use Rust for PTY, tabs, process control, and clipboard bridging.
- Keep the architecture simple enough to scale later without rewriting the UI.
