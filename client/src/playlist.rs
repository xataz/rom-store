//! Resolve M3U entries without changing the playlist or its directory layout.
use crate::install::{existing_regular, relative_path};
use anyhow::{bail, Context, Result};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_PLAYLIST: u64 = 1024 * 1024;
pub const MAX_DISCS: usize = 240;

pub fn is_playlist(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("m3u") || ext.eq_ignore_ascii_case("m3u8"))
}

pub fn dependent(path: &str) -> bool {
    is_playlist(path)
        || Path::new(path).extension().is_some_and(|ext| {
            ["cue", "gdi", "ccd", "mds"]
                .iter()
                .any(|kind| ext.eq_ignore_ascii_case(kind))
        })
}

pub fn read(path: &Path) -> Result<Vec<u8>> {
    if !existing_regular(path)? {
        bail!("M3U absent");
    }
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_PLAYLIST + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PLAYLIST {
        bail!("M3U trop volumineux (limite 1 Mio)");
    }
    Ok(bytes)
}

pub fn parse(path: &str, bytes: &[u8]) -> Result<Vec<PathBuf>> {
    if bytes.len() as u64 > MAX_PLAYLIST {
        bail!("M3U trop volumineux (limite 1 Mio)");
    }
    let text = std::str::from_utf8(bytes).context("Le M3U doit être UTF-8")?;
    let parent = Path::new(path).parent().unwrap_or(Path::new(""));
    let mut seen = HashSet::new();
    let mut discs = Vec::new();
    let mut count = 0;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        count += 1;
        if count > MAX_DISCS {
            bail!("Trop de disques dans le M3U (limite 240)");
        }
        if line.starts_with('/') || line.contains(['\\', ':', '\0']) {
            bail!("Chemin de disque interdit");
        }
        let mut target = parent.to_path_buf();
        for part in line.split('/') {
            match part {
                "" | "." => (),
                ".." => {
                    if !target.pop() {
                        bail!("Disque hors système");
                    }
                }
                part => {
                    relative_path(part)?;
                    target.push(part);
                }
            }
        }
        let target = relative_path(target.to_str().context("Chemin non UTF-8")?)?;
        if dependent(target.to_str().context("Chemin non UTF-8")?) {
            bail!("Disque à dépendances non pris en charge");
        }
        if seen.insert(target.clone()) {
            discs.push(target);
        }
    }
    if discs.is_empty() {
        bail!("Le M3U ne contient aucun disque");
    }
    Ok(discs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_bom_crlf_parent_paths_and_duplicates_preserve_reference_order() {
        assert_eq!(parse("games/demo.m3u", "\u{feff}#EXTM3U\r\n\r\ndiscs/Été & test.chd\r\n../disc2.zip\r\n./discs/Été & test.chd\r\n".as_bytes()).unwrap(), [PathBuf::from("games/discs/Été & test.chd"), PathBuf::from("disc2.zip")]);
        assert!(is_playlist("DEMO.M3U8"));
    }

    #[test]
    fn invalid_references_encodings_and_limits_are_rejected() {
        for line in [
            "",
            "#EXTM3U",
            "../../outside.chd",
            "/etc/passwd",
            "https://example.org/disc.chd",
            "C:\\disc.chd",
            ".romstore/private",
            "gamelist.xml",
            "disc.cue",
            "disc.m3u",
            "disc.gdi",
            "../.romstore/../disc.chd",
        ] {
            assert!(parse("games/demo.m3u", line.as_bytes()).is_err(), "{line}");
        }
        assert!(parse("demo.m3u", &[0xff]).is_err());
        assert!(parse("demo.m3u", &vec![b'x'; MAX_PLAYLIST as usize + 1]).is_err());
        assert!(parse("demo.m3u", "disc.chd\n".repeat(MAX_DISCS + 1).as_bytes()).is_err());
    }
}
