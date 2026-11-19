use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComponentKind {
    Desktop,
    Ide,
    Cli,
}

impl ComponentKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Desktop => "Antigravity Desktop",
            Self::Ide => "Antigravity IDE",
            Self::Cli => "Antigravity CLI",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiscoveredTarget {
    pub kind: ComponentKind,
    pub name: String,
    pub path: PathBuf,
    pub is_binary: bool,
}

/// Search for installed Antigravity Desktop, IDE, and CLI components.
pub fn discover_all_targets() -> Vec<DiscoveredTarget> {
    let mut targets = Vec::new();
    let mut seen_paths = HashSet::new();

    discover_desktop(&mut targets, &mut seen_paths);
    discover_ide(&mut targets, &mut seen_paths);
    discover_cli(&mut targets, &mut seen_paths);

    targets
}

fn add_target_if_exists(
    targets: &mut Vec<DiscoveredTarget>,
    seen: &mut HashSet<PathBuf>,
    kind: ComponentKind,
    name: &str,
    path: PathBuf,
    is_binary: bool,
) {
    if path.is_file() {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.insert(canonical) {
            targets.push(DiscoveredTarget {
                kind,
                name: name.to_string(),
                path,
                is_binary,
            });
        }
    }
}

fn discover_desktop(targets: &mut Vec<DiscoveredTarget>, seen: &mut HashSet<PathBuf>) {
    let mut base_dirs = Vec::new();

    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(&local_app_data);
        base_dirs.push(p.join("Programs").join("Antigravity"));
        base_dirs.push(p.join("Antigravity"));
    }
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        base_dirs.push(PathBuf::from(program_files).join("Antigravity"));
    }
    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        base_dirs.push(PathBuf::from(program_files_x86).join("Antigravity"));
    }

    for base in base_dirs {
        add_target_if_exists(
            targets,
            seen,
            ComponentKind::Desktop,
            "language_server.exe (Desktop)",
            base.join("resources").join("bin").join("language_server.exe"),
            true,
        );
    }
}

fn discover_ide(targets: &mut Vec<DiscoveredTarget>, seen: &mut HashSet<PathBuf>) {
    let mut base_dirs = Vec::new();

    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(&local_app_data);
        base_dirs.push(p.join("Programs").join("Antigravity IDE"));
        base_dirs.push(p.join("Antigravity IDE"));
    }
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        base_dirs.push(PathBuf::from(program_files).join("Antigravity IDE"));
    }
    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        base_dirs.push(PathBuf::from(program_files_x86).join("Antigravity IDE"));
    }

    for base in &base_dirs {
        // Main.js
        add_target_if_exists(
            targets,
            seen,
            ComponentKind::Ide,
            "main.js (IDE core)",
            base.join("resources").join("app").join("out").join("main.js"),
            false,
        );

        // Extension.js
        add_target_if_exists(
            targets,
            seen,
            ComponentKind::Ide,
            "extension.js (IDE extension)",
            base.join("resources")
                .join("app")
                .join("extensions")
                .join("antigravity")
                .join("dist")
                .join("extension.js"),
            false,
        );

        // Language server in extension
        add_target_if_exists(
            targets,
            seen,
            ComponentKind::Ide,
            "language_server_windows_x64.exe (IDE backend)",
            base.join("resources")
                .join("app")
                .join("extensions")
                .join("antigravity")
                .join("bin")
                .join("language_server_windows_x64.exe"),
            true,
        );

        // Standalone language server in bin
        add_target_if_exists(
            targets,
            seen,
            ComponentKind::Ide,
            "language_server.exe (IDE backend)",
            base.join("resources").join("bin").join("language_server.exe"),
            true,
        );
    }

    // New IDE backend in %USERPROFILE%\.gemini\bin\agy.exe
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(user_profile);
        add_target_if_exists(
            targets,
            seen,
            ComponentKind::Ide,
            "agy.exe (.gemini backend)",
            p.join(".gemini").join("bin").join("agy.exe"),
            true,
        );
    }
}

fn discover_cli(targets: &mut Vec<DiscoveredTarget>, seen: &mut HashSet<PathBuf>) {
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(&local_app_data);
        add_target_if_exists(
            targets,
            seen,
            ComponentKind::Cli,
            "agy.exe (CLI)",
            p.join("agy").join("agy.exe"),
            true,
        );
        add_target_if_exists(
            targets,
            seen,
            ComponentKind::Cli,
            "agy.exe (CLI bin)",
            p.join("agy").join("bin").join("agy.exe"),
            true,
        );
    }
}

/// Helper to discover a custom directory or file if manual selection is made.
pub fn probe_custom_path(path: &Path) -> Option<DiscoveredTarget> {
    if !path.exists() {
        return None;
    }
    if path.is_file() {
        let name = path.file_name()?.to_string_lossy();
        let is_binary = name.ends_with(".exe");
        let kind = if name.contains("agy") {
            ComponentKind::Cli
        } else if name.contains("extension") || name.contains("main.js") {
            ComponentKind::Ide
        } else {
            ComponentKind::Desktop
        };
        return Some(DiscoveredTarget {
            kind,
            name: name.to_string(),
            path: path.to_path_buf(),
            is_binary,
        });
    }
    None
}
