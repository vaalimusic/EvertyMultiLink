use super::backup::{create_backup, restore_file};
use super::detector::{
    detect_status, PatchState, TargetStatus, MARKER_EXTENSION_JS, MARKER_MAIN_JS, SIG_MODIFIED,
    SIG_ORIGINAL,
};
use super::discovery::DiscoveredTarget;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchOperationResult {
    pub success: bool,
    pub target_path: String,
    pub message: String,
    pub new_status: Option<TargetStatus>,
}

/// Check if a file is locked by a running process by attempting to open it for writing.
pub fn check_file_locked(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    match OpenOptions::new().write(true).open(path) {
        Ok(_) => Ok(false),
        Err(e) => {
            #[cfg(windows)]
            {
                // ERROR_SHARING_VIOLATION = 32, ERROR_LOCK_VIOLATION = 33
                if let Some(code) = e.raw_os_error() {
                    if code == 32 || code == 33 {
                        return Ok(true);
                    }
                }
            }
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Ok(true)
            } else {
                Err(format!("Ошибка проверки доступа к файлу: {e}"))
            }
        }
    }
}

/// Apply patch to a discovered target.
/// Does NOT touch any proxy settings (https_proxy, AG_LS_PROXY).
pub fn apply_patch(target: &DiscoveredTarget, backup_dir: &Path) -> PatchOperationResult {
    let path_str = target.path.display().to_string();

    // 1. Check if file is locked / process running
    match check_file_locked(&target.path) {
        Ok(true) => {
            return PatchOperationResult {
                success: false,
                target_path: path_str,
                message: "Файл занят запущенным процессом. Закройте Antigravity Desktop / IDE / CLI и повторите.".into(),
                new_status: None,
            };
        }
        Err(e) => {
            return PatchOperationResult {
                success: false,
                target_path: path_str,
                message: e,
                new_status: None,
            };
        }
        Ok(false) => {}
    }

    // 2. Check current status
    let current_status = detect_status(target, Some(backup_dir));
    if current_status.state == PatchState::Patched {
        return PatchOperationResult {
            success: true,
            target_path: path_str,
            message: "Файл уже имеет активную модификацию".into(),
            new_status: Some(current_status),
        };
    }
    if current_status.state == PatchState::Partial {
        return PatchOperationResult {
            success: false,
            target_path: path_str,
            message: "Обнаружена частичная модификация. Сначала восстановите оригинал из резервной копии.".into(),
            new_status: Some(current_status),
        };
    }
    if current_status.state == PatchState::Unknown {
        return PatchOperationResult {
            success: false,
            target_path: path_str,
            message: "Неизвестная структура или версия файла. Автоматическая модификация отклонена в целях безопасности.".into(),
            new_status: Some(current_status),
        };
    }

    // 3. Create verified backup
    if let Err(e) = create_backup(&target.path, backup_dir) {
        return PatchOperationResult {
            success: false,
            target_path: path_str,
            message: format!("Не удалось создать резервную копию: {e}. Операция отменена."),
            new_status: None,
        };
    }

    // 4. Perform patch modification
    let patch_result = if target.is_binary {
        patch_binary(&target.path)
    } else {
        patch_js(&target.path)
    };

    if let Err(e) = patch_result {
        // Attempt rollback on failure
        let _ = restore_file(&target.path, backup_dir);
        return PatchOperationResult {
            success: false,
            target_path: path_str,
            message: format!("Ошибка применения патча: {e}. Выполнен откат."),
            new_status: Some(detect_status(target, Some(backup_dir))),
        };
    }

    // 5. Verify the new state
    let new_status = detect_status(target, Some(backup_dir));
    if new_status.state == PatchState::Patched {
        PatchOperationResult {
            success: true,
            target_path: path_str,
            message: "Патч успешно применён и проверен".into(),
            new_status: Some(new_status),
        }
    } else {
        // Verification failed, rollback
        let _ = restore_file(&target.path, backup_dir);
        PatchOperationResult {
            success: false,
            target_path: path_str,
            message: "Проверка после патча не подтвердила корректность. Выполнен откат к оригиналу.".into(),
            new_status: Some(detect_status(target, Some(backup_dir))),
        }
    }
}

fn patch_binary(path: &Path) -> Result<(), String> {
    let mut data = fs::read(path).map_err(|e| format!("Не удалось прочитать бинарник: {e}"))?;

    let mut replaced_count = 0;
    let mut pos = 0;
    let needle_len = SIG_ORIGINAL.len();

    while pos + needle_len <= data.len() {
        if &data[pos..pos + needle_len] == SIG_ORIGINAL {
            data[pos..pos + needle_len].copy_from_slice(SIG_MODIFIED);
            replaced_count += 1;
            pos += needle_len;
        } else {
            pos += 1;
        }
    }

    if replaced_count == 0 {
        return Err("Сигнатура для замены не найдена".into());
    }

    // Write to safe tmp file
    let tmp_path = path.with_extension("ag_tmp");
    fs::write(&tmp_path, &data).map_err(|e| format!("Не удалось записать временный файл: {e}"))?;

    // Atomic replace
    fs::rename(&tmp_path, path)
        .or_else(|_| fs::copy(&tmp_path, path).map(|_| ()))
        .map_err(|e| format!("Не удалось заменить файл: {e}"))?;

    let _ = fs::remove_file(&tmp_path);
    Ok(())
}

fn patch_js(path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| format!("Не удалось прочитать JS файл: {e}"))?;

    let is_main_js = path.file_name().map_or(false, |f| f == "main.js");
    let marker = if is_main_js {
        MARKER_MAIN_JS
    } else {
        MARKER_EXTENSION_JS
    };

    if content.contains(marker) {
        return Ok(());
    }

    // Prepare modified content with appropriate unlocked trailer/header
    let mut modified = content;
    if is_main_js {
        modified.push_str("\n\n// Antigravity Eligibility Bypass\n");
        modified.push_str(MARKER_MAIN_JS);
        modified.push('\n');
    } else {
        modified.push_str("\n\n");
        modified.push_str(MARKER_EXTENSION_JS);
        modified.push_str("\n// Antigravity Extension Patch\n");
    }

    let tmp_path = path.with_extension("ag_tmp");
    fs::write(&tmp_path, modified).map_err(|e| format!("Не удалось записать JS файл: {e}"))?;

    fs::rename(&tmp_path, path)
        .or_else(|_| fs::copy(&tmp_path, path).map(|_| ()))
        .map_err(|e| format!("Не удалось заменить JS файл: {e}"))?;

    let _ = fs::remove_file(&tmp_path);
    Ok(())
}
