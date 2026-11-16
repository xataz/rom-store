//! Embedded translations: no locale files or network requests needed at runtime.
use serde::Deserialize;
use std::{collections::BTreeMap, fmt::Display, sync::OnceLock};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Fr,
    En,
    Es,
    De,
}

impl Language {
    /// The v1 API exposes this reason as text rather than a machine-readable code.
    /// Preserve any unknown server diagnostics instead of hiding useful details.
    pub fn unavailable_reason(self, reason: Option<&str>) -> &str {
        match reason {
            Some("Format multi-fichiers non pris en charge") => self.text("unsupported_multifile"),
            Some(reason) => reason,
            None => self.text("unsupported_format"),
        }
    }

    fn catalog(self) -> &'static BTreeMap<String, String> {
        static FR: OnceLock<BTreeMap<String, String>> = OnceLock::new();
        static EN: OnceLock<BTreeMap<String, String>> = OnceLock::new();
        static ES: OnceLock<BTreeMap<String, String>> = OnceLock::new();
        static DE: OnceLock<BTreeMap<String, String>> = OnceLock::new();
        let (cache, source) = match self {
            Self::Fr => (&FR, include_str!("../locales/fr.yaml")),
            Self::En => (&EN, include_str!("../locales/en.yaml")),
            Self::Es => (&ES, include_str!("../locales/es.yaml")),
            Self::De => (&DE, include_str!("../locales/de.yaml")),
        };
        cache.get_or_init(|| serde_saphyr::from_str(source).expect("Invalid embedded translation"))
    }

    pub fn text(self, key: &'static str) -> &'static str {
        self.catalog()
            .get(key)
            .or_else(|| Self::Fr.catalog().get(key))
            .map_or(key, String::as_str)
    }

    /// Substitute numbered placeholders without interpreting braces in user data.
    pub fn format(self, key: &'static str, args: &[&dyn Display]) -> String {
        let mut rest = self.text(key);
        let mut output = String::new();
        while let Some(start) = rest.find('{') {
            output.push_str(&rest[..start]);
            let Some(end) = rest[start..].find('}').map(|end| start + end) else {
                output.push_str(&rest[start..]);
                return output;
            };
            if let Some(value) = rest[start + 1..end]
                .parse::<usize>()
                .ok()
                .and_then(|index| args.get(index))
            {
                output.push_str(&value.to_string());
            } else {
                output.push_str(&rest[start..=end]);
            }
            rest = &rest[end + 1..];
        }
        output.push_str(rest);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .map(|part| part.split('}').next().unwrap())
            .collect()
    }

    #[test]
    fn every_language_has_the_same_nonempty_keys_and_placeholders() {
        let reference = Language::Fr.catalog();
        for language in [Language::En, Language::Es, Language::De] {
            let catalog = language.catalog();
            assert_eq!(
                reference.keys().collect::<Vec<_>>(),
                catalog.keys().collect::<Vec<_>>()
            );
            for (key, text) in catalog {
                assert!(!text.trim().is_empty(), "{language:?}: {key}");
                assert_eq!(
                    placeholders(&reference[key]),
                    placeholders(text),
                    "{language:?}: {key}"
                );
            }
        }
    }

    #[test]
    fn interpolation_does_not_interpret_inserted_values() {
        assert_eq!(
            Language::En.format("retry_confirm", &[&"error {0} {1}"]),
            "error {0} {1} — press Enter to retry"
        );
        assert_eq!(
            Language::En.format("catalog_position", &[&2, &10000]),
            "2 / 10000 games"
        );
    }

    #[test]
    fn known_server_reason_is_localized_and_unknown_diagnostics_are_preserved() {
        assert_eq!(
            Language::En.unavailable_reason(Some("Format multi-fichiers non pris en charge")),
            "Multi-file format not supported"
        );
        assert_eq!(
            Language::En.unavailable_reason(Some("Server-specific diagnostic")),
            "Server-specific diagnostic"
        );
        assert_eq!(Language::En.unavailable_reason(None), "Unsupported format");
    }

    #[test]
    fn all_literal_ui_keys_exist() {
        for source in [
            include_str!("views.rs"),
            include_str!("main.rs"),
            include_str!("ui.rs"),
            include_str!("fonts.rs"),
            include_str!("smoke.rs"),
            include_str!("search.rs"),
        ] {
            for marker in [".text(", ".format("] {
                for tail in source.split(marker).skip(1) {
                    if let Some(tail) = tail.trim_start().strip_prefix('"') {
                        let key = tail.split('"').next().unwrap();
                        assert!(
                            Language::Fr.catalog().contains_key(key),
                            "Missing translation: {key}"
                        );
                    }
                }
            }
        }
    }
}
