pub mod backup;
pub mod detector;
pub mod discovery;
pub mod patch_manager;

pub use backup::{backup_exists, restore_file, BackupEntry, BackupManifest};
pub use detector::{detect_status, PatchState, TargetStatus};
pub use discovery::{discover_all_targets, ComponentKind, DiscoveredTarget};
pub use patch_manager::{apply_patch, check_file_locked, PatchOperationResult};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentStatusReport {
    pub kind: ComponentKind,
    pub name: String,
    pub files: Vec<String>,
    pub state: PatchState,
    pub has_backup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityPatchReport {
    pub components: Vec<ComponentStatusReport>,
    pub has_backup: bool,
}

pub fn default_backup_dir() -> std::path::PathBuf {
    if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
        std::path::PathBuf::from(appdata).join("Everty").join("antigravity_backups")
    } else if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        std::path::PathBuf::from(home).join(".everty").join("antigravity_backups")
    } else {
        std::env::temp_dir().join("everty_antigravity_backups")
    }
}

pub fn detect_all_installations() -> AntigravityPatchReport {
    let backup_dir = default_backup_dir();
    let targets = discover_all_targets();
    let mut desktop_targets = Vec::new();
    let mut ide_targets = Vec::new();
    let mut cli_targets = Vec::new();

    for t in targets {
        match t.kind {
            ComponentKind::Desktop => desktop_targets.push(t),
            ComponentKind::Ide => ide_targets.push(t),
            ComponentKind::Cli => cli_targets.push(t),
        }
    }

    let check_group = |kind: ComponentKind, name: &str, group: &[DiscoveredTarget]| -> ComponentStatusReport {
        if group.is_empty() {
            return ComponentStatusReport {
                kind,
                name: name.to_string(),
                files: Vec::new(),
                state: PatchState::Unknown,
                has_backup: false,
            };
        }
        let mut any_not_patched = false;
        let mut any_partial = false;
        let mut any_patched = false;
        let mut group_has_backup = false;
        let mut files = Vec::new();

        for t in group {
            files.push(t.name.clone());
            let st = detect_status(t, Some(&backup_dir));
            if backup_exists(&t.path, &backup_dir) {
                group_has_backup = true;
            }
            match st.state {
                PatchState::Patched => any_patched = true,
                PatchState::NotPatched => any_not_patched = true,
                PatchState::Partial | PatchState::Error => any_partial = true,
                PatchState::Unknown | PatchState::NotInstalled => {}
            }
        }

        let state = if any_partial {
            PatchState::Partial
        } else if any_not_patched {
            PatchState::NotPatched
        } else if any_patched {
            PatchState::Patched
        } else {
            PatchState::Unknown
        };

        ComponentStatusReport {
            kind,
            name: name.to_string(),
            files,
            state,
            has_backup: group_has_backup,
        }
    };

    let desktop = check_group(ComponentKind::Desktop, "Antigravity Desktop", &desktop_targets);
    let ide = check_group(ComponentKind::Ide, "Antigravity IDE", &ide_targets);
    let cli = check_group(ComponentKind::Cli, "Antigravity CLI", &cli_targets);

    let has_backup = desktop.has_backup || ide.has_backup || cli.has_backup;
    AntigravityPatchReport {
        components: vec![desktop, ide, cli],
        has_backup,
    }
}

pub fn apply_selected_patches(kinds: &[ComponentKind]) -> Result<Vec<PatchOperationResult>, String> {
    let backup_dir = default_backup_dir();
    let targets = discover_all_targets();
    let mut results = Vec::new();
    for t in targets {
        if kinds.contains(&t.kind) {
            let res = apply_patch(&t, &backup_dir);
            results.push(res);
        }
    }
    Ok(results)
}

pub fn restore_all_backups() -> Result<(), String> {
    let backup_dir = default_backup_dir();
    let targets = discover_all_targets();
    for t in targets {
        if backup_exists(&t.path, &backup_dir) {
            restore_file(&t.path, &backup_dir)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_binary_patch_and_restore_cycle() {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("ag_test_{nonce}"));
        fs::create_dir_all(&temp_dir).unwrap();

        let binary_file = temp_dir.join("language_server.exe");
        let backup_dir = temp_dir.join("backups");

        // 1. Create simulated binary with "ineligible" string
        let original_data = b"HEADER_MOCK_DATA_ineligible_FOOTER_MOCK";
        fs::write(&binary_file, original_data).unwrap();

        let target = DiscoveredTarget {
            kind: ComponentKind::Desktop,
            name: "language_server.exe".into(),
            path: binary_file.clone(),
            is_binary: true,
        };

        // 2. Initial detection: should be NotPatched
        let status = detect_status(&target, Some(&backup_dir));
        assert_eq!(status.state, PatchState::NotPatched);
        assert_eq!(status.original_signature_count, 1);
        assert_eq!(status.modified_signature_count, 0);

        // 3. Apply patch
        let result = apply_patch(&target, &backup_dir);
        assert!(result.success, "apply_patch failed: {}", result.message);

        // 4. Verification: should be Patched
        let patched_data = fs::read(&binary_file).unwrap();
        assert_eq!(&patched_data, b"HEADER_MOCK_DATA_inexigible_FOOTER_MOCK");

        let status_after = detect_status(&target, Some(&backup_dir));
        assert_eq!(status_after.state, PatchState::Patched);
        assert!(status_after.backup_available);

        // 5. Restore from backup
        restore_file(&target.path, &backup_dir).unwrap();

        // 6. Verify restored bytes match original
        let restored_data = fs::read(&binary_file).unwrap();
        assert_eq!(&restored_data, original_data);

        let status_restored = detect_status(&target, Some(&backup_dir));
        assert_eq!(status_restored.state, PatchState::NotPatched);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_js_patch_and_restore_cycle() {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("ag_js_test_{nonce}"));
        fs::create_dir_all(&temp_dir).unwrap();

        let js_file = temp_dir.join("main.js");
        let backup_dir = temp_dir.join("backups");

        let original_js = "console.log('original code');";
        fs::write(&js_file, original_js).unwrap();

        let target = DiscoveredTarget {
            kind: ComponentKind::Ide,
            name: "main.js".into(),
            path: js_file.clone(),
            is_binary: false,
        };

        // Initial check: NotPatched
        let status = detect_status(&target, Some(&backup_dir));
        assert_eq!(status.state, PatchState::NotPatched);

        // Apply patch
        let res = apply_patch(&target, &backup_dir);
        assert!(res.success);

        // Patched check
        let status_patched = detect_status(&target, Some(&backup_dir));
        assert_eq!(status_patched.state, PatchState::Patched);

        let patched_content = fs::read_to_string(&js_file).unwrap();
        assert!(patched_content.contains("// UNLOCKED"));

        // Restore
        restore_file(&target.path, &backup_dir).unwrap();
        let restored_content = fs::read_to_string(&js_file).unwrap();
        assert_eq!(restored_content, original_js);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
