use super::discovery::DiscoveredTarget;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub const SIG_ORIGINAL: &[u8] = b"ineligible";
pub const SIG_MODIFIED: &[u8] = b"inexigible";

pub const MARKER_MAIN_JS: &str = "// UNLOCKED";
pub const MARKER_EXTENSION_JS: &str = "/*[AG_EXT_PATCHED]*/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PatchState {
    Patched,
    NotPatched,
    Partial,
    Unknown,
    NotInstalled,
    Error,
}

impl PatchState {
    pub fn badge_text(&self) -> &'static str {
        match self {
            Self::Patched => "✓ Патч обнаружен",
            Self::NotPatched => "⚠ Патч отсутствует",
            Self::Partial => "! Частичная модификация",
            Self::Unknown => "! Неизвестная версия",
            Self::NotInstalled => "Не установлен",
            Self::Error => "! Ошибка проверки",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetStatus {
    pub target: DiscoveredTarget,
    pub state: PatchState,
    pub message: String,
    pub original_signature_count: usize,
    pub modified_signature_count: usize,
    pub sha256: Option<String>,
    pub backup_available: bool,
}

/// Compute SHA256 hex string of a file.
pub fn compute_sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("Не удалось открыть файл: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|e| format!("Ошибка чтения: {e}"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Detect patch status of a discovered target.
pub fn detect_status(target: &DiscoveredTarget, backup_dir: Option<&Path>) -> TargetStatus {
    if !target.path.is_file() {
        return TargetStatus {
            target: target.clone(),
            state: PatchState::NotInstalled,
            message: "Файл не найден".into(),
            original_signature_count: 0,
            modified_signature_count: 0,
            sha256: None,
            backup_available: false,
        };
    }

    let sha256 = compute_sha256(&target.path).ok();
    let backup_available = backup_dir.map_or(false, |bd| {
        super::backup::backup_exists(&target.path, bd)
    });

    if target.is_binary {
        detect_binary_status(target, sha256, backup_available)
    } else {
        detect_js_status(target, sha256, backup_available)
    }
}

fn detect_binary_status(
    target: &DiscoveredTarget,
    sha256: Option<String>,
    backup_available: bool,
) -> TargetStatus {
    let data = match std::fs::read(&target.path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return TargetStatus {
                target: target.clone(),
                state: PatchState::Error,
                message: format!("Ошибка чтения файла: {e}"),
                original_signature_count: 0,
                modified_signature_count: 0,
                sha256,
                backup_available,
            };
        }
    };

    let orig_count = count_occurrences(&data, SIG_ORIGINAL);
    let mod_count = count_occurrences(&data, SIG_MODIFIED);

    let (state, message) = if mod_count > 0 && orig_count == 0 {
        (
            PatchState::Patched,
            format!("Модификация активна ({mod_count} сигнатур)"),
        )
    } else if orig_count > 0 && mod_count == 0 {
        (
            PatchState::NotPatched,
            format!("Исходный файл ({orig_count} сигнатур)"),
        )
    } else if orig_count > 0 && mod_count > 0 {
        (
            PatchState::Partial,
            format!("Смешанное состояние ({orig_count} ориг., {mod_count} мод.)"),
        )
    } else {
        (
            PatchState::Unknown,
            "Сигнатуры eligibility не найдены".into(),
        )
    };

    TargetStatus {
        target: target.clone(),
        state,
        message,
        original_signature_count: orig_count,
        modified_signature_count: mod_count,
        sha256,
        backup_available,
    }
}

fn detect_js_status(
    target: &DiscoveredTarget,
    sha256: Option<String>,
    backup_available: bool,
) -> TargetStatus {
    let content = match std::fs::read_to_string(&target.path) {
        Ok(s) => s,
        Err(e) => {
            return TargetStatus {
                target: target.clone(),
                state: PatchState::Error,
                message: format!("Ошибка чтения текстового файла: {e}"),
                original_signature_count: 0,
                modified_signature_count: 0,
                sha256,
                backup_available,
            };
        }
    };

    let is_main_js = target.path.file_name().map_or(false, |f| f == "main.js");
    let has_marker = if is_main_js {
        content.contains(MARKER_MAIN_JS)
    } else {
        content.contains(MARKER_EXTENSION_JS)
    };

    let (state, message) = if has_marker {
        (PatchState::Patched, "Патч обнаружен по маркеру".into())
    } else {
        (
            PatchState::NotPatched,
            "Исходный скрипт без модификаций".into(),
        )
    };

    TargetStatus {
        target: target.clone(),
        state,
        message,
        original_signature_count: if has_marker { 0 } else { 1 },
        modified_signature_count: if has_marker { 1 } else { 0 },
        sha256,
        backup_available,
    }
}

fn count_occurrences(data: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || data.len() < needle.len() {
        return 0;
    }
    let mut count = 0;
    let mut pos = 0;
    while pos + needle.len() <= data.len() {
        if &data[pos..pos + needle.len()] == needle {
            count += 1;
            pos += needle.len();
        } else {
            pos += 1;
        }
    }
    count
}
