use super::detector::compute_sha256;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    pub original_path: PathBuf,
    pub original_sha256: String,
    pub backup_filename: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackupManifest {
    pub version: u32,
    pub entries: HashMap<String, BackupEntry>, // Keyed by canonical original_path string
}

impl BackupManifest {
    pub fn load(dir: &Path) -> Self {
        let manifest_file = dir.join("manifest.json");
        if let Ok(data) = fs::read_to_string(&manifest_file) {
            if let Ok(manifest) = serde_json::from_str::<Self>(&data) {
                return manifest;
            }
        }
        Self {
            version: 1,
            entries: HashMap::new(),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let manifest_file = dir.join("manifest.json");
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Ошибка сериализации манифеста: {e}"))?;
        fs::write(&manifest_file, data)
            .map_err(|e| format!("Не удалось сохранить манифест резервных копий: {e}"))
    }
}

pub fn key_for_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}

pub fn backup_exists(original_path: &Path, backup_dir: &Path) -> bool {
    let manifest = BackupManifest::load(backup_dir);
    let key = key_for_path(original_path);
    if let Some(entry) = manifest.entries.get(&key) {
        let backup_file = backup_dir.join(&entry.backup_filename);
        backup_file.is_file()
    } else {
        false
    }
}

/// Create a backup of the original file if not already backed up.
/// Returns Ok(backup_path).
pub fn create_backup(original_path: &Path, backup_dir: &Path) -> Result<PathBuf, String> {
    if !original_path.is_file() {
        return Err("Файл для резервной копии не существует".into());
    }

    fs::create_dir_all(backup_dir)
        .map_err(|e| format!("Не удалось создать папку резервных копий: {e}"))?;

    let mut manifest = BackupManifest::load(backup_dir);
    let key = key_for_path(original_path);

    let sha256 = compute_sha256(original_path)?;

    // If an existing backup already exists in manifest, check if it's on disk
    if let Some(entry) = manifest.entries.get(&key) {
        let existing_backup = backup_dir.join(&entry.backup_filename);
        if existing_backup.is_file() {
            // Already safely backed up, do not overwrite with modified file
            return Ok(existing_backup);
        }
    }

    let backup_filename = format!("{}.bak", &sha256[..16]);
    let backup_file = backup_dir.join(&backup_filename);

    fs::copy(original_path, &backup_file)
        .map_err(|e| format!("Не удалось скопировать резервную копию: {e}"))?;

    let entry = BackupEntry {
        original_path: original_path.to_path_buf(),
        original_sha256: sha256,
        backup_filename,
        created_at: chrono_now_string(),
    };

    manifest.entries.insert(key, entry);
    manifest.save(backup_dir)?;

    Ok(backup_file)
}

/// Restore original file from backup.
pub fn restore_file(original_path: &Path, backup_dir: &Path) -> Result<(), String> {
    let manifest = BackupManifest::load(backup_dir);
    let key = key_for_path(original_path);
    let entry = manifest
        .entries
        .get(&key)
        .ok_or_else(|| "Резервная копия для данного файла не найдена в манифесте".to_string())?;

    let backup_file = backup_dir.join(&entry.backup_filename);
    if !backup_file.is_file() {
        return Err("Файл резервной копии поврежден или удален с диска".into());
    }

    // Verify backup integrity
    let current_backup_sha256 = compute_sha256(&backup_file)?;
    if current_backup_sha256 != entry.original_sha256 {
        return Err("Хеш резервной копии не совпадает с исходным, откат отменен".into());
    }

    // Safely copy backup over the file
    let tmp_dest = original_path.with_extension("restore_tmp");
    fs::copy(&backup_file, &tmp_dest)
        .map_err(|e| format!("Не удалось подготовить файл восстановления: {e}"))?;

    fs::rename(&tmp_dest, original_path)
        .or_else(|_| fs::copy(&backup_file, original_path).map(|_| ()))
        .map_err(|e| format!("Не удалось заменить файл оригиналом: {e}"))?;

    let _ = fs::remove_file(&tmp_dest);

    // Verify restored file
    let restored_sha256 = compute_sha256(original_path)?;
    if restored_sha256 != entry.original_sha256 {
        return Err("Восстановление не завершено: хеш восстановленного файла не совпадает".into());
    }

    Ok(())
}

fn chrono_now_string() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{now}")
}
