//! M3U references are relative to the playlist, not to gamelist.xml.
use crate::paths::{regular_file, relative_path};
use anyhow::{bail, Context, Result};
use std::{collections::HashSet, io::Read, path::Path};

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
    let file = regular_file(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_PLAYLIST + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PLAYLIST {
        bail!("M3U trop volumineux (limite 1 Mio)");
    }
    Ok(bytes)
}

pub fn parse(path: &str, bytes: &[u8]) -> Result<Vec<String>> {
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
        if dependent(&target) {
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
