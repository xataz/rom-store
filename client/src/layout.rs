//! Local filesystem layout. Server identifiers and manifests stay unchanged.
use crate::{
    api::{File, Manifest},
    config::{Config, Frontend},
    install::{directory, relative_path, system_name},
};
use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct Layout {
    pub system: PathBuf,
    pub gamelist: PathBuf,
    pub media: Option<PathBuf>,
}

fn system_directory(root: &Path, system: &str) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    directory(&root.canonicalize()?, Path::new(system))
}

impl Layout {
    pub fn new(config: &Config, server_system: &str) -> Result<Self> {
        config.validate_layout()?;
        system_name(server_system)?;
        let name = config.local_system(server_system);
        system_name(name)?;
        let system = system_directory(&config.roms_dir, name)?;
        let gamelist = match &config.gamelists_dir {
            Some(root) => system_directory(root, name)?,
            None => system.clone(),
        };
        let media = if config.frontend == Frontend::Esde {
            Some(system_directory(
                config.media_dir.as_ref().context("media_dir absent")?,
                name,
            )?)
        } else {
            None
        };
        Ok(Self {
            system,
            gamelist,
            media,
        })
    }

    /// One source per ES-DE category, with deterministic fallback priorities.
    pub fn files<'a>(&self, manifest: &'a Manifest) -> Result<Vec<(&'a File, PathBuf, PathBuf)>> {
        let mut targets = Vec::new();
        for file in &manifest.files {
            if self.media.is_none() || matches!(file.role.as_str(), "rom" | "disk") {
                targets.push((file, self.system.clone(), relative_path(&file.destination)?));
            }
        }
        if let Some(media) = &self.media {
            let rom = relative_path(&manifest.path)?;
            for (category, roles) in [
                ("marquees", &["marquee", "wheel"][..]),
                ("covers", &["boxart", "thumbnail"][..]),
                ("fanart", &["fanart"][..]),
                ("miximages", &["image"][..]),
                ("physicalmedia", &["cartridge"][..]),
                ("screenshots", &["screenshot"][..]),
                ("titlescreens", &["titleshot"][..]),
                ("backcovers", &["boxback"][..]),
                ("videos", &["video"][..]),
                ("manuals", &["manual"][..]),
            ] {
                if let Some(file) = roles
                    .iter()
                    .find_map(|role| manifest.files.iter().find(|file| file.role == *role))
                {
                    let source = relative_path(&file.destination)?;
                    let extension = source
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let supported = match category {
                        "videos" => ["mp4", "mkv", "avi", "wmv", "mov", "webm", "m4v"]
                            .contains(&extension.as_str()),
                        "manuals" => extension == "pdf",
                        _ => ["png", "jpg", "webp"].contains(&extension.as_str()),
                    };
                    if !supported {
                        anyhow::bail!(
                            "Format média non pris en charge par ES-DE : {}",
                            file.destination
                        );
                    }
                    let mut destination = PathBuf::from(category).join(&rom);
                    destination.set_extension(extension);
                    // Revalidate generated names too (including reserved paths).
                    let destination =
                        relative_path(destination.to_str().context("Chemin média non UTF-8")?)?;
                    targets.push((file, media.clone(), destination));
                }
            }
        }
        Ok(targets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn esde_uses_fallbacks_and_playlist_names_without_changing_disc_paths() {
        let layout = Layout {
            system: "/roms/gc".into(),
            gamelist: "/gamelists/gc".into(),
            media: Some("/media/gc".into()),
        };
        let mut manifest = Manifest {
            version: 1,
            game_id: "a".repeat(64),
            revision: "b".repeat(64),
            system: "gamecube".into(),
            path: "Multidisc/Game.v2.m3u".into(),
            metadata: BTreeMap::new(),
            files: Vec::new(),
        };
        for (role, destination) in [
            ("rom", "Multidisc/Game.v2.m3u"),
            ("disk", "Multidisc/discs/Disc 1.chd"),
            ("wheel", "media/logo.PNG"),
            ("thumbnail", "media/cover.JPG"),
            ("map", "media/map.png"),
            ("bezel", "media/bezel.png"),
        ] {
            manifest.files.push(File {
                id: "c".repeat(64),
                role: role.into(),
                destination: destination.into(),
                size: 1,
                sha256: "d".repeat(64),
            });
        }
        let targets = layout.files(&manifest).unwrap();
        assert_eq!(targets.len(), 4);
        assert_eq!(targets[0].2, PathBuf::from("Multidisc/Game.v2.m3u"));
        assert_eq!(targets[1].2, PathBuf::from("Multidisc/discs/Disc 1.chd"));
        assert_eq!(
            targets[2].2,
            PathBuf::from("marquees/Multidisc/Game.v2.png")
        );
        assert_eq!(targets[3].2, PathBuf::from("covers/Multidisc/Game.v2.jpg"));
        manifest.files[2].destination = "media/logo.gif".into();
        assert!(layout
            .files(&manifest)
            .unwrap_err()
            .to_string()
            .contains("Format média"));
    }
}
