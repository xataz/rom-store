use romstore_client::{
    api::{Api, File, Game, Manifest},
    config::{Config, Frontend},
    install::install_configured,
    uninstall::remove_configured,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::Path,
    sync::atomic::AtomicBool,
    thread,
    time::Duration,
};
use xmltree::Element;

fn config(root: &Path) -> Config {
    Config {
        frontend: Frontend::Esde,
        roms_dir: root.join("ROMs"),
        media_dir: Some(root.join("ES-DE/downloaded_media")),
        gamelists_dir: Some(root.join("ES-DE/gamelists")),
        system_mapping: BTreeMap::from([("gamecube".into(), "gc".into())]),
        ..Config::default()
    }
}

fn manifest() -> Manifest {
    let files: Vec<_> = [
        ("rom", "sub/Batman & Robin [ID].rvz"),
        ("image", "images/mix.PNG"),
        ("marquee", "images/logo.png"),
        ("boxart", "images/box.jpg"),
        ("fanart", "images/background.webp"),
        ("cartridge", "images/disc.png"),
        ("screenshot", "images/screen.jpg"),
        ("titleshot", "images/title.png"),
        ("boxback", "images/back.png"),
        ("video", "videos/demo.mp4"),
        ("manual", "manuals/book.pdf"),
        // Fallbacks must not overwrite preferred media or trigger extra requests.
        ("wheel", "images/wheel.png"),
        ("thumbnail", "images/thumb.png"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (role, destination))| File {
        id: format!("{index:064x}"),
        role: role.into(),
        destination: destination.into(),
        size: role.len() as u64,
        sha256: hex::encode(Sha256::digest(role.as_bytes())),
    })
    .collect();
    let mut metadata: BTreeMap<_, _> = [
        ("name", "Batman & Robin"),
        ("desc", "Une description\nsur deux lignes."),
        ("rating", "0.8"),
        ("releasedate", "20050614T000000"),
        ("developer", "Eurocom"),
        ("publisher", "EA Games"),
        ("genre", "Action"),
        ("players", "1"),
    ]
    .into_iter()
    .map(|(tag, value)| (tag.into(), value.into()))
    .collect();
    for file in files.iter().filter(|file| file.role != "rom") {
        metadata.insert(file.role.clone(), format!("./{}", file.destination));
    }
    Manifest {
        version: 1,
        game_id: "a".repeat(64),
        revision: "b".repeat(64),
        system: "gamecube".into(),
        path: files[0].destination.clone(),
        metadata,
        files,
    }
}

fn game(manifest: &Manifest) -> Game {
    Game {
        installed: true,
        id: manifest.game_id.clone(),
        system: manifest.system.clone(),
        path: manifest.path.clone(),
        name: "Batman".into(),
        metadata: manifest.metadata.clone(),
        installable: true,
        unavailable_reason: None,
        files: manifest.files.clone(),
    }
}

#[test]
fn esde_download_reinstall_and_uninstall_use_mapped_system_and_external_paths() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let manifest = manifest();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = Api::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let mut downloaded: BTreeMap<_, _> = manifest.files[..11]
        .iter()
        .map(|file| (file.id.clone(), file.clone()))
        .collect();
    let server = thread::spawn(move || {
        while !downloaded.is_empty() {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let id = downloaded
                .keys()
                .find(|id| line.contains(id.as_str()))
                .unwrap()
                .clone();
            let expected = downloaded.remove(&id).unwrap();
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                expected.size, expected.role
            )
            .unwrap();
        }
    });
    let xml_dir = config.gamelists_dir.as_ref().unwrap().join("gc");
    fs::create_dir_all(&xml_dir).unwrap();
    let original = br#"<gameList><game><path>./sub/Batman &amp; Robin [ID].rvz</path><favorite>true</favorite><playcount>17</playcount><image>./old.png</image><custom>keep</custom></game><game><path>./other.rvz</path><name>Other</name></game></gameList>"#;
    fs::write(xml_dir.join("gamelist.xml"), original).unwrap();
    let cancel = AtomicBool::new(false);
    assert!(!config.rom_exists("gamecube", &manifest.path));
    install_configured(&api, &config, &manifest, &cancel, |_| {}).unwrap();
    server.join().unwrap();
    // No server remains: a repeated install must reuse verified local files.
    install_configured(&api, &config, &manifest, &cancel, |_| {}).unwrap();
    assert!(config.rom_exists("gamecube", &manifest.path));
    assert!(!config.roms_dir.join("gamecube").exists());
    assert!(!config.roms_dir.join("gc/gamelist.xml").exists());
    assert!(!config.roms_dir.join("gc/images").exists());
    let media = config.media_dir.as_ref().unwrap().join("gc");
    for (category, extension, role) in [
        ("miximages", "png", "image"),
        ("marquees", "png", "marquee"),
        ("covers", "jpg", "boxart"),
        ("fanart", "webp", "fanart"),
        ("physicalmedia", "png", "cartridge"),
        ("screenshots", "jpg", "screenshot"),
        ("titlescreens", "png", "titleshot"),
        ("backcovers", "png", "boxback"),
        ("videos", "mp4", "video"),
        ("manuals", "pdf", "manual"),
    ] {
        assert_eq!(
            fs::read(media.join(format!("{category}/sub/Batman & Robin [ID].{extension}")))
                .unwrap(),
            role.as_bytes()
        );
    }
    let bytes = fs::read(xml_dir.join("gamelist.xml")).unwrap();
    let xml = Element::parse(bytes.as_slice()).unwrap();
    let entry = xml.get_child("game").unwrap();
    for (tag, value) in [
        ("path", "./sub/Batman & Robin [ID].rvz"),
        ("favorite", "true"),
        ("playcount", "17"),
        ("custom", "keep"),
    ] {
        assert_eq!(entry.get_child(tag).unwrap().get_text().unwrap(), value);
    }
    for tag in [
        "image",
        "marquee",
        "fanart",
        "boxart",
        "cartridge",
        "video",
        "manual",
        "thumbnail",
        "wheel",
    ] {
        assert!(entry.get_child(tag).is_none(), "{tag}");
    }
    for (tag, value) in &manifest.metadata {
        if [
            "name",
            "desc",
            "rating",
            "releasedate",
            "developer",
            "publisher",
            "genre",
            "players",
        ]
        .contains(&tag.as_str())
        {
            assert_eq!(
                entry.get_child(tag.as_str()).unwrap().get_text().as_deref(),
                Some(value.as_str())
            );
        }
    }
    remove_configured(&config, &game(&manifest), &cancel, |_| {}).unwrap();
    assert!(!config.rom_exists("gamecube", &manifest.path));
    assert!(media
        .join("miximages/sub/Batman & Robin [ID].png")
        .is_file());
    let bytes = fs::read(xml_dir.join("gamelist.xml")).unwrap();
    let xml = Element::parse(bytes.as_slice()).unwrap();
    assert_eq!(
        xml.get_child("game")
            .unwrap()
            .get_child("name")
            .unwrap()
            .get_text()
            .unwrap(),
        "Other"
    );
}

fn seed_files(config: &Config, manifest: &Manifest) {
    let system = config.roms_dir.join(config.local_system(&manifest.system));
    fs::create_dir_all(system.join("sub")).unwrap();
    fs::write(system.join(&manifest.path), b"rom").unwrap();
    let media = config.media_dir.as_ref().unwrap().join("gc/miximages/sub");
    fs::create_dir_all(&media).unwrap();
    fs::write(media.join("Batman & Robin [ID].png"), b"image").unwrap();
}

#[test]
fn recovery_keeps_server_manifest_and_reapplies_the_local_layout() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let mut manifest = manifest();
    manifest.files.truncate(2);
    manifest
        .metadata
        .retain(|tag, _| tag == "name" || tag == "image");
    seed_files(&config, &manifest);
    let state = config.roms_dir.join("gc/.romstore");
    fs::create_dir(&state).unwrap();
    let journal = state.join(format!("{}.json", manifest.game_id));
    fs::write(&journal, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let recovered: Manifest = serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
    assert_eq!(recovered.system, "gamecube");
    assert_eq!(recovered.files[1].destination, "images/mix.PNG");
    let api = Api::new("http://127.0.0.1:1").unwrap();
    install_configured(&api, &config, &recovered, &AtomicBool::new(false), |_| {}).unwrap();
    assert!(!journal.exists());
    assert!(config
        .gamelists_dir
        .unwrap()
        .join("gc/gamelist.xml")
        .is_file());
}

#[test]
fn conflicts_in_external_media_or_gamelist_prevent_rom_publication() {
    for conflict in ["media", "xml"] {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let mut manifest = manifest();
        manifest.files.truncate(2);
        manifest
            .metadata
            .retain(|tag, _| tag == "name" || tag == "image");
        if conflict == "media" {
            let media = config.media_dir.as_ref().unwrap().join("gc/miximages/sub");
            fs::create_dir_all(&media).unwrap();
            fs::write(media.join("Batman & Robin [ID].png"), b"user image").unwrap();
        } else {
            let xml = config.gamelists_dir.as_ref().unwrap().join("gc");
            fs::create_dir_all(&xml).unwrap();
            fs::write(xml.join("gamelist.xml"), b"<broken").unwrap();
        }
        let api = Api::new("http://127.0.0.1:1").unwrap();
        let error = install_configured(&api, &config, &manifest, &AtomicBool::new(false), |_| {})
            .unwrap_err();
        assert!(
            error.to_string().contains(if conflict == "media" {
                "Conflit local"
            } else {
                "gamelist.xml local invalide"
            }),
            "{error:#}"
        );
        assert!(!config.rom_exists("gamecube", &manifest.path));
    }
}

#[test]
fn system_mapping_also_works_without_esde() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = config(temp.path());
    config.frontend = Frontend::Batocera;
    config.gamelists_dir = None;
    config.media_dir = None;
    let mut manifest = manifest();
    manifest.files.truncate(1);
    manifest.metadata.retain(|tag, _| tag == "name");
    fs::create_dir_all(config.roms_dir.join("gc/sub")).unwrap();
    fs::write(config.roms_dir.join("gc").join(&manifest.path), b"rom").unwrap();
    let api = Api::new("http://127.0.0.1:1").unwrap();
    install_configured(&api, &config, &manifest, &AtomicBool::new(false), |_| {}).unwrap();
    assert!(config.rom_exists("gamecube", &manifest.path));
    assert!(config.roms_dir.join("gc/gamelist.xml").is_file());
    remove_configured(&config, &game(&manifest), &AtomicBool::new(false), |_| {}).unwrap();
    assert!(!config.rom_exists("gamecube", &manifest.path));
}

#[cfg(unix)]
#[test]
fn external_system_and_media_subdirectory_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;
    for target in ["media_system", "xml_system", "media_subdir"] {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let mut manifest = manifest();
        manifest.files.truncate(2);
        manifest
            .metadata
            .retain(|tag, _| tag == "name" || tag == "image");
        let path = match target {
            "media_system" => config.media_dir.as_ref().unwrap().join("gc"),
            "xml_system" => config.gamelists_dir.as_ref().unwrap().join("gc"),
            _ => config.media_dir.as_ref().unwrap().join("gc/miximages/sub"),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(outside.path(), &path).unwrap();
        let api = Api::new("http://127.0.0.1:1").unwrap();
        assert!(
            install_configured(&api, &config, &manifest, &AtomicBool::new(false), |_| {}).is_err()
        );
        assert!(!config.rom_exists("gamecube", &manifest.path));
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
