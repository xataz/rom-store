use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Config {
    pub library: PathBuf,
    pub database: PathBuf,
    pub system_icon: PathBuf,
}

/// Resolve existing symlinks, including ancestors of paths not created yet.
fn resolve(path: &Path) -> Result<PathBuf> {
    let path = if path == Path::new("~") || path.starts_with("~/") {
        PathBuf::from(std::env::var_os("HOME").context("HOME introuvable")?)
            .join(path.strip_prefix("~")?)
    } else {
        path.to_path_buf()
    };
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => (),
            _ => {
                resolved.push(part);
                match resolved.canonicalize() {
                    Ok(real) => resolved = real,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    Ok(resolved)
}

impl Config {
    pub fn new(
        library: impl AsRef<Path>,
        database: impl AsRef<Path>,
        system_icon: Option<&Path>,
    ) -> Result<Self> {
        let library = resolve(library.as_ref())?
            .canonicalize()
            .context("Bibliothèque introuvable")?;
        if !library.is_dir() {
            bail!("La bibliothèque doit être un répertoire");
        }
        let database = resolve(database.as_ref())?;
        if database.starts_with(&library) {
            bail!("La base doit être située hors de la bibliothèque");
        }
        let parent = database.parent().context("Répertoire de base invalide")?;
        let default_icons = parent.join("system-icons");
        let system_icon = resolve(system_icon.unwrap_or(&default_icons))?;
        if system_icon == database || (system_icon.exists() && !system_icon.is_dir()) {
            bail!("Le chemin --system-icon doit désigner un répertoire");
        }
        fs::create_dir_all(parent)?;
        Ok(Self {
            library,
            database,
            system_icon,
        })
    }
}
