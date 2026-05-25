# Portable Config

## Goal
Keep `gamma-termul` portable by storing user tweaks next to the executable.

## File Layout
- Executable: `gamma-termul.exe`
- Config: `gamma-termul.config`
- Path rule: derive the filename from the running exe stem

## Config Schema
- `window`: position, size, maximized state
- `terminal`: font, theme, scrollback, startup directory, default shell
- `tabs`: saved tab directories, saved tab titles, active tab index, and restore preferences

## Save Rules
- Save when the window moves or resizes
- Save when terminal preferences change
- Save when the tab list or active tab changes
- Keep live PTY processes out of the file

## Notes
- JSON is human-readable and easy to diff
- The app should boot with defaults when the file is missing
- Corrupt config should fall back to safe defaults instead of blocking startup
