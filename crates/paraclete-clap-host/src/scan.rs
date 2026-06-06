// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

/// Returns paths to all `.clap` files found in OS-standard directories.
/// Non-existent directories are silently skipped.
pub fn scan_clap_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for dir in clap_search_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("clap") {
                paths.push(path);
            }
        }
    }
    paths
}

#[cfg(target_os = "linux")]
fn clap_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/lib/clap"),
        PathBuf::from("/usr/local/lib/clap"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".clap"));
    }
    dirs
}

#[cfg(target_os = "macos")]
fn clap_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/Library/Audio/Plug-Ins/CLAP")];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
    }
    dirs
}

#[cfg(target_os = "windows")]
fn clap_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    if let Ok(pf) = std::env::var("COMMONPROGRAMFILES") {
        dirs.push(PathBuf::from(pf).join("CLAP"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Programs\\Common\\CLAP"));
    }
    dirs
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn clap_search_dirs() -> Vec<PathBuf> {
    vec![]
}
