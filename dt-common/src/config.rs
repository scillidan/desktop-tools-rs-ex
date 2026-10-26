//! Exe-relative config file helpers: load-or-create next to the exe, mtime
//! polling for hot-reload. Provenance: extracted from desktop-tools-rs
//! src/config.rs (repo folder window-tools-rs), daemon-specific parts
//! (bindings, presets, migration) left behind.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// `path` if it exists, else the default TOML written to `path` first.
pub fn load_or_create(path: &Path, default_toml: &str) -> Result<String> {
    if !path.exists() {
        std::fs::write(path, default_toml)
            .with_context(|| format!("cannot create default {}", path.display()))?;
    }
    std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))
}

/// `<exe_dir>/name` — config lives next to the exe.
pub fn exe_dir_file(name: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot locate own executable")?;
    let dir = exe.parent().context("executable has no parent directory")?;
    Ok(dir.join(name))
}

/// Modification time for the 1 s hot-reload poll. mtime only: FAT/exFAT has
/// 2 s granularity, so a second save inside that window can be missed.
pub fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_or_create_roundtrip() {
        let dir = std::env::temp_dir().join(format!("dt-common-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dt-ocr.toml");
        let _ = std::fs::remove_file(&path);

        let text = load_or_create(&path, "x = 1\n").unwrap();
        assert_eq!(text, "x = 1\n");
        assert!(path.exists());
        std::fs::write(&path, "y = 2\n").unwrap();
        assert_eq!(load_or_create(&path, "x = 1\n").unwrap(), "y = 2\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exe_dir_file_is_absolute_and_named() {
        let p = exe_dir_file("dt-ocr.toml").unwrap();
        assert!(p.is_absolute());
        assert_eq!(p.file_name().unwrap(), "dt-ocr.toml");
    }

    #[test]
    fn modified_tracks_writes() {
        let dir = std::env::temp_dir().join(format!("dt-common-mtime-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.toml");
        assert!(modified(&path).is_none());
        std::fs::write(&path, "a = 1\n").unwrap();
        assert!(modified(&path).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
