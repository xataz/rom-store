use anyhow::{anyhow, bail, Result};
use fontdue::{Font, FontSettings};
use romstore_client::i18n::Language;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const FALLBACKS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/google-noto-vf/NotoSans[wght].ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
    "/usr/share/emulationstation/resources/opensans_hebrew.ttf",
    "/usr/share/emulationstation/resources/DejaVuSans.ttf",
];

pub fn load_font(preferred: &Path, language: Language) -> Result<Font> {
    let paths = std::iter::once(preferred.to_path_buf()).chain(FALLBACKS.iter().map(PathBuf::from));
    let mut failures = Vec::new();
    let decode = |bytes| Font::from_bytes(bytes, FontSettings::default()).map_err(|e| anyhow!(e));
    let mut selected = load_first(paths, decode, &mut failures);
    if selected.is_none() {
        // Optional discovery, without a shell or a link-time Fontconfig dependency.
        if let Ok(output) = Command::new("fc-match")
            .args(["-f", "%{file}\n", "sans-serif"])
            .output()
        {
            if output.status.success() {
                let paths = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(PathBuf::from)
                    .collect::<Vec<_>>();
                selected = load_first(paths, decode, &mut failures);
            }
        }
    }
    if let Some((font, path)) = selected {
        eprintln!("{}", language.format("font_used", &[&path.display()]));
        return Ok(font);
    }
    bail!("{}", language.format("no_font", &[&failures.join("\n")]))
}

fn load_first<T>(
    paths: impl IntoIterator<Item = PathBuf>,
    mut decode: impl FnMut(Vec<u8>) -> Result<T>,
    failures: &mut Vec<String>,
) -> Option<(T, PathBuf)> {
    for path in paths {
        match std::fs::read(&path)
            .map_err(anyhow::Error::from)
            .and_then(&mut decode)
        {
            Ok(font) => return Some((font, path)),
            Err(error) => failures.push(format!("- {} : {error}", path.display())),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_unreadable_and_invalid_fonts_fall_back_to_next_candidate() {
        let root = tempfile::tempdir().unwrap();
        let invalid = root.path().join("invalid.ttf");
        let valid = root.path().join("valid.ttf");
        std::fs::write(&invalid, b"invalid").unwrap();
        std::fs::write(&valid, b"valid").unwrap();
        let mut failures = Vec::new();
        let result = load_first(
            [
                root.path().join("missing.ttf"),
                root.path().to_path_buf(),
                invalid,
                valid.clone(),
            ],
            |bytes| {
                if bytes != b"valid" {
                    bail!("invalid font");
                }
                Ok(bytes)
            },
            &mut failures,
        )
        .unwrap();
        assert_eq!(result, (b"valid".to_vec(), valid));
        assert_eq!(failures.len(), 3);
    }

    #[test]
    fn preferred_font_wins_without_loading_fallbacks() {
        let root = tempfile::tempdir().unwrap();
        let preferred = root.path().join("preferred.ttf");
        std::fs::write(&preferred, b"preferred").unwrap();
        let mut failures = Vec::new();
        let result = load_first(
            [preferred.clone(), root.path().join("fallback.ttf")],
            Ok,
            &mut failures,
        )
        .unwrap();
        assert_eq!(result.1, preferred);
        assert!(failures.is_empty());
    }

    #[test]
    fn no_usable_font_reports_all_attempted_paths() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing.ttf");
        let mut failures = Vec::new();
        assert!(load_first([path.clone()], Ok, &mut failures).is_none());
        assert!(failures[0].contains(&path.display().to_string()));
    }
}
