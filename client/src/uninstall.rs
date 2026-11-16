//! Offline removal of one local ROM and its XML entry. Media and saves are never deleted.
use crate::{
    api::Game,
    config::Config,
    install::{
        directory, encode_xml, existing_regular, parse_xml, publish_gamelist, read_xml,
        relative_path, sync_directory, system_name, Progress,
    },
    layout::Layout,
};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Validate existing parents without creating directories or following symlinks.
fn local_path(system: &Path, relative: &Path) -> Result<PathBuf> {
    let mut path = system.to_path_buf();
    let parts: Vec<_> = relative.components().collect();
    for (index, part) in parts.iter().enumerate() {
        if !matches!(part, Component::Normal(_)) {
            bail!("Chemin local invalide");
        }
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta)
                if meta.file_type().is_symlink() || (index + 1 < parts.len() && !meta.is_dir()) =>
            {
                bail!(
                    "Chemin local non régulier ou lien symbolique : {}",
                    path.display()
                )
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(system.join(relative))
            }
            Err(error) => return Err(error.into()),
        }
    }
    existing_regular(&path)?;
    Ok(path)
}

fn references(system: &Path, raw: &str, rom: &Path) -> bool {
    let candidate = system.join(raw);
    candidate == rom || candidate.canonicalize().ok().as_deref() == Some(rom)
}

fn cleanup_quarantine(directory: &Path) {
    // Only the two known private entries are ever removed, never recursively.
    if let Err(error) =
        fs::remove_file(directory.join("restore.json")).and_then(|_| fs::remove_dir(directory))
    {
        eprintln!(
            "Nettoyage de désinstallation incomplet ({}): {error}",
            directory.display()
        );
    }
}

/// Stage the ROM persistently before publishing XML. A failed publication restores
/// it without clobbering a concurrently created file; a crash leaves a recoverable ROM.
fn remove_with_xml(rom: &Path, state: &Path, publish: impl FnOnce() -> Result<()>) -> Result<()> {
    if !existing_regular(rom)? {
        return publish();
    }
    let quarantine = tempfile::Builder::new()
        .prefix("uninstall-")
        .tempdir_in(state)?
        .keep();
    let staged = quarantine.join("rom");
    let mut record = fs::File::create(quarantine.join("restore.json"))?;
    serde_json::to_writer_pretty(
        &mut record,
        &serde_json::json!({"original": rom, "file": "rom"}),
    )?;
    record.flush()?;
    record.sync_all()?;
    sync_directory(&quarantine)?;
    sync_directory(state)?;
    if let Err(error) = fs::rename(rom, &staged) {
        // A network filesystem may have applied a rename despite reporting an
        // I/O error. Keep its recovery record rather than guessing its outcome.
        return Err(error).with_context(|| {
            format!(
                "Déplacement de la ROM impossible ; dossier de récupération conservé : {}",
                quarantine.display()
            )
        });
    }
    let parent = rom.parent().context("Répertoire de ROM absent")?;
    let result = (|| {
        sync_directory(&quarantine)?;
        sync_directory(parent)?;
        publish()
    })();
    if let Err(error) = result {
        // hard_link fails rather than overwriting another application's new ROM.
        match fs::hard_link(&staged, rom) {
            Ok(()) => {
                fs::remove_file(&staged)?;
                sync_directory(parent)?;
                cleanup_quarantine(&quarantine);
                return Err(error.context("Désinstallation interrompue ; ROM restaurée"));
            }
            Err(restore_error) => bail!("Désinstallation interrompue : {error:#}. ROM conservée dans {} ; restauration manuelle nécessaire ({restore_error})", staged.display()),
        }
    }
    fs::remove_file(&staged).with_context(|| {
        format!(
            "Entrée XML retirée ; ROM encore conservée dans {}",
            staged.display()
        )
    })?;
    cleanup_quarantine(&quarantine);
    sync_directory(state)?;
    Ok(())
}

pub fn remove(
    root: &Path,
    game: &Game,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<()> {
    let config = Config {
        roms_dir: root.to_path_buf(),
        ..Config::default()
    };
    remove_configured(&config, game, cancel, progress)
}

pub fn remove_configured(
    config: &Config,
    game: &Game,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<()> {
    config.validate_layout()?;
    if cancel.load(Ordering::Relaxed) {
        bail!("Désinstallation annulée");
    }
    system_name(&game.system)?;
    system_name(config.local_system(&game.system))?;
    let relative = relative_path(&game.path)?;
    let root = config.roms_dir.canonicalize()?;
    let system = root.join(config.local_system(&game.system));
    let metadata = fs::symlink_metadata(&system)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("Système local non régulier");
    }
    let rom = local_path(&system, &relative)?;
    let state = directory(&system, Path::new(".romstore"))?;
    let lock_path = state.join("install.lock");
    existing_regular(&lock_path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.try_lock_exclusive()
        .context("Une autre opération est active pour ce système")?;
    let layout = Layout::new(config, &game.system)?;
    let original = read_xml(&layout.gamelist.join("gamelist.xml"))?;
    let mut xml = parse_xml(original.as_deref())?;
    let matches: Vec<_> = xml
        .children
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            let entry = node.as_element()?;
            if entry.name != "game" {
                return None;
            }
            let path = entry.get_child("path")?.get_text()?;
            references(&system, &path, &rom).then_some(index)
        })
        .collect();
    if matches.len() > 1 {
        bail!("Plusieurs entrées XML locales utilisent cette ROM ; aucune suppression");
    }
    if let Some(index) = matches.first() {
        xml.children.remove(*index);
    }
    // Other games/folders may refer to this ROM as a ROM or media, including aliases.
    for node in &xml.children {
        if let Some(entry) = node.as_element() {
            for field in &entry.children {
                if let Some(field) = field.as_element() {
                    if (field.name == "path"
                        || crate::install::MEDIA_TAGS.contains(&field.name.as_str()))
                        && field
                            .get_text()
                            .is_some_and(|text| references(&system, &text, &rom))
                    {
                        bail!("Cette ROM est référencée par une autre entrée XML ; aucune suppression");
                    }
                }
            }
        }
    }
    let merged = if matches.is_empty() {
        None
    } else {
        Some(encode_xml(&xml)?)
    };
    if cancel.load(Ordering::Relaxed) {
        bail!("Désinstallation annulée");
    }
    // After staging starts, finish or restore even if cancellation arrives.
    remove_with_xml(&rom, &state, || {
        if let Some(merged) = &merged {
            publish_gamelist(&layout.gamelist, &state, original.as_deref(), merged)
        } else {
            if read_xml(&layout.gamelist.join("gamelist.xml"))? != original {
                bail!("gamelist.xml modifié pendant la désinstallation");
            }
            Ok(())
        }
    })?;
    progress(Progress {
        completed: 1,
        total: 1,
        message: "Désinstallation terminée".into(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const ROM: &str = "games/Mario & Luigi's game.nes";
    const XML_PATH: &str = "./games/Mario &amp; Luigi's game.nes";

    fn fixture(xml: Option<&str>) -> (tempfile::TempDir, Game) {
        let temp = tempfile::tempdir().unwrap();
        let system = temp.path().join("roms/nes");
        fs::create_dir_all(system.join("games")).unwrap();
        fs::write(system.join(ROM), b"local ROM").unwrap();
        fs::write(system.join("other.nes"), b"another game").unwrap();
        fs::write(system.join("shared.png"), b"shared artwork").unwrap();
        fs::create_dir(temp.path().join("saves")).unwrap();
        fs::write(temp.path().join("saves/game.srm"), b"saved progress").unwrap();
        if let Some(xml) = xml {
            fs::write(system.join("gamelist.xml"), xml).unwrap();
        }
        (
            temp,
            Game {
                id: "a".repeat(64),
                installed: true,
                system: "nes".into(),
                path: ROM.into(),
                name: "Mario & Luigi".into(),
                metadata: BTreeMap::new(),
                installable: true,
                unavailable_reason: None,
                files: Vec::new(),
            },
        )
    }

    fn default_xml() -> String {
        format!("<gameList custom=\"keep\"><game id=\"local\"><path>{XML_PATH}</path><favorite>true</favorite><image>./shared.png</image></game><game id=\"other\"><path>./other.nes</path><favorite>true</favorite><custom field=\"keep\">local</custom><image>./shared.png</image></game><folder><path>./games</path><name>Folder</name></folder></gameList>")
    }

    #[test]
    fn remove_only_selected_rom_and_xml_entry_preserving_media_saves_and_backup() {
        let original = default_xml();
        let (temp, game) = fixture(Some(&original));
        let root = temp.path().join("roms");
        let system = root.join("nes");
        remove(&root, &game, &AtomicBool::new(false), |p| {
            assert_eq!((p.completed, p.total), (1, 1))
        })
        .unwrap();
        assert!(!system.join(ROM).exists());
        assert_eq!(
            fs::read(system.join("shared.png")).unwrap(),
            b"shared artwork"
        );
        assert_eq!(fs::read(system.join("other.nes")).unwrap(), b"another game");
        assert_eq!(
            fs::read(temp.path().join("saves/game.srm")).unwrap(),
            b"saved progress"
        );
        let xml = parse_xml(read_xml(&system.join("gamelist.xml")).unwrap().as_deref()).unwrap();
        assert_eq!(xml.attributes["custom"], "keep");
        let remaining: Vec<_> = xml
            .children
            .iter()
            .filter_map(|n| n.as_element())
            .filter(|e| e.name == "game")
            .collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].attributes["id"], "other");
        assert_eq!(
            remaining[0]
                .get_child("favorite")
                .unwrap()
                .get_text()
                .unwrap(),
            "true"
        );
        assert_eq!(
            remaining[0].get_child("custom").unwrap().attributes["field"],
            "keep"
        );
        assert!(xml.get_child("folder").is_some());
        let backups: Vec<_> = fs::read_dir(system.join(".romstore"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "xml"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(&backups[0]).unwrap(), original.as_bytes());
        assert!(!fs::read_dir(system.join(".romstore")).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("uninstall-")));
    }

    #[test]
    fn missing_xml_or_already_missing_rom_are_supported_without_creating_a_gamelist() {
        let (temp, game) = fixture(None);
        let root = temp.path().join("roms");
        remove(&root, &game, &AtomicBool::new(false), |_| {}).unwrap();
        remove(&root, &game, &AtomicBool::new(false), |_| {}).unwrap();
        assert!(!root.join("nes/gamelist.xml").exists());
        let (temp, game) = fixture(Some(&default_xml()));
        let root = temp.path().join("roms");
        fs::remove_file(root.join("nes").join(ROM)).unwrap();
        remove(&root, &game, &AtomicBool::new(false), |_| {}).unwrap();
        assert!(!fs::read_to_string(root.join("nes/gamelist.xml"))
            .unwrap()
            .contains("id=\"local\""));
    }

    #[test]
    fn removing_a_playlist_keeps_its_discs_as_unowned_auxiliary_files() {
        let (temp, mut game) = fixture(None);
        let root = temp.path().join("roms");
        game.path = "games/demo.m3u".into();
        fs::write(root.join("nes/games/demo.m3u"), "disc.chd\n").unwrap();
        fs::write(root.join("nes/games/disc.chd"), b"shared disc").unwrap();
        remove(&root, &game, &AtomicBool::new(false), |_| {}).unwrap();
        assert!(!root.join("nes/games/demo.m3u").exists());
        assert_eq!(
            fs::read(root.join("nes/games/disc.chd")).unwrap(),
            b"shared disc"
        );
    }

    #[test]
    fn invalid_or_duplicate_local_xml_prevents_any_removal() {
        let duplicate = format!("<gameList><game><path>{XML_PATH}</path></game><game><path>{XML_PATH}</path></game></gameList>");
        for original in [
            "<broken",
            "<wrong/>",
            "<!DOCTYPE gameList><gameList/>",
            duplicate.as_str(),
        ] {
            let (temp, game) = fixture(Some(original));
            let root = temp.path().join("roms");
            assert!(remove(&root, &game, &AtomicBool::new(false), |_| {}).is_err());
            assert_eq!(fs::read(root.join("nes").join(ROM)).unwrap(), b"local ROM");
            assert_eq!(
                fs::read_to_string(root.join("nes/gamelist.xml")).unwrap(),
                original
            );
        }
    }

    #[test]
    fn rom_referenced_as_another_games_media_is_not_removed() {
        let original = default_xml().replace(
            "<image>./shared.png</image></game><folder>",
            &format!("<image>{XML_PATH}</image></game><folder>"),
        );
        let (temp, game) = fixture(Some(&original));
        let root = temp.path().join("roms");
        let error = remove(&root, &game, &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(error.to_string().contains("autre entrée"));
        assert_eq!(fs::read(root.join("nes").join(ROM)).unwrap(), b"local ROM");
        assert_eq!(
            fs::read_to_string(root.join("nes/gamelist.xml")).unwrap(),
            original
        );
    }

    #[test]
    fn unsafe_paths_and_directories_cannot_be_removed() {
        for path in [
            "../../outside.nes",
            "/etc/passwd",
            ".romstore/install.lock",
            "gamelist.xml",
            "games",
        ] {
            let (temp, mut game) = fixture(Some(&default_xml()));
            game.path = path.into();
            let root = temp.path().join("roms");
            assert!(remove(&root, &game, &AtomicBool::new(false), |_| {}).is_err());
            assert_eq!(fs::read(root.join("nes").join(ROM)).unwrap(), b"local ROM");
        }
        let (temp, mut game) = fixture(None);
        game.system = "../nes".into();
        assert!(remove(
            &temp.path().join("roms"),
            &game,
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err());
    }

    #[test]
    fn cancel_and_competing_installation_keep_rom_and_xml_untouched() {
        let original = default_xml();
        let (temp, game) = fixture(Some(&original));
        let root = temp.path().join("roms");
        assert!(remove(&root, &game, &AtomicBool::new(true), |_| {}).is_err());
        let state = root.join("nes/.romstore");
        fs::create_dir(&state).unwrap();
        let lock = fs::File::create(state.join("install.lock")).unwrap();
        lock.try_lock_exclusive().unwrap();
        assert!(remove(&root, &game, &AtomicBool::new(false), |_| {}).is_err());
        assert_eq!(fs::read(root.join("nes").join(ROM)).unwrap(), b"local ROM");
        assert_eq!(
            fs::read_to_string(root.join("nes/gamelist.xml")).unwrap(),
            original
        );
    }

    #[test]
    fn xml_conflict_restores_rom_and_never_overwrites_external_xml_changes() {
        let original = default_xml();
        let (temp, _) = fixture(Some(&original));
        let system = temp.path().join("roms/nes");
        let state = system.join(".romstore");
        fs::create_dir(&state).unwrap();
        let rom = system.join(ROM);
        let external = b"<gameList external=\"changed\"/>";
        assert!(remove_with_xml(&rom, &state, || {
            fs::write(system.join("gamelist.xml"), external)?;
            publish_gamelist(&system, &state, Some(original.as_bytes()), b"<gameList/>")
        })
        .is_err());
        assert_eq!(fs::read(&rom).unwrap(), b"local ROM");
        assert_eq!(fs::read(system.join("gamelist.xml")).unwrap(), external);
    }

    #[test]
    fn failed_restore_never_clobbers_a_concurrent_rom_and_preserves_original_in_quarantine() {
        let (temp, _) = fixture(None);
        let system = temp.path().join("roms/nes");
        let state = system.join(".romstore");
        fs::create_dir(&state).unwrap();
        let rom = system.join(ROM);
        let error = remove_with_xml(&rom, &state, || {
            fs::write(&rom, b"external replacement")?;
            bail!("simulated publication failure")
        })
        .unwrap_err();
        assert!(error.to_string().contains("restauration manuelle"));
        assert_eq!(fs::read(&rom).unwrap(), b"external replacement");
        let retained = fs::read_dir(&state)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(retained.join("rom")).unwrap(), b"local ROM");
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(retained.join("restore.json")).unwrap()).unwrap();
        assert_eq!(record["original"], rom.to_string_lossy().as_ref());
    }

    #[test]
    fn unwinding_during_publication_leaves_recoverable_data_instead_of_deleting_it() {
        let (temp, _) = fixture(None);
        let system = temp.path().join("roms/nes");
        let state = system.join(".romstore");
        fs::create_dir(&state).unwrap();
        let rom = system.join(ROM);
        assert!(std::panic::catch_unwind(|| {
            let _ = remove_with_xml(&rom, &state, || panic!("simulated crash"));
        })
        .is_err());
        assert!(!rom.exists());
        let retained = fs::read_dir(&state)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(retained.join("rom")).unwrap(), b"local ROM");
        assert!(retained.join("restore.json").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_rom_parent_xml_and_state_are_rejected() {
        use std::os::unix::fs::symlink;
        for target in ["rom", "parent", "xml", "state"] {
            let (temp, game) = fixture(Some(&default_xml()));
            let root = temp.path().join("roms");
            let system = root.join("nes");
            match target {
                "rom" => {
                    fs::rename(system.join(ROM), temp.path().join("saved-rom")).unwrap();
                    symlink(temp.path().join("saved-rom"), system.join(ROM)).unwrap();
                }
                "parent" => {
                    fs::rename(system.join("games"), system.join("original-games")).unwrap();
                    symlink(system.join("original-games"), system.join("games")).unwrap();
                }
                "xml" => {
                    fs::rename(system.join("gamelist.xml"), temp.path().join("saved.xml")).unwrap();
                    symlink(temp.path().join("saved.xml"), system.join("gamelist.xml")).unwrap();
                }
                _ => {
                    let outside = temp.path().join("outside-state");
                    fs::create_dir(&outside).unwrap();
                    symlink(outside, system.join(".romstore")).unwrap();
                }
            }
            assert!(remove(&root, &game, &AtomicBool::new(false), |_| {}).is_err());
            assert_eq!(fs::read(system.join(ROM)).unwrap(), b"local ROM");
        }
    }
}
