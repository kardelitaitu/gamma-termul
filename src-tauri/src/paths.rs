//! Portable file paths derived from the running executable.
//!
//! The app keeps its config next to the `.exe`, so moving the binary
//! moves the settings with it.

use std::path::PathBuf;

/// Returns the executable filename without extension.
pub fn exe_stem() -> String {
    std::env::current_exe()
        .expect("failed to get executable path")
        .file_stem()
        .expect("failed to get executable stem")
        .to_string_lossy()
        .to_string()
}

/// Returns the directory that contains the executable.
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .expect("failed to get executable path")
        .parent()
        .expect("failed to get executable parent")
        .to_path_buf()
}

/// Returns the JSON config path next to the executable.
pub fn config_path() -> PathBuf {
    exe_dir().join(format!("{}.config", exe_stem()))
}

/// Lightweight startup check so we fail early if the executable path is unavailable.
pub fn verify() -> Result<(), String> {
    let _ = exe_stem();
    let _ = exe_dir();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_stem_is_present() {
        let stem = exe_stem();
        assert!(!stem.is_empty());
        assert!(!stem.contains('.'));
    }

    #[test]
    fn exe_dir_is_absolute() {
        assert!(exe_dir().is_absolute());
    }

    #[test]
    fn config_path_ends_with_config() {
        let path = config_path();
        assert!(path.to_string_lossy().ends_with(".config"));
        assert_eq!(path.parent(), Some(exe_dir().as_path()));
    }

    #[test]
    fn verify_returns_ok() {
        assert!(verify().is_ok());
    }

    #[test]
    fn exe_stem_has_no_extension() {
        let stem = exe_stem();
        assert!(!stem.is_empty());
        assert!(
            !stem.contains('.'),
            "stem should not contain dots, got: {stem}"
        );
    }
}
