//! Verified, non-destructive installation. Published files are never rolled back:
//! a retained journal and a repeated installation finish an interrupted merge.
use crate::api::{validate_id, Api, File, Manifest};
use crate::{config::Config, layout::Layout};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tempfile::NamedTempFile;
use xmltree::{Element, EmitterConfig, XMLNode};

const MAX_XML: u64 = 16 * 1024 * 1024;
const LOCAL_TAGS: &[&str] = &["favorite", "hidden", "playcount", "lastplayed", "gametime"];
pub(crate) const MEDIA_TAGS: &[&str] = &[
    "image",
    "screenshot",
    "thumbnail",
    "video",
    "marquee",
    "fanart",
    "manual",
    "titleshot",
    "boxart",
    "boxback",
    "cartridge",
    "wheel",
    "map",
    "bezel",
];

#[derive(Clone, Debug)]
pub struct Progress {
    pub completed: u64,
    pub total: u64,
    pub message: String,
}

pub fn relative_path(raw: &str) -> Result<PathBuf> {
    if raw.is_empty() || raw.contains(['\\', ':', '\0']) || raw.starts_with('/') {
        bail!("Chemin relatif invalide");
    }
    let path = Path::new(raw);
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => (),
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                if name.starts_with(".romstore")
                    || name == "gamelist.xml"
                    || name == "gamelist.xml.bak"
                {
                    bail!("Destination réservée");
                }
                result.push(name.as_ref());
            }
            _ => bail!("Chemin hors système"),
        }
    }
    if result.as_os_str().is_empty() {
        bail!("Chemin vide");
    }
    Ok(result)
}

pub(crate) fn system_name(name: &str) -> Result<()> {
    let path = relative_path(name)?;
    if path.components().count() != 1 || name.starts_with('.') || path.to_str() != Some(name) {
        bail!("Identifiant système invalide");
    }
    Ok(())
}

/// Presence only: no hashing of large ROMs and no writes to the local library.
pub fn rom_exists(root: &Path, system: &str, path: &str) -> bool {
    let check = || -> Result<bool> {
        system_name(system)?;
        let relative = relative_path(path)?;
        let mut current = root.canonicalize()?;
        for component in Path::new(system).join(&relative).components() {
            current.push(component);
            let metadata = fs::symlink_metadata(&current)?;
            if metadata.file_type().is_symlink() {
                return Ok(false);
            }
        }
        if !fs::metadata(&current)?.is_file() {
            return Ok(false);
        }
        if crate::playlist::is_playlist(path) {
            let bytes = crate::playlist::read(&current)?;
            let discs =
                crate::playlist::parse(relative.to_str().context("Chemin non UTF-8")?, &bytes)?;
            return Ok(discs.iter().all(|disc| {
                disc.to_str()
                    .is_some_and(|disc| rom_exists(root, system, disc))
            }));
        }
        Ok(true)
    };
    check().unwrap_or(false)
}

pub fn validate_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.version != 1 || manifest.files.is_empty() || manifest.files.len() > 256 {
        bail!("Manifeste non pris en charge");
    }
    validate_id(&manifest.game_id)?;
    validate_id(&manifest.revision)?;
    system_name(&manifest.system)?;
    let rom_path = relative_path(&manifest.path)?;
    let playlist = crate::playlist::is_playlist(&manifest.path);
    let mut destinations = BTreeMap::new();
    let mut roms = 0;
    let mut discs = HashSet::new();
    let mut total = 0_u64;
    for file in &manifest.files {
        validate_id(&file.id)?;
        validate_id(&file.sha256)?;
        let destination = relative_path(&file.destination)?;
        total = total
            .checked_add(file.size)
            .context("Taille totale invalide")?;
        if let Some(previous) = destinations.insert(destination.clone(), (&file.sha256, file.size))
        {
            if previous != (&file.sha256, file.size) {
                bail!("Destinations contradictoires");
            }
        }
        if file.role == "rom" {
            roms += 1;
            if destination != rom_path {
                bail!("Le chemin de la ROM ne correspond pas au manifeste");
            }
            if playlist && file.size > crate::playlist::MAX_PLAYLIST {
                bail!("M3U trop volumineux");
            }
        } else if file.role == "disk" {
            if !playlist || destination == rom_path || crate::playlist::dependent(&file.destination)
            {
                bail!("Disque non pris en charge");
            }
            if !discs.insert(destination) {
                bail!("Disque dupliqué dans le manifeste");
            }
        } else if !MEDIA_TAGS.contains(&file.role.as_str()) {
            bail!("Rôle de fichier non pris en charge");
        }
    }
    if roms != 1 {
        bail!("Le manifeste doit contenir exactement une ROM");
    }
    if playlist && (discs.is_empty() || discs.len() > crate::playlist::MAX_DISCS) {
        bail!("Le manifeste M3U doit contenir de 1 à 240 disques");
    }
    for (tag, text) in &manifest.metadata {
        if tag.is_empty()
            || tag.len() > 128
            || text.len() > 1024 * 1024
            || !tag.bytes().enumerate().all(|(i, c)| {
                c == b'_'
                    || c.is_ascii_alphabetic()
                    || (i > 0 && (c.is_ascii_digit() || c == b'-' || c == b'.'))
            })
        {
            bail!("Métadonnée invalide");
        }
        if tag == "path" {
            bail!("Métadonnée path interdite");
        }
        if MEDIA_TAGS.contains(&tag.as_str()) {
            let path = relative_path(text)?;
            if !manifest.files.iter().any(|f| {
                f.role == *tag && relative_path(&f.destination).ok().as_ref() == Some(&path)
            }) {
                bail!("Média sans fichier correspondant");
            }
        }
    }
    Ok(())
}

/// Each existing component must be a real directory, never a symlink.
pub(crate) fn directory(root: &Path, relative: &Path) -> Result<PathBuf> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            bail!("Composant de répertoire invalide");
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => (),
            Ok(_) => bail!(
                "Répertoire remplacé ou lien symbolique : {}",
                current.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(current)
}

pub(crate) fn existing_regular(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            bail!(
                "Fichier non régulier ou lien symbolique : {}",
                path.display()
            )
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn digest(path: &Path) -> Result<String> {
    let mut stream = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let size = stream.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hasher.update(&buffer[..size]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn matches_file(path: &Path, file: &File) -> Result<bool> {
    Ok(fs::metadata(path)?.len() == file.size && digest(path)? == file.sha256.to_lowercase())
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Installation annulée ; relancer pour terminer si des fichiers ont été publiés");
    }
    Ok(())
}

pub fn install(
    api: &Api,
    root: &Path,
    manifest: &Manifest,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<()> {
    let config = Config {
        roms_dir: root.to_path_buf(),
        ..Config::default()
    };
    install_configured(api, &config, manifest, cancel, progress)
}

pub fn install_configured(
    api: &Api,
    config: &Config,
    manifest: &Manifest,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<()> {
    install_with_space(
        api,
        config,
        manifest,
        cancel,
        progress,
        crate::space::snapshot,
    )
}

fn install_with_space(
    api: &Api,
    config: &Config,
    manifest: &Manifest,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
    probe: impl Fn(&Path) -> Result<crate::space::Snapshot>,
) -> Result<()> {
    validate_manifest(manifest)?;
    let layout = Layout::new(config, &manifest.system)?;
    let system = &layout.system;
    let state = directory(system, Path::new(".romstore"))?;
    let lock_path = state.join("install.lock");
    existing_regular(&lock_path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.try_lock_exclusive()
        .context("Une installation est déjà active pour ce système")?;
    cancelled(cancel)?;

    let mut unique = HashSet::new();
    let mut pending = Vec::new();
    let mut destinations = BTreeMap::new();
    for (file, root, relative) in layout.files(manifest)? {
        let destination = root.join(&relative);
        if let Some(previous) = destinations.insert(destination.clone(), (&file.sha256, file.size))
        {
            if previous != (&file.sha256, file.size) {
                bail!(
                    "Destinations locales contradictoires : {}",
                    destination.display()
                );
            }
        }
        if !unique.insert(destination) {
            continue;
        }
        let parent = directory(&root, relative.parent().unwrap_or(Path::new("")))?;
        let target = parent.join(relative.file_name().context("Destination vide")?);
        if existing_regular(&target)? {
            if !matches_file(&target, file)? {
                bail!("Conflit local, aucun écrasement : {}", target.display());
            }
        } else {
            pending.push((file, root, target));
        }
    }
    // Validate the existing XML before transferring any large file.
    let baseline = read_xml(&layout.gamelist.join("gamelist.xml"))?;
    let merged = merge_local_xml(baseline.as_deref(), manifest, layout.media.is_some())?;
    let total: u64 = pending.iter().map(|(f, _, _)| f.size).sum();
    let journal_bytes = serde_json::to_vec_pretty(manifest)?;
    // Include XML replacement, its retained backup and the recovery journal.
    // Destination parents can themselves be mount points: inspect the real target,
    // rather than assuming every child belongs to its configured root's volume.
    let mut required = vec![
        (layout.gamelist.clone(), merged.len() as u64),
        (
            state.clone(),
            baseline.as_ref().map_or(0, |bytes| bytes.len() as u64),
        ),
        (state.clone(), journal_bytes.len() as u64),
    ];
    for (file, _, target) in &pending {
        required.push((
            target
                .parent()
                .context("Répertoire cible absent")?
                .to_path_buf(),
            file.size,
        ));
    }
    crate::space::ensure(required, &probe)?;
    drop(merged);
    drop(baseline);

    let journal_path = state.join(format!("{}.json", manifest.game_id));
    existing_regular(&journal_path)?;
    let mut journal = NamedTempFile::new_in(&state)?;
    journal.write_all(&journal_bytes)?;
    drop(journal_bytes);
    journal.as_file().sync_all()?;
    journal.persist(&journal_path)?;
    sync_directory(&state)?;

    let mut completed = 0;
    let mut staged = Vec::new();
    for (file, target_root, target) in pending {
        cancelled(cancel)?;
        // Available space can change after the initial check (another process,
        // another queued install): refuse before opening this file's HTTP request.
        crate::space::ensure(
            vec![(
                target
                    .parent()
                    .context("Répertoire cible absent")?
                    .to_path_buf(),
                file.size,
            )],
            &probe,
        )?;
        // Stage on the destination filesystem for atomic publication across mounts.
        let mut temporary = tempfile::Builder::new()
            .prefix(".romstore-download-")
            .tempfile_in(target.parent().context("Répertoire cible absent")?)?;
        let mut response = api.file(file)?;
        if let Some(size) = response.content_length() {
            if size != file.size {
                bail!("Taille HTTP incorrecte");
            }
        }
        let mut size = 0_u64;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 128 * 1024];
        loop {
            cancelled(cancel)?;
            let count = response.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            size = size
                .checked_add(count as u64)
                .context("Fichier trop volumineux")?;
            if size > file.size {
                bail!("Le serveur a envoyé trop de données");
            }
            temporary.write_all(&buffer[..count])?;
            hasher.update(&buffer[..count]);
            completed += count as u64;
            progress(Progress {
                completed,
                total,
                message: file.destination.clone(),
            });
        }
        if size != file.size || hex::encode(hasher.finalize()) != file.sha256.to_lowercase() {
            bail!(
                "Fichier incomplet ou empreinte incorrecte : {}",
                file.destination
            );
        }
        temporary.as_file().sync_all()?;
        staged.push((temporary, target_root, target));
    }
    // Verify that the downloaded (or reused) playlist names exactly the manifest's
    // discs before publishing anything, including during journal recovery.
    if crate::playlist::is_playlist(&manifest.path) {
        let target = system.join(relative_path(&manifest.path)?);
        let source = staged
            .iter()
            .find(|(_, _, path)| path == &target)
            .map(|(temporary, _, _)| temporary.path())
            .unwrap_or(&target);
        let bytes = crate::playlist::read(source)?;
        let referenced: HashSet<_> = crate::playlist::parse(&manifest.path, &bytes)?
            .into_iter()
            .collect();
        let declared: HashSet<_> = manifest
            .files
            .iter()
            .filter(|file| file.role == "disk")
            .map(|file| relative_path(&file.destination))
            .collect::<Result<_>>()?;
        if referenced != declared {
            bail!("Les disques du M3U ne correspondent pas au manifeste");
        }
        // Publish the entry point last so an interrupted transfer cannot expose
        // a new playlist before its discs are available.
        staged.sort_by_key(|(_, _, path)| path == &target);
    }
    cancelled(cancel)?;
    for (temporary, target_root, target) in staged {
        // Recheck parent paths and use no-clobber publication.
        let relative = target.strip_prefix(&target_root)?;
        directory(&target_root, relative.parent().unwrap_or(Path::new("")))?;
        temporary
            .persist_noclobber(&target)
            .with_context(|| format!("Conflit pendant la publication : {}", target.display()))?;
        sync_directory(target.parent().context("Répertoire cible absent")?)?;
    }
    // Once publishing begins, finish the XML merge even if cancellation arrives.
    update_gamelist(&layout.gamelist, &state, manifest, layout.media.is_some())?;
    fs::remove_file(journal_path)?;
    sync_directory(&state)?;
    progress(Progress {
        completed: total,
        total,
        message: "Installation terminée".into(),
    });
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()?;
        Ok(())
    }
    // Rust's ordinary File::open cannot open a directory on Windows. The file
    // contents are already flushed before each atomic rename/publication.
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

pub(crate) fn read_xml(path: &Path) -> Result<Option<Vec<u8>>> {
    if !existing_regular(path)? {
        return Ok(None);
    }
    if fs::metadata(path)?.len() > MAX_XML {
        bail!("gamelist.xml trop volumineux");
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > MAX_XML {
        bail!("gamelist.xml trop volumineux");
    }
    Ok(Some(bytes))
}

pub(crate) fn parse_xml(original: Option<&[u8]>) -> Result<Element> {
    let root = match original {
        Some(bytes) => {
            // Only UTF-8 XML is accepted. Reject DTDs/entities before XML parsing.
            let text = std::str::from_utf8(bytes).context("Le XML local doit être UTF-8")?;
            let upper = text.to_ascii_uppercase();
            if upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY") {
                bail!("DTD et entités XML interdites");
            }
            Element::parse(bytes).context("gamelist.xml local invalide ; aucun remplacement")?
        }
        None => Element::new("gameList"),
    };
    if root.name != "gameList" {
        bail!("Racine XML locale invalide");
    }
    Ok(root)
}

pub fn merge_xml(original: Option<&[u8]>, manifest: &Manifest) -> Result<Vec<u8>> {
    merge_local_xml(original, manifest, false)
}

fn merge_local_xml(original: Option<&[u8]>, manifest: &Manifest, esde: bool) -> Result<Vec<u8>> {
    validate_manifest(manifest)?;
    let mut root = parse_xml(original)?;
    let wanted = relative_path(&manifest.path)?;
    let matches: Vec<usize> = root
        .children
        .iter()
        .enumerate()
        .filter_map(|(i, node)| {
            let game = node.as_element()?;
            if game.name != "game" {
                return None;
            }
            let path = game.get_child("path")?.get_text()?;
            (relative_path(&path).ok().as_ref() == Some(&wanted)).then_some(i)
        })
        .collect();
    if matches.len() > 1 {
        bail!("Plusieurs entrées XML locales ont le même chemin");
    }
    let index = if let Some(index) = matches.first() {
        *index
    } else {
        root.children.push(XMLNode::Element(Element::new("game")));
        root.children.len() - 1
    };
    let game = root.children[index]
        .as_mut_element()
        .context("Entrée XML invalide")?;
    let mut values = manifest.metadata.clone();
    values.retain(|tag, _| !LOCAL_TAGS.contains(&tag.as_str()));
    if esde {
        values.retain(|tag, _| !MEDIA_TAGS.contains(&tag.as_str()));
        game.children.retain(|node| {
            !node
                .as_element()
                .is_some_and(|field| MEDIA_TAGS.contains(&field.name.as_str()))
        });
    }
    values.insert("path".into(), format!("./{}", wanted.to_string_lossy()));
    // Preserve attributes of existing fields and all unrelated nodes.
    for (tag, text) in values {
        let mut found = false;
        game.children.retain_mut(|node| {
            if let Some(element) = node.as_mut_element() {
                if element.name == tag {
                    if found {
                        return false;
                    }
                    element.children = vec![XMLNode::Text(text.clone())];
                    found = true;
                }
            }
            true
        });
        if !found {
            let mut element = Element::new(&tag);
            element.children.push(XMLNode::Text(text));
            game.children.push(XMLNode::Element(element));
        }
    }
    encode_xml(&root)
}

pub(crate) fn encode_xml(root: &Element) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    root.write_with_config(&mut output, EmitterConfig::new().perform_indent(true))?;
    Element::parse(output.as_slice())?;
    if output.len() as u64 > MAX_XML {
        bail!("XML résultant trop volumineux");
    }
    Ok(output)
}

fn update_gamelist(system: &Path, state: &Path, manifest: &Manifest, esde: bool) -> Result<()> {
    let path = system.join("gamelist.xml");
    let original = read_xml(&path)?;
    let merged = merge_local_xml(original.as_deref(), manifest, esde)?;
    publish_gamelist(system, state, original.as_deref(), &merged)
}

pub(crate) fn publish_gamelist(
    system: &Path,
    state: &Path,
    original: Option<&[u8]>,
    merged: &[u8],
) -> Result<()> {
    let path = system.join("gamelist.xml");
    if let Some(bytes) = original {
        let mut backup = tempfile::Builder::new()
            .prefix("gamelist-backup-")
            .suffix(".xml")
            .tempfile_in(state)?;
        backup.write_all(bytes)?;
        backup.as_file().sync_all()?;
        backup.keep()?;
        sync_directory(state)?;
    }
    let mut temporary = NamedTempFile::new_in(system)?;
    temporary.write_all(merged)?;
    temporary.as_file().sync_all()?;
    if read_xml(&path)?.as_deref() != original {
        bail!("gamelist.xml modifié par un autre programme ; relancer l'opération");
    }
    if original.is_none() {
        temporary.persist_noclobber(&path)?;
    } else {
        temporary.persist(&path)?;
    }
    sync_directory(system)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_rom_presence_handles_nested_paths_without_writing_or_escaping() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("nes/subdir")).unwrap();
        fs::write(
            root.path().join("nes/subdir/Mario & Luigi's game.nes"),
            b"rom",
        )
        .unwrap();
        assert!(rom_exists(
            root.path(),
            "nes",
            "./subdir/Mario & Luigi's game.nes"
        ));
        assert!(!rom_exists(root.path(), "nes", "missing.nes"));
        assert!(!rom_exists(root.path(), "nes", "subdir"));
        assert!(!rom_exists(root.path(), "../nes", "subdir/game.nes"));
        assert!(!rom_exists(root.path(), "nes", "../outside.nes"));
        assert!(!rom_exists(root.path(), "nes", "/etc/passwd"));
        assert!(!rom_exists(root.path(), "missing", "game.nes"));
        assert!(!root.path().join("missing").exists());
        fs::remove_file(root.path().join("nes/subdir/Mario & Luigi's game.nes")).unwrap();
        assert!(!rom_exists(
            root.path(),
            "nes",
            "subdir/Mario & Luigi's game.nes"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn local_rom_presence_rejects_symlinked_files_and_subdirectories() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("game.nes"), b"rom").unwrap();
        fs::create_dir(root.path().join("nes")).unwrap();
        symlink(
            outside.path().join("game.nes"),
            root.path().join("nes/game.nes"),
        )
        .unwrap();
        symlink(outside.path(), root.path().join("nes/linked")).unwrap();
        symlink(outside.path(), root.path().join("snes")).unwrap();
        assert!(!rom_exists(root.path(), "nes", "game.nes"));
        assert!(!rom_exists(root.path(), "nes", "linked/game.nes"));
        assert!(!rom_exists(root.path(), "snes", "game.nes"));
    }

    pub fn manifest() -> Manifest {
        Manifest {
            version: 1,
            game_id: "a".repeat(64),
            revision: "b".repeat(64),
            system: "nes".into(),
            path: "games/demo.nes".into(),
            metadata: BTreeMap::from([("name".into(), "Démo & test".into())]),
            files: vec![File {
                id: "c".repeat(64),
                role: "rom".into(),
                destination: "games/demo.nes".into(),
                size: 3,
                sha256: hex::encode(Sha256::digest(b"rom")),
            }],
        }
    }

    #[test]
    fn insufficient_space_stops_before_any_payload_download_or_journal_publication() {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            roms_dir: root.path().into(),
            ..Config::default()
        };
        let api = Api::new("http://127.0.0.1:1").unwrap();
        let error = install_with_space(
            &api,
            &config,
            &manifest(),
            &AtomicBool::new(false),
            |_| panic!("No transfer should start"),
            |_| {
                Ok(crate::space::Snapshot {
                    volume: "disk".into(),
                    available: 0,
                    allocation: 4096,
                })
            },
        )
        .unwrap_err();
        assert!(error
            .downcast_ref::<crate::space::InsufficientSpace>()
            .is_some());
        assert!(!root.path().join("nes/games/demo.nes").exists());
        assert!(!root.path().join("nes/gamelist.xml").exists());
        assert!(!root
            .path()
            .join(format!("nes/.romstore/{}.json", "a".repeat(64)))
            .exists());
    }

    #[test]
    fn space_is_checked_again_immediately_before_the_http_file_request() {
        use std::cell::Cell;
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            roms_dir: root.path().into(),
            ..Config::default()
        };
        let api = Api::new("http://127.0.0.1:1").unwrap();
        let calls = Cell::new(0);
        let error = install_with_space(
            &api,
            &config,
            &manifest(),
            &AtomicBool::new(false),
            |_| panic!("No transfer should start"),
            |_| {
                calls.set(calls.get() + 1);
                Ok(crate::space::Snapshot {
                    volume: "disk".into(),
                    available: if calls.get() <= 4 { u64::MAX } else { 0 },
                    allocation: 1,
                })
            },
        )
        .unwrap_err();
        assert!(error
            .downcast_ref::<crate::space::InsufficientSpace>()
            .is_some());
        assert_eq!(calls.get(), 5);
        assert!(!root.path().join("nes/games/demo.nes").exists());
        assert!(root
            .path()
            .join(format!("nes/.romstore/{}.json", "a".repeat(64)))
            .exists());
    }

    #[test]
    fn paths_reject_escape_and_reserved_files() {
        for value in [
            "",
            ".",
            "../rom",
            "/rom",
            "C:/rom",
            "a/../../rom",
            "a\\rom",
            ".romstore/journal",
            "gamelist.xml",
        ] {
            assert!(relative_path(value).is_err(), "{value}");
        }
        assert_eq!(
            relative_path("./games/demo.nes").unwrap(),
            PathBuf::from("games/demo.nes")
        );
    }

    fn multidisc_manifest() -> (Manifest, Vec<(&'static str, &'static [u8])>) {
        let contents: Vec<(&str, &[u8])> = vec![
            ("games/demo.m3u", b"\xef\xbb\xbf#EXTM3U\r\ndiscs/Disc 1.chd\r\n../Disc 2.rvz\r\n./discs/Disc 1.chd\r\n"),
            ("games/discs/Disc 1.chd", b"disc one"),
            ("Disc 2.rvz", b"disc two"),
        ];
        let mut m = manifest();
        m.path = "games/demo.m3u".into();
        m.files = contents
            .iter()
            .enumerate()
            .map(|(index, (destination, bytes))| File {
                id: format!("{index:064x}"),
                role: if index == 0 { "rom" } else { "disk" }.into(),
                destination: (*destination).into(),
                size: bytes.len() as u64,
                sha256: hex::encode(Sha256::digest(bytes)),
            })
            .collect();
        (m, contents)
    }

    #[test]
    fn multidisc_install_reuses_verified_files_and_merges_only_the_playlist() {
        let root = tempfile::tempdir().unwrap();
        let (m, contents) = multidisc_manifest();
        for (destination, bytes) in &contents {
            let target = root.path().join("nes").join(destination);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, bytes).unwrap();
        }
        let api = Api::new("http://127.0.0.1:1").unwrap();
        for _ in 0..2 {
            install(&api, root.path(), &m, &AtomicBool::new(false), |_| {}).unwrap();
        }
        assert!(rom_exists(root.path(), "nes", &m.path));
        let xml = parse_xml(
            read_xml(&root.path().join("nes/gamelist.xml"))
                .unwrap()
                .as_deref(),
        )
        .unwrap();
        assert_eq!(
            xml.children
                .iter()
                .filter(|node| node.as_element().is_some_and(|entry| entry.name == "game"))
                .count(),
            1
        );
        assert_eq!(
            xml.get_child("game")
                .unwrap()
                .get_child("path")
                .unwrap()
                .get_text()
                .unwrap(),
            "./games/demo.m3u"
        );
        for (destination, bytes) in &contents {
            assert_eq!(
                fs::read(root.path().join("nes").join(destination)).unwrap(),
                *bytes
            );
        }
        fs::remove_file(root.path().join("nes/Disc 2.rvz")).unwrap();
        assert!(!rom_exists(root.path(), "nes", &m.path));
    }

    #[test]
    fn multidisc_manifest_rejects_missing_duplicate_unsafe_or_unrelated_discs() {
        let (m, _) = multidisc_manifest();
        validate_manifest(&m).unwrap();
        let mut invalid = m.clone();
        invalid.files.retain(|file| file.role != "disk");
        assert!(validate_manifest(&invalid).is_err());
        for destination in [
            "../escape.chd",
            "games/demo.m3u",
            "disc.cue",
            "disc.m3u8",
            "gamelist.xml",
        ] {
            let mut invalid = m.clone();
            invalid.files[1].destination = destination.into();
            assert!(validate_manifest(&invalid).is_err(), "{destination}");
        }
        let mut invalid = m.clone();
        invalid.files.push(invalid.files[1].clone());
        assert!(validate_manifest(&invalid).is_err());
        let mut invalid = manifest();
        invalid.files.push(m.files[1].clone());
        assert!(validate_manifest(&invalid).is_err());
    }

    #[test]
    fn local_multidisc_presence_rejects_invalid_playlists_and_symlinked_discs() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("psx")).unwrap();
        for contents in [
            "",
            "#EXTM3U",
            "missing.chd",
            "../outside.chd",
            "/etc/passwd",
            "loop.m3u",
            "disc.cue",
        ] {
            fs::write(root.path().join("psx/demo.m3u"), contents).unwrap();
            assert!(!rom_exists(root.path(), "psx", "demo.m3u"), "{contents}");
        }
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("disc.chd"), b"disc").unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("disc.chd"),
                root.path().join("psx/disc.chd"),
            )
            .unwrap();
            fs::write(root.path().join("psx/demo.m3u"), "disc.chd").unwrap();
            assert!(!rom_exists(root.path(), "psx", "demo.m3u"));
        }
    }

    #[test]
    fn mismatched_playlist_blocks_xml_publication_even_with_verified_files() {
        let root = tempfile::tempdir().unwrap();
        let (mut m, contents) = multidisc_manifest();
        for (destination, bytes) in contents {
            let target = root.path().join("nes").join(destination);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, bytes).unwrap();
        }
        fs::write(root.path().join("nes/unrelated.chd"), b"disc one").unwrap();
        m.files[1].destination = "unrelated.chd".into();
        let api = Api::new("http://127.0.0.1:1").unwrap();
        let error = install(&api, root.path(), &m, &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(error.to_string().contains("ne correspondent pas"));
        assert!(!root.path().join("nes/gamelist.xml").exists());
        fs::write(root.path().join("nes/Disc 2.rvz"), b"different disc").unwrap();
        let (m, _) = multidisc_manifest();
        assert!(
            install(&api, root.path(), &m, &AtomicBool::new(false), |_| {})
                .unwrap_err()
                .to_string()
                .contains("Conflit local")
        );
    }

    #[test]
    fn merge_preserves_local_fields_unknown_nodes_and_attributes() {
        let xml = br#"<gameList custom="yes"><folder><path>./folder</path></folder>
            <game id="local"><path>./games/demo.nes</path><name source="local">Old</name>
            <favorite>true</favorite><playcount>42</playcount><custom><nested/></custom></game>
            <game><path>./other.nes</path><name>Other</name></game></gameList>"#;
        let output = merge_xml(Some(xml), &manifest()).unwrap();
        let root = Element::parse(output.as_slice()).unwrap();
        assert_eq!(root.attributes["custom"], "yes");
        let game = root.get_child("game").unwrap();
        assert_eq!(game.attributes["id"], "local");
        assert_eq!(
            game.get_child("name").unwrap().attributes["source"],
            "local"
        );
        assert_eq!(
            game.get_child("favorite").unwrap().get_text().unwrap(),
            "true"
        );
        assert_eq!(
            game.get_child("playcount").unwrap().get_text().unwrap(),
            "42"
        );
        assert!(game
            .get_child("custom")
            .unwrap()
            .get_child("nested")
            .is_some());
        assert_eq!(output, merge_xml(Some(&output), &manifest()).unwrap());
    }

    #[test]
    fn xml_creation_and_invalid_xml() {
        let m = manifest();
        assert!(merge_xml(None, &m).is_ok());
        for xml in ["<broken", "<other/>", "<!DOCTYPE gameList><gameList/>"] {
            assert!(merge_xml(Some(xml.as_bytes()), &m).is_err());
        }
    }

    #[test]
    fn invalid_manifests_rejected() {
        let mut m = manifest();
        m.files[0].destination = "../escape".into();
        assert!(validate_manifest(&m).is_err());
        let mut m = manifest();
        m.metadata.insert("image".into(), "./secret.png".into());
        assert!(validate_manifest(&m).is_err());
        let mut m = manifest();
        m.system = "../nes".into();
        assert!(validate_manifest(&m).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_directory_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("nes")).unwrap();
        assert!(directory(root.path(), Path::new("nes")).is_err());
    }

    #[test]
    fn existing_verified_files_can_be_installed_offline() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("nes/games")).unwrap();
        fs::write(root.path().join("nes/games/demo.nes"), b"rom").unwrap();
        let api = Api::new("http://127.0.0.1:1").unwrap();
        for _ in 0..2 {
            install(
                &api,
                root.path(),
                &manifest(),
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap();
        }
        let xml = fs::read(root.path().join("nes/gamelist.xml")).unwrap();
        let root = Element::parse(xml.as_slice()).unwrap();
        assert_eq!(
            root.children
                .iter()
                .filter(|n| n.as_element().is_some_and(|e| e.name == "game"))
                .count(),
            1
        );
    }

    #[test]
    fn screenshots_are_verified_installed_and_preserved_in_the_xml() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("nes/games")).unwrap();
        fs::create_dir_all(root.path().join("nes/images")).unwrap();
        fs::write(root.path().join("nes/games/demo.nes"), b"rom").unwrap();
        let mut m = manifest();
        for role in ["boxart", "screenshot", "titleshot", "marquee", "fanart"] {
            let destination = format!("images/{role}.png");
            fs::write(root.path().join("nes").join(&destination), b"image").unwrap();
            m.metadata.insert(role.into(), format!("./{destination}"));
            m.files.push(File {
                id: "d".repeat(64),
                role: role.into(),
                destination,
                size: 5,
                sha256: hex::encode(Sha256::digest(b"image")),
            });
        }
        validate_manifest(&m).unwrap();
        let api = Api::new("http://127.0.0.1:1").unwrap();
        for _ in 0..2 {
            install(&api, root.path(), &m, &AtomicBool::new(false), |_| {}).unwrap();
        }
        let xml = fs::read(root.path().join("nes/gamelist.xml")).unwrap();
        let tree = Element::parse(xml.as_slice()).unwrap();
        let game = tree.get_child("game").unwrap();
        for role in ["boxart", "screenshot", "titleshot", "marquee", "fanart"] {
            assert_eq!(
                game.get_child(role).unwrap().get_text().unwrap(),
                format!("./images/{role}.png")
            );
        }
        m.files.retain(|file| file.role != "screenshot");
        assert!(validate_manifest(&m).is_err());
    }

    #[test]
    fn cancelled_install_does_not_publish() {
        let root = tempfile::tempdir().unwrap();
        let api = Api::new("http://127.0.0.1:1").unwrap();
        assert!(install(
            &api,
            root.path(),
            &manifest(),
            &AtomicBool::new(true),
            |_| {}
        )
        .is_err());
        assert!(!root.path().join("nes/gamelist.xml").exists());
        assert!(!root.path().join("nes/games/demo.nes").exists());
    }

    #[test]
    fn corrupt_download_keeps_journal_but_does_not_publish() {
        use std::{net::TcpListener, thread, time::Duration};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = Api::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad")
                .unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let error = install(
            &api,
            root.path(),
            &manifest(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("empreinte incorrecte"));
        server.join().unwrap();
        assert!(!root.path().join("nes/games/demo.nes").exists());
        assert!(!root.path().join("nes/gamelist.xml").exists());
        assert!(root
            .path()
            .join(format!("nes/.romstore/{}.json", manifest().game_id))
            .exists());
    }

    #[test]
    fn existing_different_rom_is_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("nes/games")).unwrap();
        let target = root.path().join("nes/games/demo.nes");
        fs::write(&target, b"my rom").unwrap();
        let api = Api::new("http://127.0.0.1:1").unwrap();
        let error = install(
            &api,
            root.path(),
            &manifest(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("Conflit local"));
        assert_eq!(fs::read(target).unwrap(), b"my rom");
    }

    #[test]
    fn competing_installation_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("nes/.romstore");
        fs::create_dir_all(&state).unwrap();
        let lock = fs::File::create(state.join("install.lock")).unwrap();
        lock.lock_exclusive().unwrap();
        let api = Api::new("http://127.0.0.1:1").unwrap();
        let error = install(
            &api,
            root.path(),
            &manifest(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("déjà active"));
    }

    #[test]
    fn duplicate_local_xml_entries_are_not_silently_removed() {
        let xml = br#"<gameList><game><path>games/demo.nes</path></game>
            <game><path>./games/demo.nes</path></game></gameList>"#;
        assert!(merge_xml(Some(xml), &manifest()).is_err());
    }
}
