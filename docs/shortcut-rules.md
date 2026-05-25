# Shortcut Rules

## Terminal Focus Rules

| Shortcut | Terminal selection | Result |
| --- | --- | --- |
| `Ctrl+C` | Selection exists | Copy selection to clipboard |
| `Ctrl+C` | No selection | Send interrupt to the active shell |
| `Ctrl+V` | Terminal focus | Paste clipboard text into the active session |
| `Ctrl+V` | No terminal focus | Let the focused control handle paste normally |

## Unambiguous Policy
- Selection wins over interrupt in the terminal viewport.
- Interrupt is only used when the terminal has focus and no selection exists.
- Paste always injects plain text, not rich text.
- The terminal layer should intercept these shortcuts before browser default behavior runs.

## Edge Cases
- If the clipboard is empty, `Ctrl+V` should do nothing.
- If the shell is already stopped, `Ctrl+C` should not crash the tab.
- If a non-terminal text field is focused, normal OS copy and paste should work there.

