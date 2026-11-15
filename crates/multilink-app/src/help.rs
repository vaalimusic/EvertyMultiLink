use super::i18n::Language;
use std::sync::OnceLock;
pub const COUNT: usize = 8;
fn rows() -> &'static Vec<Vec<String>> {
    static ROWS: OnceLock<Vec<Vec<String>>> = OnceLock::new();
    ROWS.get_or_init(|| {
        (if cfg!(target_os = "macos") {
            include_str!("../locales/help-macos.tsv")
        } else {
            include_str!("../locales/help.tsv")
        })
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\t').map(|s| s.replace("\\n", "\n")).collect())
        .collect()
    })
}
pub fn section(language: Language, index: usize) -> (&'static str, &'static str) {
    let rows = rows();
    let index = index.min(COUNT - 1) * 2;
    (
        &rows[index][language.index()],
        &rows[index + 1][language.index()],
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use everty_ui::*;
    #[test]
    fn all_translated_sections_fit_the_smallest_window() {
        assert_eq!(rows().len(), COUNT * 2);
        let mut text = TextSystem::new();
        let style = TextStyle {
            font_size: LogicalPx(14.0),
            line_height: LogicalPx(22.0),
            ..TextStyle::default()
        };
        for lang in [
            Language::Ru,
            Language::En,
            Language::De,
            Language::Es,
            Language::Zh,
        ] {
            for i in 0..COUNT {
                let (title, body) = section(lang, i);
                assert!(!title.is_empty());
                let layout = text.shape(body, &style, Some(LogicalPx(666.0)));
                assert!(
                    layout.metrics.size.height.0 <= 350.0,
                    "{lang:?}: {title}: {}",
                    layout.metrics.size.height.0
                );
            }
        }
    }
}
