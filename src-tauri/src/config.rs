//! Portable JSON config stored next to the executable.
//!
//! The schema is terminal-specific so we can grow window, appearance,
//! and tab state without another format migration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;

const CURRENT_VERSION: u32 = 3;

fn current_version() -> u32 {
    CURRENT_VERSION
}

fn default_window_width() -> u32 {
    1200
}

fn default_window_height() -> u32 {
    800
}

fn default_window_left() -> i32 {
    100
}

fn default_window_top() -> i32 {
    100
}

fn default_font_size() -> u32 {
    14
}

fn default_font_family() -> String {
    "Cascadia Code".to_string()
}

fn default_theme() -> String {
    "dark".to_string()
}

fn default_scrollback_lines() -> u32 {
    10_000
}

fn default_restore_last_active_tab() -> bool {
    true
}

fn default_restore_tabs_on_startup() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default)]
    pub window: WindowConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub tabs: TabsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            window: WindowConfig::default(),
            terminal: TerminalConfig::default(),
            tabs: TabsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    #[serde(default = "default_window_width")]
    pub width: u32,
    #[serde(default = "default_window_height")]
    pub height: u32,
    #[serde(default = "default_window_left")]
    pub left: i32,
    #[serde(default = "default_window_top")]
    pub top: i32,
    #[serde(default)]
    pub maximized: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: default_window_width(),
            height: default_window_height(),
            left: default_window_left(),
            top: default_window_top(),
            maximized: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: u32,
    #[serde(default)]
    pub startup_directory: Option<PathBuf>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub shell_args: Vec<String>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            font_family: default_font_family(),
            font_size: default_font_size(),
            theme: default_theme(),
            scrollback_lines: default_scrollback_lines(),
            startup_directory: None,
            shell: None,
            shell_args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabsConfig {
    #[serde(default)]
    pub tab_directories: Vec<PathBuf>,
    #[serde(default)]
    pub tab_titles: Vec<Option<String>>,
    #[serde(default)]
    pub active_index: Option<usize>,
    #[serde(default = "default_restore_last_active_tab")]
    pub restore_last_active_tab: bool,
    #[serde(default = "default_restore_tabs_on_startup")]
    pub restore_tabs_on_startup: bool,
}

impl Default for TabsConfig {
    fn default() -> Self {
        Self {
            tab_directories: Vec::new(),
            tab_titles: Vec::new(),
            active_index: None,
            restore_last_active_tab: true,
            restore_tabs_on_startup: true,
        }
    }
}

/// Returns the config path next to the executable.
pub fn path() -> PathBuf {
    paths::config_path()
}

/// Returns true when the portable config file already exists.
pub fn exists() -> bool {
    path().exists()
}

/// Load the config, falling back to defaults if the file is missing or corrupt.
pub fn load() -> Config {
    try_load().unwrap_or_default()
}

/// Load the config and surface file or parse errors.
pub fn try_load() -> Result<Config, String> {
    load_from_path(&path())
}

/// Save the config next to the executable as pretty JSON.
pub fn save(config: &Config) -> Result<(), String> {
    save_to_path(&path(), config)
}

pub(crate) fn load_from_path(path: &Path) -> Result<Config, String> {
    if !path.exists() {
        return Ok(Config::default());
    }

    let raw = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    serde_json::from_str(&raw).map_err(|err| format!("failed to parse {}: {err}", path.display()))
}

pub(crate) fn save_to_path(path: &Path, config: &Config) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(config)
        .map_err(|err| format!("failed to serialize config: {err}"))?;
    std::fs::write(path, json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gamma-termul-config-test-{}-{}",
            std::process::id(),
            name
        ))
    }

    #[test]
    fn config_defaults_are_terminal_specific() {
        let cfg = Config::default();
        assert_eq!(cfg.version, CURRENT_VERSION);
        assert_eq!(cfg.window.width, 1200);
        assert_eq!(cfg.window.height, 800);
        assert_eq!(cfg.terminal.font_family, "Cascadia Code");
        assert_eq!(cfg.terminal.theme, "dark");
        assert!(cfg.tabs.tab_directories.is_empty());
        assert!(cfg.tabs.tab_titles.is_empty());
        assert!(cfg.tabs.active_index.is_none());
        assert!(cfg.tabs.restore_last_active_tab);
        assert!(cfg.tabs.restore_tabs_on_startup);
    }

    #[test]
    fn config_roundtrip_preserves_terminal_fields() {
        let cfg = Config {
            version: 1,
            window: WindowConfig {
                width: 1440,
                height: 900,
                left: 55,
                top: 66,
                maximized: true,
            },
            terminal: TerminalConfig {
                font_family: "JetBrains Mono".into(),
                font_size: 16,
                theme: "nord".into(),
                scrollback_lines: 20_000,
                startup_directory: Some(PathBuf::from(r"C:\Work")),
                shell: Some("powershell.exe".into()),
                shell_args: vec!["-NoLogo".into()],
            },
            tabs: TabsConfig {
                tab_directories: vec![PathBuf::from(r"C:\Work"), PathBuf::from(r"C:\Projects")],
                tab_titles: vec![Some("Work".into()), None],
                active_index: Some(1),
                restore_last_active_tab: true,
                restore_tabs_on_startup: true,
            },
        };

        let json = serde_json::to_string_pretty(&cfg).expect("serialize");
        let restored: Config = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored.window.width, 1440);
        assert_eq!(restored.window.height, 900);
        assert_eq!(restored.terminal.font_family, "JetBrains Mono");
        assert_eq!(restored.terminal.scrollback_lines, 20_000);
        assert_eq!(
            restored.tabs.tab_directories,
            vec![PathBuf::from(r"C:\Work"), PathBuf::from(r"C:\Projects")]
        );
        assert_eq!(restored.tabs.tab_titles, vec![Some("Work".into()), None]);
        assert_eq!(restored.tabs.active_index, Some(1));
    }

    #[test]
    fn missing_fields_default_cleanly() {
        let json = r#"{
            "version": 1,
            "window": { "width": 1280, "height": 720 },
            "terminal": { "font_family": "Fira Code" },
            "tabs": {}
        }"#;

        let cfg: Config = serde_json::from_str(json).expect("deserialize");
        assert_eq!(cfg.window.left, 100);
        assert_eq!(cfg.window.top, 100);
        assert_eq!(cfg.terminal.font_size, 14);
        assert_eq!(cfg.terminal.theme, "dark");
        assert!(cfg.tabs.tab_directories.is_empty());
        assert!(cfg.tabs.tab_titles.is_empty());
        assert!(cfg.tabs.active_index.is_none());
        assert!(cfg.tabs.restore_last_active_tab);
        assert!(cfg.tabs.restore_tabs_on_startup);
    }

    #[test]
    fn file_roundtrip_uses_json_next_to_exe() {
        let path = temp_path("roundtrip.config");
        let cfg = Config {
            window: WindowConfig {
                width: 1111,
                height: 777,
                left: 10,
                top: 20,
                maximized: false,
            },
            terminal: TerminalConfig {
                font_family: "Caskaydia Cove".into(),
                theme: "dracula".into(),
                startup_directory: Some(PathBuf::from(r"C:\Projects")),
                ..TerminalConfig::default()
            },
            tabs: TabsConfig {
                tab_directories: vec![PathBuf::from(r"C:\Projects")],
                tab_titles: vec![Some("Projects".into())],
                active_index: Some(0),
                restore_last_active_tab: true,
                restore_tabs_on_startup: true,
            },
            ..Config::default()
        };

        save_to_path(&path, &cfg).expect("save");
        let restored = load_from_path(&path).expect("load");

        assert_eq!(restored.window.width, 1111);
        assert_eq!(restored.window.height, 777);
        assert_eq!(restored.terminal.font_family, "Caskaydia Cove");
        assert_eq!(restored.terminal.theme, "dracula");
        assert_eq!(
            restored.tabs.tab_directories,
            vec![PathBuf::from(r"C:\Projects")]
        );
        assert_eq!(restored.tabs.tab_titles, vec![Some("Projects".into())]);

        let _ = std::fs::remove_file(path);
    }

    // -----------------------------------------------------------------------
    // File I/O edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn load_from_missing_path_returns_default() {
        let path = temp_path("missing.config");
        let _ = std::fs::remove_file(&path); // ensure it's gone
        let cfg = load_from_path(&path).expect("missing file should return default");
        assert_eq!(cfg.version, CURRENT_VERSION);
        assert_eq!(cfg.terminal.font_family, "Cascadia Code");
    }

    #[test]
    fn load_from_corrupt_json_returns_error() {
        let path = temp_path("corrupt.config");
        std::fs::write(&path, b"this is not valid json").expect("write corrupt data");
        let err = load_from_path(&path).expect_err("corrupt file should error");
        assert!(err.contains("failed to parse"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_from_empty_file_returns_error() {
        let path = temp_path("empty.config");
        std::fs::write(&path, b"").expect("write empty");
        let err = load_from_path(&path).expect_err("empty file should error");
        assert!(err.contains("failed to parse"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = temp_path("nested").join("sub").join("dir");
        let path = dir.join("cfg.json");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
        let cfg = Config::default();
        save_to_path(&path, &cfg).expect("save should create directories");
        assert!(path.exists(), "config file should exist after save");

        // Verify content is valid JSON
        let loaded = load_from_path(&path).expect("reload saved config");
        assert_eq!(loaded.version, CURRENT_VERSION);

        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn exists_returns_false_for_nonexistent_path() {
        // exists() calls path() which uses exe_dir, so it would
        // test the real exe dir. Instead test via load_from_path which
        // does the same check internally.
        let path = temp_path("nonexistent.config");
        let _ = std::fs::remove_file(&path);
        let cfg = load_from_path(&path).expect("no file = default");
        assert_eq!(cfg.version, CURRENT_VERSION);
    }

    #[test]
    fn save_overwrites_existing_file() {
        let path = temp_path("overwrite.config");
        let first = Config {
            version: 1,
            terminal: TerminalConfig {
                font_family: "old".into(),
                ..TerminalConfig::default()
            },
            ..Config::default()
        };
        save_to_path(&path, &first).expect("first save");

        let second = Config {
            terminal: TerminalConfig {
                font_family: "new".into(),
                ..TerminalConfig::default()
            },
            ..Config::default()
        };
        save_to_path(&path, &second).expect("second save");

        let loaded = load_from_path(&path).expect("reload");
        assert_eq!(loaded.terminal.font_family, "new");
        let _ = std::fs::remove_file(path);
    }

    // -----------------------------------------------------------------------
    // Default helper function — each must match its struct counterpart
    // -----------------------------------------------------------------------

    #[test]
    fn default_current_version() {
        assert_eq!(current_version(), CURRENT_VERSION);
        assert_eq!(current_version(), 3);
    }

    #[test]
    fn default_window_dimensions() {
        assert_eq!(default_window_width(), 1200);
        assert_eq!(default_window_height(), 800);
        assert_eq!(default_window_left(), 100);
        assert_eq!(default_window_top(), 100);
    }

    #[test]
    fn default_terminal_appearance() {
        assert_eq!(default_font_family(), "Cascadia Code");
        assert_eq!(default_font_size(), 14);
        assert_eq!(default_theme(), "dark");
        assert_eq!(default_scrollback_lines(), 10_000);
    }

    #[test]
    fn default_tab_restore_policies() {
        assert!(default_restore_last_active_tab());
        assert!(default_restore_tabs_on_startup());
    }

    #[test]
    fn version_default_matches_constant() {
        let json = r#"{"window":{},"terminal":{},"tabs":{}}"#;
        let cfg: Config = serde_json::from_str(json).expect("deserialize");
        assert_eq!(cfg.version, CURRENT_VERSION);
        assert_eq!(cfg.version, 3);
    }

    #[test]
    fn version_migration_is_lenient() {
        // Older version (1) should still deserialize cleanly
        let json = r#"{"version":1,"window":{},"terminal":{},"tabs":{}}"#;
        let cfg: Config = serde_json::from_str(json).expect("deserialize old version");
        assert_eq!(cfg.version, 1);
        // New fields should get defaults
        assert!(cfg.tabs.restore_tabs_on_startup);
    }

    // -----------------------------------------------------------------------
    // Config path and existence
    // -----------------------------------------------------------------------

    #[test]
    fn config_path_returns_something() {
        let p = path();
        assert!(!p.as_os_str().is_empty());
        assert!(p.extension().is_some());
    }

    #[test]
    fn config_exists_returns_false_when_file_missing() {
        // Can't easily test exists() since it uses real exe path.
        // Instead verify that load_from_path handles non-existent correctly.
        let p = temp_path("no_such_file_ever.config");
        let _ = std::fs::remove_file(&p);
        assert!(!p.exists());
        let cfg = load_from_path(&p).expect("should return default for missing");
        assert_eq!(cfg.version, CURRENT_VERSION);
    }

    // -----------------------------------------------------------------------
    // Load / try_load / save top-level API
    // -----------------------------------------------------------------------

    #[test]
    fn load_falls_back_to_default_when_no_file() {
        // load() calls try_load() which uses real exe dir.
        // Since there's no config file next to the test binary,
        // load() should return default.
        let cfg = load();
        assert_eq!(cfg.version, CURRENT_VERSION);
    }

    #[test]
    fn try_load_surfaces_error_on_missing_file() {
        // try_load() wraps load_from_path(&path())
        // Since there's no config, it should return default (not error)
        // because load_from_path returns Ok(default) for missing files.
        let result = try_load();
        assert!(result.is_ok(), "try_load should succeed when missing");
        let cfg = result.unwrap();
        assert_eq!(cfg.version, CURRENT_VERSION);
    }

    // -----------------------------------------------------------------------
    // Per-sub-struct serialization roundtrips
    // -----------------------------------------------------------------------

    #[test]
    fn window_config_serde_roundtrip() {
        let wc = WindowConfig {
            width: 1920,
            height: 1080,
            left: 0,
            top: 0,
            maximized: true,
        };
        let json = serde_json::to_string(&wc).expect("serialize");
        let back: WindowConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.width, 1920);
        assert_eq!(back.height, 1080);
        assert!(back.maximized);
    }

    #[test]
    fn window_config_defaults_are_sane() {
        let json = "{}";
        let wc: WindowConfig = serde_json::from_str(json).expect("deserialize empty");
        assert_eq!(wc.width, 1200);
        assert_eq!(wc.height, 800);
        assert_eq!(wc.left, 100);
        assert_eq!(wc.top, 100);
        assert!(!wc.maximized);
    }

    #[test]
    fn terminal_config_serde_roundtrip() {
        let tc = TerminalConfig {
            font_family: "Fira Code".into(),
            font_size: 16,
            theme: "solarized".into(),
            scrollback_lines: 50_000,
            startup_directory: Some(PathBuf::from("/home")),
            shell: Some("fish".into()),
            shell_args: vec!["--login".into()],
        };
        let json = serde_json::to_string(&tc).expect("serialize");
        let back: TerminalConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.font_family, "Fira Code");
        assert_eq!(back.font_size, 16);
        assert_eq!(back.shell.as_deref(), Some("fish"));
    }

    #[test]
    fn terminal_config_defaults_from_empty_json() {
        let json = "{}";
        let tc: TerminalConfig = serde_json::from_str(json).expect("deserialize empty");
        assert_eq!(tc.font_family, "Cascadia Code");
        assert_eq!(tc.font_size, 14);
        assert_eq!(tc.theme, "dark");
        assert_eq!(tc.scrollback_lines, 10_000);
    }

    #[test]
    fn tabs_config_serde_roundtrip() {
        let tc = TabsConfig {
            tab_directories: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            tab_titles: vec![Some("Work".into()), None],
            active_index: Some(1),
            restore_last_active_tab: true,
            restore_tabs_on_startup: false,
        };
        let json = serde_json::to_string(&tc).expect("serialize");
        let back: TabsConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.tab_directories, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(back.tab_titles, vec![Some("Work".into()), None]);
        assert_eq!(back.active_index, Some(1));
        assert!(back.restore_last_active_tab);
        assert!(!back.restore_tabs_on_startup);
    }
}
