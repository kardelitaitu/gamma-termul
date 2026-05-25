# PTY Backend Choice

## Decision
Use `portable-pty` as the backend for all supported desktop platforms.

## Why This Choice
- One Rust API for Windows, macOS, and Linux
- Smaller surface area than maintaining per-OS PTY code
- Good fit for a Tauri backend that needs reliable tab isolation
- Keeps the app easy to extend later with session restore and tab lifecycle logic

## Alternatives

| Option | Pros | Cons | Verdict |
| --- | --- | --- | --- |
| `portable-pty` | Cross-platform, simpler code, good MVP fit | Still hides OS-specific quirks under the hood | Pick for MVP |
| Custom per-OS PTY layer | Maximum control | More code, harder maintenance, more bug risk | Skip for now |
| Native shell bridge without PTY | Very simple at first glance | Weak terminal behavior, poor UX for a real terminal app | Not suitable |

## Practical Implication
- The session manager should treat the PTY as the source of truth for input, output, resize, and process lifetime.
- Platform-specific behavior should stay behind a small Rust abstraction so the UI stays clean.

