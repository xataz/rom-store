//! Presentation names embedded at compile time; never used as API IDs or paths.
use std::{collections::BTreeMap, sync::OnceLock};

const SOURCE: &str = include_str!("../data/systems.txt");

fn entries(source: &str) -> impl Iterator<Item = (&str, &str)> {
    source.lines().filter_map(|line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (id, name) = line.split_once(':')?;
        let (id, name) = (id.trim(), name.trim());
        (!id.is_empty() && !name.is_empty()).then_some((id, name))
    })
}

/// Prefer the embedded name, then the server's name, then the raw identifier.
/// Cargo tracks `include_str!`, so editing the data triggers a rebuild.
pub fn display_name<'a>(id: &'a str, fallback: &'a str) -> &'a str {
    static NAMES: OnceLock<BTreeMap<&'static str, &'static str>> = OnceLock::new();
    let names = NAMES.get_or_init(|| entries(SOURCE).collect());
    names
        .get(id)
        .or_else(|| names.get(id.to_ascii_lowercase().as_str()))
        .copied()
        .unwrap_or_else(|| {
            if fallback.trim().is_empty() {
                id
            } else {
                fallback
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_format_supports_comments_unicode_whitespace_crlf_and_colons_in_names() {
        let source =
            "\u{feff}# Plateformes\r\n\r\n nes : Console démo\r\ncustom: Démo : édition spéciale\n";
        let names: BTreeMap<_, _> = entries(source).collect();
        assert_eq!(names.len(), 2);
        assert_eq!(names["nes"], "Console démo");
        assert_eq!(names["custom"], "Démo : édition spéciale");
    }

    #[test]
    fn embedded_data_has_nonempty_unique_lowercase_identifiers_and_names() {
        let mut names = BTreeMap::new();
        for (index, line) in SOURCE.lines().enumerate() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (id, name) = entries(line).next().unwrap_or_else(|| {
                panic!("systems.txt ligne {} : identifiant: nom attendu", index + 1)
            });
            assert!(id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_'));
            assert!(
                names.insert(id, name).is_none(),
                "Identifiant répété : {id}"
            );
        }
        assert!(!names.is_empty());
        // Expectations follow the data: changing a label needs no Rust test changes.
        for (id, name) in names {
            assert_eq!(display_name(id, "Nom serveur"), name);
            assert_eq!(display_name(&id.to_ascii_uppercase(), "Nom serveur"), name);
            let system = crate::api::System {
                id: id.into(),
                name: "Nom serveur".into(),
                game_count: 7,
                image: None,
            };
            assert_eq!(system.display_name(), name);
            assert_eq!(system.id, id);
            assert_eq!(system.name, "Nom serveur");
            assert_eq!(system.game_count, 7);
        }
    }

    #[test]
    fn unknown_systems_keep_the_server_name_or_identifier() {
        assert_eq!(display_name("__custom_demo__", "Ma console"), "Ma console");
        assert_eq!(display_name("__custom_demo__", ""), "__custom_demo__");
        assert_eq!(display_name("__custom_demo__", "  "), "__custom_demo__");
    }
}
