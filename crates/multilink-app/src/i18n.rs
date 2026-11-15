use std::{borrow::Cow, sync::OnceLock};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Ru,
    En,
    De,
    Es,
    Zh,
}
impl Language {
    pub fn index(self) -> usize {
        match self {
            Self::Ru => 0,
            Self::En => 1,
            Self::De => 2,
            Self::Es => 3,
            Self::Zh => 4,
        }
    }
}
pub fn resolve(setting: &str, system: &str) -> Language {
    let value = if setting == "system" { system } else { setting };
    match value
        .split(['-', '_', '.'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "ru" => Language::Ru,
        "de" => Language::De,
        "es" => Language::Es,
        "zh" => Language::Zh,
        _ => Language::En,
    }
}
pub const OPTIONS: &[(&str, &str)] = &[
    ("system", "Системный язык"),
    ("ru", "Русский"),
    ("en", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("zh", "中文"),
];
pub fn label(setting: &str) -> &'static str {
    match setting {
        "ru" => "Русский",
        "en" => "English",
        "de" => "Deutsch",
        "es" => "Español",
        "zh" => "中文",
        _ => "Системный язык",
    }
}
fn rows() -> &'static Vec<Vec<&'static str>> {
    static ROWS: OnceLock<Vec<Vec<&str>>> = OnceLock::new();
    ROWS.get_or_init(|| {
        include_str!("../locales/ui.tsv")
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.split('\t').collect())
            .collect()
    })
}
fn captures<'a>(pattern: &str, value: &'a str) -> Option<Vec<&'a str>> {
    let mut chunks = Vec::new();
    let mut source = pattern;
    while let Some(open) = source.find('{') {
        let end = source[open..].find('}')? + open;
        chunks.push(&source[..open]);
        source = &source[end + 1..];
    }
    if chunks.is_empty() {
        return None;
    }
    chunks.push(source);
    let mut remaining = value.strip_prefix(chunks[0])?;
    let mut values = Vec::new();
    for (i, chunk) in chunks.iter().enumerate().skip(1) {
        let at = if chunk.is_empty() {
            if i == chunks.len() - 1 {
                remaining.len()
            } else {
                return None;
            }
        } else if i == chunks.len() - 1 {
            remaining
                .len()
                .checked_sub(chunk.len())
                .filter(|at| remaining.get(*at..) == Some(*chunk))?
        } else {
            remaining.find(chunk)?
        };
        values.push(&remaining[..at]);
        remaining = &remaining[at + chunk.len()..];
    }
    remaining.is_empty().then_some(values)
}
pub fn tr<'a>(language: Language, value: &'a str) -> Cow<'a, str> {
    if language == Language::Ru {
        return Cow::Borrowed(value);
    }
    let index = language.index();
    if let Some(row) = rows().iter().find(|row| row[0] == value) {
        return Cow::Borrowed(row[index]);
    }
    for row in rows().iter().filter(|r| r[0].contains('{')) {
        if let Some(values) = captures(row[0], value) {
            let mut result = String::new();
            let mut translated = row[index];
            for capture in values {
                if let Some(open) = translated.find('{') {
                    if let Some(end) = translated[open..].find('}') {
                        result.push_str(&translated[..open]);
                        result.push_str(&tr_plain(language, capture));
                        translated = &translated[open + end + 1..];
                    }
                }
            }
            result.push_str(translated);
            return Cow::Owned(result);
        }
    }
    Cow::Borrowed(value)
}
fn tr_plain(language: Language, value: &str) -> String {
    tr(language, value).into_owned()
}
impl super::App {
    pub fn language(&self) -> Language {
        static SYSTEM: OnceLock<String> = OnceLock::new();
        resolve(
            &self.settings.language,
            SYSTEM.get_or_init(multilink_platform::system_locale),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locales_and_templates_work() {
        assert_eq!(resolve("system", "ru-RU"), Language::Ru);
        assert_eq!(resolve("system", "ru_RU.UTF-8"), Language::Ru);
        assert_eq!(resolve("en", "ru-RU"), Language::En);
        assert_eq!(resolve("system", "de-DE"), Language::De);
        assert_eq!(resolve("system", "fr-FR"), Language::En);
        assert_eq!(
            tr(Language::En, "25 серверов · 3 включено"),
            "25 servers · 3 enabled"
        );
        for r in rows() {
            assert_eq!(r.len(), 5, "{}", r[0]);
            assert!(r.iter().all(|s| !s.is_empty()));
            let arguments = r[0].matches('{').count();
            for value in &r[1..] {
                assert_eq!(value.matches('{').count(), arguments, "{}", r[0]);
            }
        }
    }
}
