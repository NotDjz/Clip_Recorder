//! Where the app keeps its files. The install folder (Program Files) is read-only for the user,
//! so config.json and the log live in %APPDATA%\ClipRecorder.

use std::path::PathBuf;

/// `CLIPREC_DATA_DIR` if set and not empty (tests), else `%APPDATA%\ClipRecorder`.
pub fn data_dir() -> PathBuf {
    match std::env::var_os("CLIPREC_DATA_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("APPDATA").unwrap_or_else(|| ".".into())).join("ClipRecorder"),
    }
}

pub fn config_file() -> PathBuf {
    data_dir().join("config.json")
}

pub fn log_file() -> PathBuf {
    data_dir().join("clip_recorder.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_lives_in_appdata_unless_overridden() {
        // One test owns the variable: tests run in parallel threads of one process.
        unsafe { std::env::remove_var("CLIPREC_DATA_DIR") };
        let appdata = std::env::var("APPDATA").unwrap();
        assert_eq!(data_dir(), PathBuf::from(appdata).join("ClipRecorder"));

        unsafe { std::env::set_var("CLIPREC_DATA_DIR", r"C:\somewhere") };
        assert_eq!(config_file(), PathBuf::from(r"C:\somewhere\config.json"));
        unsafe { std::env::set_var("CLIPREC_DATA_DIR", "") };
        assert!(data_dir().ends_with("ClipRecorder"), "an empty override must fall back to %APPDATA%");
        unsafe { std::env::remove_var("CLIPREC_DATA_DIR") };
    }
}
