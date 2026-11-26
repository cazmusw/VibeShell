use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::Path,
    sync::{LazyLock, OnceLock},
};

type Catalog = HashMap<String, String>;

static RUSSIAN: LazyLock<Catalog> =
    LazyLock::new(|| catalog(&[include_str!("../assets/i18n/mosaic-files-ru.tsv")]));
static ENGLISH: LazyLock<Catalog> = LazyLock::new(|| {
    catalog(&[
        include_str!("../assets/i18n/mosaic-files-en.tsv"),
        include_str!("../assets/i18n/mosaic-files-tab.tsv"),
        include_str!("../assets/i18n/mosaic-files-dialogs.tsv"),
    ])
});
static LANGUAGE: OnceLock<Language> = OnceLock::new();

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Language {
    #[default]
    Russian,
    English,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::Russian, Self::English];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Russian => "ru",
            Self::English => "en",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Russian => "Русский",
            Self::English => "English",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|language| language.code() == code)
    }

    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|code| Self::from_code(code.trim()))
            .unwrap_or_default()
    }

    pub fn save(self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .expect("language preference has a parent directory");
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::with_prefix_in(".language-", parent)?;
        writeln!(file, "{}", self.code())?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    fn translate(self, source: &str) -> &str {
        let translations = match self {
            Self::Russian => &*RUSSIAN,
            Self::English => &*ENGLISH,
        };
        translations.get(source).map_or(source, String::as_str)
    }
}

pub fn initialize(path: &Path) {
    LANGUAGE.get_or_init(|| Language::load(path));
}

pub fn current() -> Language {
    LANGUAGE.get().copied().unwrap_or_default()
}

pub fn tr(source: &str) -> &str {
    current().translate(source)
}

fn catalog(sources: &[&str]) -> Catalog {
    sources
        .iter()
        .flat_map(|source| source.lines())
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('\t'))
        .map(|(source, translation)| (decode(source), decode(translation)))
        .collect()
}

fn decode(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        match chars.next() {
            Some('n') => decoded.push('\n'),
            Some('t') => decoded.push('\t'),
            Some('\\') => decoded.push('\\'),
            Some(next) => {
                decoded.push('\\');
                decoded.push(next);
            }
            None => decoded.push('\\'),
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_the_selected_catalog_with_a_source_fallback() {
        assert_eq!(Language::Russian.translate("Файлы"), "Файлы");
        assert_eq!(Language::English.translate("Файлы"), "Files");
        assert_eq!(Language::English.translate("notes.txt"), "notes.txt");
    }

    #[test]
    fn restores_the_saved_language_across_launches() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mosaic-files/language");

        assert_eq!(Language::load(&path), Language::Russian);
        Language::English.save(&path).unwrap();
        assert_eq!(Language::load(&path), Language::English);
        Language::Russian.save(&path).unwrap();
        assert_eq!(Language::load(&path), Language::Russian);
    }
}
