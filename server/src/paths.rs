use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};

pub fn relative_path(raw: &str) -> Result<String> {
    if raw.is_empty() || raw.contains(['\\', ':', '\0']) || raw.starts_with('/') {
        bail!("Chemin relatif invalide");
    }
    let mut parts = Vec::new();
    for component in Path::new(raw).components() {
        match component {
            Component::CurDir => (),
            Component::Normal(name) => {
                let name = name.to_str().context("Chemin non UTF-8")?;
                if name.starts_with(".romstore")
                    || matches!(name, "gamelist.xml" | "gamelist.xml.bak")
                {
                    bail!("Chemin réservé");
                }
                parts.push(name);
            }
            _ => bail!("Chemin hors système"),
        }
    }
    if parts.is_empty() {
        bail!("Chemin vide");
    }
    Ok(parts.join("/"))
}

pub fn source_file(root: &Path, raw: &str) -> Result<(String, PathBuf)> {
    let relative = relative_path(raw)?;
    let path = root.join(&relative).canonicalize()?;
    if !path.starts_with(root) || !path.is_file() {
        bail!("Fichier hors système ou non régulier");
    }
    Ok((relative, path))
}

pub fn regular_file(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        bail!("Fichier non régulier");
    }
    Ok(file)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Signature(pub u64, pub u64, pub u64, pub i64, pub i64);

impl Signature {
    pub fn of(meta: &Metadata) -> Self {
        Self(
            meta.dev(),
            meta.ino(),
            meta.len(),
            meta.mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.mtime_nsec()),
            meta.ctime()
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.ctime_nsec()),
        )
    }
}

pub fn reject_symlink_root(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() || path.is_symlink() {
        bail!("Répertoire source remplacé");
    }
    Ok(())
}
