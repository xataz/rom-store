//! Actual Rust HTTP server -> client binary -> isolated local library.
//! Run with ROMSTORE_CLIENT=/absolute/path/to/romstore-client cargo test ... --test e2e -- --ignored
use romstore_server::{api, catalog::Catalog, config::Config, models::GamePage};
use std::{fs, path::PathBuf, time::Duration};
use tokio::process::Command;

async fn run(command: &mut Command) -> std::process::Output {
    tokio::time::timeout(Duration::from_secs(30), command.kill_on_drop(true).output())
        .await
        .expect("Le client ne doit pas bloquer")
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Requires the SDL client binary: set ROMSTORE_CLIENT and use --ignored"]
async fn client_install_reinstall_conflict_recovery_and_unusual_media_names() {
    let client =
        PathBuf::from(std::env::var_os("ROMSTORE_CLIENT").expect("Définir ROMSTORE_CLIENT"));
    assert!(client.is_file());
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("library/nes");
    fs::create_dir_all(system.join("images")).unwrap();
    fs::write(system.join("demo.nes"), b"synthetic rom").unwrap();
    let stem = "Mario & Luigi - Superstar Saga + Bowser's Minions (Europe) (En,Fr,De,Es,It,Nl)";
    let mut xml =
        String::from("<gameList><game><path>./demo.nes</path><name>Démo ROM Store</name>");
    for role in [
        "image",
        "thumbnail",
        "marquee",
        "fanart",
        "screenshot",
        "boxart",
        "titleshot",
    ] {
        let name = format!("{stem}-{role}.png");
        fs::write(
            system.join("images").join(&name),
            format!("synthetic {role}"),
        )
        .unwrap();
        xml.push_str(&format!(
            "<{role}>./images/{}</{role}>",
            name.replace('&', "&amp;")
        ));
    }
    xml.push_str("</game></gameList>");
    fs::write(system.join("gamelist.xml"), xml).unwrap();
    let catalog = Catalog::new(
        Config::new(
            system.parent().unwrap(),
            temp.path().join("catalog.sqlite3"),
            None,
        )
        .unwrap(),
    )
    .unwrap();
    catalog.reindex().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, api::router(catalog))
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let url = format!("http://{address}");
    let http = reqwest::Client::new();
    let page = http
        .get(format!("{url}/api/v1/games"))
        .send()
        .await
        .unwrap()
        .json::<GamePage>()
        .await
        .unwrap();
    let game = &page.items[0];
    let destination = temp.path().join("installed");
    let local = destination.join("nes");
    fs::create_dir_all(&local).unwrap();
    let config = temp.path().join("client.yaml");
    let refresh_script = temp.path().join("refresh games.sh");
    let refresh_marker = temp.path().join("refresh-count");
    fs::write(&refresh_script, "#!/bin/sh\nset -eu\ntest -s \"$1/nes/gamelist.xml\"\ntest -s \"$1/nes/demo.nes\"\nprintf 'refreshed\\n' >> \"$2\"\n").unwrap();
    let refresh_command = serde_json::to_string(&[
        "/bin/sh".to_owned(),
        refresh_script.display().to_string(),
        destination.display().to_string(),
        refresh_marker.display().to_string(),
    ])
    .unwrap();
    fs::write(
        &config,
        format!("server_url: {url}\nroms_dir: {}\nfrontend: custom\nauto_refresh: true\nrefresh_command: {refresh_command}\n", destination.display()),
    )
    .unwrap();
    fs::write(local.join("gamelist.xml"), "<gameList><game id=\"local\"><path>./demo.nes</path><favorite>true</favorite><playcount>17</playcount><custom>local</custom></game></gameList>").unwrap();
    for _ in 0..2 {
        let result = run(Command::new(&client)
            .arg("--config")
            .arg(&config)
            .args(["--install", &game.id]))
        .await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        for file in &game.files {
            use sha2::{Digest, Sha256};
            assert_eq!(
                hex::encode(Sha256::digest(
                    fs::read(local.join(&file.destination)).unwrap()
                )),
                file.sha256
            );
        }
        let xml = fs::read_to_string(local.join("gamelist.xml")).unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        assert_eq!(
            doc.descendants().filter(|n| n.has_tag_name("game")).count(),
            1
        );
        assert!(doc
            .descendants()
            .any(|n| n.has_tag_name("favorite") && n.text() == Some("true")));
        assert!(xml.contains("Mario &amp; Luigi"));
        assert!(!xml.contains("&amp;amp;"));
    }
    assert_eq!(
        fs::read_to_string(&refresh_marker).unwrap().lines().count(),
        2
    );
    for mode in ["--help", "--smoke-test"] {
        let result = run(Command::new(&client)
            .arg("--config")
            .arg(&config)
            .arg(mode)
            .env("SDL_VIDEODRIVER", "dummy"))
        .await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            fs::read_to_string(&refresh_marker).unwrap().lines().count(),
            2,
            "No frontend refresh for {mode}"
        );
    }
    // A changed local ROM remains untouched; the server does not dictate overwrites.
    fs::write(local.join("demo.nes"), b"my local ROM").unwrap();
    let result = run(Command::new(&client)
        .arg("--config")
        .arg(&config)
        .args(["--install", &game.id]))
    .await;
    assert!(!result.status.success());
    assert_eq!(fs::read(local.join("demo.nes")).unwrap(), b"my local ROM");
    assert_eq!(
        fs::read_to_string(&refresh_marker).unwrap().lines().count(),
        2,
        "Failed installations do not trigger the success hook"
    );
    // Recover an interrupted installation from a manifest on a new destination.
    let manifest = http
        .get(format!("{url}/api/v1/games/{}/manifest", game.id))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let recovery_root = temp.path().join("recovered");
    let state = recovery_root.join("nes/.romstore");
    fs::create_dir_all(&state).unwrap();
    fs::write(recovery_root.join("nes/demo.nes"), b"synthetic rom").unwrap();
    let journal = state.join(format!("{}.json", game.id));
    fs::write(&journal, manifest).unwrap();
    fs::write(
        &config,
        format!("server_url: {url}\nroms_dir: {}\n", recovery_root.display()),
    )
    .unwrap();
    let result = run(Command::new(&client)
        .arg("--config")
        .arg(&config)
        .arg("--recover")
        .arg(&journal))
    .await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!journal.exists());
    for (language, expected) in [
        ("fr", "Sans option"),
        ("en", "No option"),
        ("es", "Sin opciones"),
        ("de", "Ohne Option"),
    ] {
        fs::write(&config, format!("language: {language}\n")).unwrap();
        let result = run(Command::new(&client)
            .arg("--config")
            .arg(&config)
            .arg("--help"))
        .await;
        assert!(result.status.success());
        assert!(String::from_utf8(result.stdout).unwrap().contains(expected));
    }
    // ES-DE uses the same server manifest but routes files and XML locally.
    let esde = temp.path().join("esde");
    let esde_roms = esde.join("ROMs");
    let esde_media = esde.join("downloaded_media");
    let esde_xml = esde.join("gamelists");
    fs::write(&config, format!("server_url: {url}\nfrontend: es-de\nroms_dir: {}\nmedia_dir: {}\ngamelists_dir: {}\nsystem_mapping:\n  nes: famicom\n", esde_roms.display(), esde_media.display(), esde_xml.display())).unwrap();
    for mode in ["--install", "--recover"] {
        let journal = esde_roms.join(format!("famicom/.romstore/{}.json", game.id));
        let argument = if mode == "--recover" {
            let manifest = http
                .get(format!("{url}/api/v1/games/{}/manifest", game.id))
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            fs::write(&journal, manifest).unwrap();
            journal.to_str().unwrap()
        } else {
            &game.id
        };
        let result = run(Command::new(&client)
            .arg("--config")
            .arg(&config)
            .args([mode, argument]))
        .await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(esde_roms.join("famicom/demo.nes").is_file());
        assert!(!esde_roms.join("nes").exists());
        assert!(!esde_roms.join("famicom/gamelist.xml").exists());
        for (category, role) in [
            ("miximages", "image"),
            ("covers", "boxart"),
            ("marquees", "marquee"),
            ("fanart", "fanart"),
            ("screenshots", "screenshot"),
            ("titlescreens", "titleshot"),
        ] {
            assert_eq!(
                fs::read(esde_media.join(format!("famicom/{category}/demo.png"))).unwrap(),
                format!("synthetic {role}").as_bytes()
            );
        }
        let xml = fs::read_to_string(esde_xml.join("famicom/gamelist.xml")).unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        assert!(doc
            .descendants()
            .any(|node| node.has_tag_name("path") && node.text() == Some("./demo.nes")));
        assert!(!doc.descendants().any(|node| node.has_tag_name("image")
            || node.has_tag_name("thumbnail")
            || node.has_tag_name("marquee")));
        assert!(!journal.exists());
    }
    let _ = shutdown.send(());
    server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Requires the SDL client binary: set ROMSTORE_CLIENT and use --ignored"]
async fn client_multidisc_download_reinstall_and_recovery_preserve_source_layout() {
    let client =
        PathBuf::from(std::env::var_os("ROMSTORE_CLIENT").expect("Définir ROMSTORE_CLIENT"));
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("library/psx");
    fs::create_dir_all(system.join("games/Jeu & test/discs")).unwrap();
    fs::create_dir_all(system.join("images")).unwrap();
    let path = "games/Jeu & test.m3u";
    let playlist = "\u{feff}#EXTM3U\r\nJeu & test/discs/Disc 1.chd\r\nJeu & test/Disc 2.rvz\r\n../Disc 3.zip\r\n";
    let contents: Vec<(&str, &[u8])> = vec![
        (path, playlist.as_bytes()),
        ("games/Jeu & test/discs/Disc 1.chd", b"synthetic chd"),
        ("games/Jeu & test/Disc 2.rvz", b"synthetic rvz"),
        ("Disc 3.zip", b"synthetic archive kept intact"),
        ("images/cover.png", b"synthetic cover"),
    ];
    for (destination, bytes) in &contents {
        fs::write(system.join(destination), bytes).unwrap();
    }
    fs::write(system.join("gamelist.xml"), "<gameList><game><path>./games/Jeu &amp; test.m3u</path><name>Jeu multidisque</name><thumbnail>./images/cover.png</thumbnail></game></gameList>").unwrap();
    let catalog = Catalog::new(
        Config::new(
            system.parent().unwrap(),
            temp.path().join("catalog.sqlite3"),
            None,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(catalog.reindex().unwrap().diagnostics.is_empty());
    let game = catalog.page(None, "", 0, 50).unwrap().items.remove(0);
    assert!(game.installable);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, api::router(catalog))
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let config = temp.path().join("client.yaml");
    let destination = temp.path().join("installed");
    fs::write(
        &config,
        format!("server_url: {url}\nroms_dir: {}\n", destination.display()),
    )
    .unwrap();
    for _ in 0..2 {
        let result = run(Command::new(&client)
            .arg("--config")
            .arg(&config)
            .args(["--install", &game.id]))
        .await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        for (relative, bytes) in &contents {
            assert_eq!(
                fs::read(destination.join("psx").join(relative)).unwrap(),
                *bytes
            );
            assert_eq!(fs::read(system.join(relative)).unwrap(), *bytes);
        }
        let xml = fs::read_to_string(destination.join("psx/gamelist.xml")).unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        assert_eq!(
            doc.descendants()
                .filter(|node| node.has_tag_name("game"))
                .count(),
            1
        );
        assert!(
            doc.descendants()
                .any(|node| node.has_tag_name("path")
                    && node.text() == Some("./games/Jeu & test.m3u"))
        );
        assert!(doc.descendants().any(
            |node| node.has_tag_name("thumbnail") && node.text() == Some("./images/cover.png")
        ));
    }
    // Recover with only one disc already present, without flattening the paths.
    let recovered = temp.path().join("recovered");
    let state = recovered.join("psx/.romstore");
    fs::create_dir_all(&state).unwrap();
    fs::write(recovered.join("psx/Disc 3.zip"), contents[3].1).unwrap();
    let manifest = reqwest::get(format!("{url}/api/v1/games/{}/manifest", game.id))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let journal = state.join(format!("{}.json", game.id));
    fs::write(&journal, &manifest).unwrap();
    fs::write(
        &config,
        format!("server_url: {url}\nroms_dir: {}\n", recovered.display()),
    )
    .unwrap();
    let result = run(Command::new(&client)
        .arg("--config")
        .arg(&config)
        .arg("--recover")
        .arg(&journal))
    .await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!journal.exists());
    for (relative, bytes) in &contents {
        assert_eq!(
            fs::read(recovered.join("psx").join(relative)).unwrap(),
            *bytes
        );
    }
    fs::write(recovered.join("psx/Disc 3.zip"), b"local disc").unwrap();
    let result = run(Command::new(&client)
        .arg("--config")
        .arg(&config)
        .args(["--install", &game.id]))
    .await;
    assert!(!result.status.success());
    assert_eq!(
        fs::read(recovered.join("psx/Disc 3.zip")).unwrap(),
        b"local disc"
    );
    let _ = shutdown.send(());
    server.await.unwrap();
}
