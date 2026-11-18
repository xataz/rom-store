use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use romstore_server::{
    api,
    catalog::{Catalog, StoredFile},
    config::Config,
    identity::{casefold, identifier, python_json},
    models::Game,
    paths::relative_path,
};
use serde_json::{json, Value};
use std::{fs, path::Path, time::Duration};
use tower::ServiceExt;

const XML: &str = "<gameList><game><path>./demo.nes</path><name>Démo</name><image>./images/demo.png</image><favorite>true</favorite><custom>kept</custom></game></gameList>";

struct Fixture {
    temp: tempfile::TempDir,
    catalog: Catalog,
    app: Router,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        fs::create_dir_all(root.join("nes/images")).unwrap();
        fs::write(root.join("nes/demo.nes"), b"synthetic rom").unwrap();
        fs::write(root.join("nes/images/demo.png"), b"synthetic image").unwrap();
        fs::write(root.join("nes/gamelist.xml"), XML).unwrap();
        let config = Config::new(root, temp.path().join("catalog.sqlite3"), None).unwrap();
        let catalog = Catalog::new(config).unwrap();
        assert!(catalog.reindex().unwrap().diagnostics.is_empty());
        let app = api::router(catalog.clone());
        Self { temp, catalog, app }
    }
    fn system(&self) -> std::path::PathBuf {
        self.catalog.config.library.join("nes")
    }
    fn xml(&self, value: &str) {
        fs::write(self.system().join("gamelist.xml"), value).unwrap();
    }
    fn game(&self) -> Game {
        self.catalog.page(None, "", 0, 50).unwrap().items.remove(0)
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        etag: Option<&str>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(etag) = etag {
            request = request.header("If-Match", etag);
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        (
            status,
            headers,
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
    }
    async fn json(&self, method: &str, path: &str) -> (StatusCode, Value) {
        let (status, _, body) = self.request(method, path, None).await;
        (status, serde_json::from_slice(&body).unwrap())
    }
    async fn reload(&self) -> Value {
        let (status, body) = self.json("POST", "/api/v1/catalog/reload").await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["status"], "running");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (_, state) = self.json("GET", "/api/v1/catalog/reload").await;
                if state["status"] != "running" {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn catalog_manifest_and_streaming_preserve_api_v1() {
    let f = Fixture::new();
    assert_eq!(
        f.json("GET", "/api/v1/health").await.1["manifest_version"],
        1
    );
    let (_, systems) = f.json("GET", "/api/v1/systems").await;
    assert_eq!(
        systems,
        json!([{"id":"nes","name":"nes","game_count":1,"image":null}])
    );
    let game = f.game();
    assert!(!game.metadata.contains_key("favorite"));
    assert_eq!(game.metadata["custom"], "kept");
    let (_, manifest) = f
        .json("GET", &format!("/api/v1/games/{}/manifest", game.id))
        .await;
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["files"].as_array().unwrap().len(), 2);
    for file in game.files {
        let uri = format!("/api/v1/files/{}", file.id);
        let (status, headers, bytes) = f.request("GET", &uri, None).await;
        assert_eq!(status, StatusCode::OK);
        use sha2::{Digest, Sha256};
        assert_eq!(hex::encode(Sha256::digest(&bytes)), file.sha256);
        assert_eq!(
            headers["content-length"].to_str().unwrap(),
            file.size.to_string()
        );
        assert_eq!(
            headers["etag"].to_str().unwrap(),
            format!("\"{}\"", file.sha256)
        );
        assert_eq!(
            f.request("GET", &uri, Some("\"wrong\"")).await.0,
            StatusCode::PRECONDITION_FAILED
        );
    }
}

#[tokio::test]
async fn advanced_metadata_filters_are_combined_before_counting_and_pagination() {
    let f = Fixture::new();
    f.xml("<gameList>
        <game><path>demo.nes</path><name>Alpha</name><releasedate>19911224T000000</releasedate><players>1-4</players><genre>Action</genre><rating>0.8</rating><custom>Démo</custom></game>
        <game><path>second.nes</path><name>Beta</name><releasedate>19910101T000000</releasedate><players>2</players><genre>RPG</genre><rating>0.9</rating><custom>Démo</custom></game>
        <game><path>third.nes</path><name>Gamma</name><releasedate>20010101T000000</releasedate><players>1</players><genre>Action</genre></game>
    </gameList>");
    fs::write(f.system().join("second.nes"), b"second").unwrap();
    fs::write(f.system().join("third.nes"), b"third").unwrap();
    f.catalog.reindex().unwrap();
    let (status, fields) = f.json("GET", "/api/v1/games/fields").await;
    assert_eq!(status, StatusCode::OK);
    assert!(fields.as_array().unwrap().contains(&json!("custom")));
    assert!(fields.as_array().unwrap().contains(&json!("year")));
    let path = |filters: Value, offset: usize| {
        let mut url = reqwest::Url::parse("http://localhost/api/v1/games").unwrap();
        url.query_pairs_mut()
            .append_pair("system", "nes")
            .append_pair("filters", &filters.to_string())
            .append_pair("limit", "1")
            .append_pair("offset", &offset.to_string());
        format!("{}?{}", url.path(), url.query().unwrap())
    };
    let filters = json!({"year":"1991", "players":"2", "custom":"=démo", "rating":">=0.8"});
    for (offset, name) in [(0, "Alpha"), (1, "Beta")] {
        let (status, page) = f.json("GET", &path(filters.clone(), offset)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["total"], 2);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["name"], name);
    }
    for (filters, total) in [
        (json!({"year":">=2000", "players":"2"}), 0),
        (json!({"name":"=beta", "genre":"rpg"}), 1),
        (json!({"unknown":"value"}), 0),
        (json!({"genre":"%"}), 0),
        (json!({"custom":"' OR 1=1 --"}), 0),
    ] {
        let (status, page) = f.json("GET", &path(filters, 0)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["total"], total);
    }
    for filters in [
        json!({"bad'key":"x"}),
        json!({"genre":"x".repeat(257)}),
        json!(["not a map"]),
    ] {
        assert_eq!(
            f.json("GET", &path(filters, 0)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        f.json("GET", "/api/v1/games?filters=not-json").await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[test]
fn python_identifiers_revisions_and_cache_remain_compatible() {
    let f = Fixture::new();
    f.xml("<gameList><game><path>./demo.nes</path><name>Démo 🎮 &amp; Mario</name><desc>Première ligne\nÉté ß \"test\"</desc><image>./images/demo.png</image><thumbnail>./images/demo.png</thumbnail><favorite>true</favorite><custom>kept</custom></game></gameList>");
    let report = f.catalog.reindex().unwrap();
    assert_eq!(report.reused, 2);
    assert_eq!(report.calculated, 0);
    let expected: Value = serde_json::from_str(include_str!("data/python-v1-game.json")).unwrap();
    assert_eq!(serde_json::to_value(f.game()).unwrap(), expected);
    let db = f.catalog.connect().unwrap();
    db.pragma_update(None, "user_version", 0).unwrap();
    // A previous Python snapshot has nullable/missing images and tuple signatures stored as JSON strings.
    let old = Catalog::new(f.catalog.config.clone()).unwrap();
    assert_eq!(old.reindex().unwrap().reused, 2);
    assert_eq!(
        serde_json::to_value(old.page(None, "", 0, 50).unwrap().items.remove(0)).unwrap(),
        expected
    );
}

#[tokio::test]
async fn validation_errors_and_unknown_routes_are_json() {
    let f = Fixture::new();
    for uri in [
        "/api/v1/games?limit=999",
        "/api/v1/games?offset=-1",
        "/api/v1/games?limit=0",
        "/api/v1/games?limit=bad",
        "/api/v1/games/%FF",
        "/api/v1/files/%FF",
        "/api/v1/games?offset=18446744073709551615",
    ] {
        let (status, value) = f.json("GET", uri).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
        assert!(value["error"]["message"].is_string());
    }
    let long = format!("/api/v1/games?search={}", "a".repeat(257));
    assert_eq!(
        f.json("GET", &long).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for uri in ["/api/v1/games/unknown", "/api/v1/files/unknown", "/unknown"] {
        assert_eq!(f.json("GET", uri).await.0, StatusCode::NOT_FOUND);
    }
    assert_eq!(
        f.json("DELETE", "/api/v1/games").await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        f.json("GET", "/api/v1/games?system=unknown").await.1["total"],
        0
    );
}

#[tokio::test]
async fn changed_rom_and_replaced_system_are_never_streamed() {
    let f = Fixture::new();
    let file = f.game().files.remove(0);
    fs::write(f.system().join("demo.nes"), b"changed ROM").unwrap();
    let (status, _, body) = f
        .request("GET", &format!("/api/v1/files/{}", file.id), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(!String::from_utf8(body)
        .unwrap()
        .contains(f.temp.path().to_str().unwrap()));
    use std::os::unix::fs::symlink;
    fs::rename(f.system(), f.temp.path().join("old-nes")).unwrap();
    symlink(f.temp.path().join("old-nes"), f.system()).unwrap();
    assert_eq!(
        f.request("GET", &format!("/api/v1/files/{}", file.id), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
}

#[test]
fn every_media_role_and_escaped_batocera_path_roundtrips() {
    for role in romstore_server::catalog::MEDIA_TAGS {
        let f = Fixture::new();
        let name = "Mario & Luigi - Bowser's Minions (Europe) (En,Fr,De,Es,It,Nl).png";
        fs::write(f.system().join("images").join(name), b"synthetic media").unwrap();
        let xml = XML.replace(
            "</game>",
            &format!(
                "<{role}>./images/{}</{role}></game>",
                name.replace('&', "&amp;")
            ),
        );
        f.xml(&xml);
        f.catalog.reindex().unwrap();
        let game = f.game();
        // Existing image is retained as first occurrence; all other roles use the long path.
        if *role != "image" {
            assert_eq!(game.metadata[*role], format!("./images/{name}"));
            assert!(game
                .files
                .iter()
                .any(|file| file.role == *role && file.destination.ends_with(name)));
        }
    }
}

#[test]
fn traversal_missing_media_and_xml_attacks_are_isolated() {
    for path in [
        "../outside.nes",
        "/etc/passwd",
        "images/../../outside.nes",
        "file:bad",
        ".romstore/state",
        "gamelist.xml",
        ".",
    ] {
        let f = Fixture::new();
        f.xml(&format!(
            "<gameList><game><path>{path}</path></game></gameList>"
        ));
        assert!(!f.catalog.reindex().unwrap().diagnostics.is_empty());
        assert_eq!(f.catalog.page(None, "", 0, 50).unwrap().total, 0);
    }
    for xml in ["<broken", "<wrong/>", "<!DOCTYPE gameList [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><gameList><game><path>&x;</path></game></gameList>"] {
        let f = Fixture::new(); f.xml(xml);
        assert!(!f.catalog.reindex().unwrap().diagnostics.is_empty()); assert_eq!(f.catalog.page(None, "", 0, 50).unwrap().total, 0);
    }
    let f = Fixture::new();
    fs::remove_file(f.system().join("images/demo.png")).unwrap();
    assert!(!f.catalog.reindex().unwrap().diagnostics.is_empty());
    assert_eq!(f.game().files.len(), 1);
    assert!(!f.game().metadata.contains_key("image"));
}

#[test]
fn symlinks_are_allowed_only_for_files_still_within_their_platform() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::rename(f.system().join("demo.nes"), f.system().join("real.nes")).unwrap();
    symlink(f.system().join("real.nes"), f.system().join("demo.nes")).unwrap();
    assert!(f.catalog.reindex().unwrap().diagnostics.is_empty());
    fs::remove_file(f.system().join("demo.nes")).unwrap();
    let outside = f.temp.path().join("outside.nes");
    fs::write(&outside, b"private").unwrap();
    symlink(outside, f.system().join("demo.nes")).unwrap();
    assert!(!f.catalog.reindex().unwrap().diagnostics.is_empty());
    assert_eq!(f.catalog.page(None, "", 0, 50).unwrap().total, 0);
}

#[tokio::test]
async fn dependent_formats_stay_visible_but_cannot_be_installed() {
    for extension in ["cue", "gdi", "ccd", "mds"] {
        let f = Fixture::new();
        fs::write(
            f.system().join(format!("demo.{extension}")),
            b"synthetic dependent",
        )
        .unwrap();
        let entry = format!("<game><path>./demo.{extension}</path></game>");
        f.xml(&format!("<gameList>{entry}{entry}</gameList>"));
        assert_eq!(f.catalog.reindex().unwrap().diagnostics.len(), 1);
        let game = f.game();
        assert!(!game.installable);
        assert_eq!(
            f.json("GET", &format!("/api/v1/games/{}/manifest", game.id))
                .await
                .0,
            StatusCode::CONFLICT
        );
    }
}

#[test]
fn unicode_casefold_pagination_and_literal_search_are_preserved() {
    let f = Fixture::new();
    let mut xml = String::from("<gameList>");
    for (index, name) in ["Zulu", "alpha", "Éclair", "ALPHA", "Straße"]
        .iter()
        .enumerate()
    {
        fs::write(f.system().join(format!("game{index}.nes")), b"rom").unwrap();
        xml.push_str(&format!(
            "<game><path>game{index}.nes</path><name>{name}</name></game>"
        ));
    }
    xml.push_str("</gameList>");
    f.xml(&xml);
    f.catalog.reindex().unwrap();
    let first = f.catalog.page(None, "", 0, 2).unwrap();
    let second = f.catalog.page(None, "", 2, 50).unwrap();
    assert_eq!(first.total, 5);
    assert!(first
        .items
        .iter()
        .all(|game| casefold(&game.name) == "alpha"));
    assert_eq!(
        second
            .items
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>(),
        ["Straße", "Zulu", "Éclair"]
    );
    assert_eq!(f.catalog.page(None, "ÉCLAIR", 0, 50).unwrap().total, 1);
    assert_eq!(f.catalog.page(None, "STRASSE", 0, 50).unwrap().total, 1);
    assert_eq!(f.catalog.page(None, "%_", 0, 50).unwrap().total, 0);
    assert!(f.catalog.page(None, "", 100, 50).unwrap().items.is_empty());
}

#[tokio::test]
async fn multidisc_manifests_preserve_playlist_bytes_paths_order_and_revisions() {
    for extension in ["m3u", "M3U", "m3u8"] {
        let f = Fixture::new();
        fs::create_dir_all(f.system().join("games/discs")).unwrap();
        let path = format!("games/Démo & test.{extension}");
        let content = "\u{feff}#EXTM3U\r\n\r\n#EXTINF:0,Disc 1\r\ndiscs/Disc 1.chd\r\n../Disc 2.rvz\r\n./discs/Disc 1.chd\r\n";
        fs::write(f.system().join(&path), content).unwrap();
        fs::write(f.system().join("games/discs/Disc 1.chd"), b"disc one").unwrap();
        fs::write(f.system().join("Disc 2.rvz"), b"disc two").unwrap();
        f.xml(&format!(
            "<gameList><game><path>./{}</path><image>./images/demo.png</image></game></gameList>",
            path.replace('&', "&amp;")
        ));
        assert!(f.catalog.reindex().unwrap().diagnostics.is_empty());
        let game = f.game();
        assert!(game.installable);
        assert_eq!(game.path, path);
        assert_eq!(
            game.files
                .iter()
                .map(|file| (file.role.as_str(), file.destination.as_str()))
                .collect::<Vec<_>>(),
            [
                ("rom", path.as_str()),
                ("disk", "games/discs/Disc 1.chd"),
                ("disk", "Disc 2.rvz"),
                ("image", "images/demo.png")
            ]
        );
        assert_eq!(f.catalog.systems().unwrap()[0].game_count, 1);
        let (status, manifest) = f
            .json("GET", &format!("/api/v1/games/{}/manifest", game.id))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            manifest["files"],
            serde_json::to_value(&game.files).unwrap()
        );
        assert_eq!(
            f.request("GET", &format!("/api/v1/files/{}", game.files[0].id), None)
                .await
                .2,
            content.as_bytes()
        );
        for file in &game.files {
            assert_eq!(
                f.request("GET", &format!("/api/v1/files/{}", file.id), None)
                    .await
                    .0,
                StatusCode::OK
            );
        }
        assert_eq!(f.catalog.reindex().unwrap().calculated, 0);
        fs::write(f.system().join("Disc 2.rvz"), b"disc two changed").unwrap();
        f.catalog.reindex().unwrap();
        assert_eq!(f.game().id, game.id);
        assert_ne!(f.game().revision, game.revision);
    }
}

#[tokio::test]
async fn invalid_playlists_are_visible_but_never_partially_installable() {
    for content in [
        "",
        "# comments only\n",
        "missing.chd\n",
        "../outside.chd\n",
        "/etc/passwd\n",
        "https://example.org/disc.chd\n",
        "C:\\disc.chd\n",
        ".romstore/secret.chd\n",
        "gamelist.xml\n",
        "demo.m3u\n",
        "disc.cue\n",
    ] {
        let f = Fixture::new();
        fs::write(f.system().join("demo.m3u"), content).unwrap();
        f.xml("<gameList><game><path>./demo.m3u</path></game></gameList>");
        assert_eq!(
            f.catalog.reindex().unwrap().diagnostics.len(),
            1,
            "{content}"
        );
        let game = f.game();
        assert!(!game.installable, "{content}");
        assert!(game
            .unavailable_reason
            .unwrap()
            .starts_with("M3U non installable"));
        assert_eq!(game.files.len(), 1);
        assert_eq!(
            f.json("GET", &format!("/api/v1/games/{}/manifest", game.id))
                .await
                .0,
            StatusCode::CONFLICT
        );
    }
    let f = Fixture::new();
    f.xml("<gameList><game><path>demo.m3u</path></game></gameList>");
    for content in [
        vec![0xff],
        vec![b'x'; 1024 * 1024 + 1],
        "demo.nes\n".repeat(241).into_bytes(),
    ] {
        fs::write(f.system().join("demo.m3u"), content).unwrap();
        assert!(!f.catalog.reindex().unwrap().diagnostics.is_empty());
        assert!(!f.game().installable);
    }
    use std::os::unix::fs::symlink;
    fs::write(f.temp.path().join("outside.chd"), b"private").unwrap();
    symlink(
        f.temp.path().join("outside.chd"),
        f.system().join("disc.chd"),
    )
    .unwrap();
    fs::write(f.system().join("demo.m3u"), "disc.chd\n").unwrap();
    f.catalog.reindex().unwrap();
    assert!(!f.game().installable);
}

#[tokio::test]
async fn icons_are_dynamic_and_namespaced_and_old_library_icons_are_ignored() {
    for extension in ["png", "jpg", "jpeg", "webp", "pnm", "ppm"] {
        let f = Fixture::new();
        fs::write(f.system().join("system.png"), b"old ignored icon").unwrap();
        assert!(f.catalog.systems().unwrap()[0].image.is_none());
        fs::create_dir(&f.catalog.config.system_icon).unwrap();
        let path = f
            .catalog
            .config
            .system_icon
            .join(format!("nes.{extension}"));
        fs::write(&path, b"icon").unwrap();
        let image = f.catalog.systems().unwrap().remove(0).image.unwrap();
        let uri = format!("/api/v1/files/{}", image.id);
        assert_eq!(f.request("GET", &uri, None).await.2, b"icon");
        f.catalog.reindex().unwrap();
        assert_eq!(f.request("GET", &uri, None).await.0, StatusCode::OK);
        fs::write(&path, b"changed icon").unwrap();
        assert_eq!(f.request("GET", &uri, None).await.0, StatusCode::CONFLICT);
        assert_ne!(
            f.catalog.systems().unwrap()[0].image.as_ref().unwrap().id,
            image.id
        );
        assert_eq!(f.request("GET", &uri, None).await.0, StatusCode::NOT_FOUND);
        fs::remove_file(path).unwrap();
        assert!(f.catalog.systems().unwrap()[0].image.is_none());
    }
}

#[tokio::test]
async fn icon_priority_limits_and_symlink_escape_are_enforced() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let icons = &f.catalog.config.system_icon;
    fs::create_dir(icons).unwrap();
    fs::write(icons.join("nes.jpg"), b"jpeg").unwrap();
    fs::write(icons.join("nes.png"), b"png").unwrap();
    assert_eq!(
        f.catalog.systems().unwrap()[0]
            .image
            .as_ref()
            .unwrap()
            .destination,
        "nes.png"
    );
    fs::remove_file(icons.join("nes.png")).unwrap();
    symlink(f.system().join("demo.nes"), icons.join("nes.png")).unwrap();
    assert_eq!(
        f.catalog.systems().unwrap()[0]
            .image
            .as_ref()
            .unwrap()
            .destination,
        "nes.jpg"
    );
    fs::remove_file(icons.join("nes.jpg")).unwrap();
    let big = fs::File::create(icons.join("nes.webp")).unwrap();
    big.set_len(8 * 1024 * 1024 + 1).unwrap();
    assert!(f.catalog.systems().unwrap()[0].image.is_none());
    fs::write(icons.join("nes.ppm"), b"fallback").unwrap();
    let image = f.catalog.systems().unwrap()[0].image.clone().unwrap();
    fs::rename(icons, f.temp.path().join("old-icons")).unwrap();
    symlink(f.temp.path().join("old-icons"), icons).unwrap();
    assert_eq!(
        f.request("GET", &format!("/api/v1/files/{}", image.id), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert!(f.catalog.systems().unwrap()[0].image.is_none());
}

#[tokio::test]
async fn legacy_icon_records_cannot_escape_into_the_rom_library() {
    let f = Fixture::new();
    let game = f.game();
    let mut legacy: StoredFile = f.catalog.get("files", &game.files[0].id).unwrap().unwrap();
    legacy.id = "a".repeat(64);
    let db = f.catalog.connect().unwrap();
    db.execute(
        "INSERT INTO system_images VALUES (?1, ?2)",
        [&legacy.id, &serde_json::to_string(&legacy).unwrap()],
    )
    .unwrap();
    assert_eq!(
        f.request("GET", &format!("/api/v1/files/{}", legacy.id), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    legacy.id = game.files[0].id.clone();
    db.execute(
        "INSERT INTO system_images VALUES (?1, ?2)",
        [&legacy.id, &serde_json::to_string(&legacy).unwrap()],
    )
    .unwrap();
    assert_eq!(
        f.request("GET", &format!("/api/v1/files/{}", legacy.id), None)
            .await
            .2,
        b"synthetic rom"
    );
}

#[tokio::test]
async fn hot_reload_refreshes_added_removed_and_renamed_games() {
    let f = Fixture::new();
    let old = f.game();
    fs::write(f.system().join("new.nes"), b"new game").unwrap();
    f.xml("<gameList><game><path>new.nes</path><name>New</name></game></gameList>");
    assert_eq!(f.game().id, old.id);
    assert_eq!(f.reload().await["status"], "completed");
    assert_eq!(f.game().name, "New");
    assert_eq!(
        f.json("GET", &format!("/api/v1/games/{}", old.id)).await.0,
        StatusCode::NOT_FOUND
    );
    fs::remove_file(f.system().join("new.nes")).unwrap();
    let state = f.reload().await;
    assert_eq!(state["games"], 0);
    assert_eq!(state["diagnostics"], 1);
}

#[tokio::test]
async fn failed_reload_keeps_published_catalogue_and_can_be_retried() {
    let f = Fixture::new();
    let old = f.game();
    let moved = f.temp.path().join("moved-library");
    fs::rename(&f.catalog.config.library, &moved).unwrap();
    let state = f.reload().await;
    assert_eq!(state["status"], "failed");
    assert!(!state["error"]
        .as_str()
        .unwrap()
        .contains(f.temp.path().to_str().unwrap()));
    assert_eq!(f.game(), old);
    fs::rename(moved, &f.catalog.config.library).unwrap();
    assert_eq!(f.reload().await["status"], "completed");
}

#[tokio::test]
async fn docs_and_openapi_remain_available() {
    let f = Fixture::new();
    let (_, schema) = f.json("GET", "/openapi.json").await;
    assert_eq!(
        schema["components"]["schemas"]["Manifest"]["properties"]["version"]["const"],
        1
    );
    assert!(schema["paths"]["/api/v1/catalog/reload"]["post"].is_object());
    assert!(schema["paths"]["/api/v1/catalog/summary"]["get"].is_object());
    assert_eq!(f.request("GET", "/docs", None).await.0, StatusCode::OK);
    let (status, headers, body) = f.request("GET", "/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .contains("text/html"));
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains("/api/v1/catalog/reload"));
    assert!(html.contains("/api/v1/catalog/summary"));
    assert!(!html.contains("https://"));
}

#[tokio::test]
async fn management_counts_include_uninstallable_games_and_follow_publication() {
    let f = Fixture::new();
    fs::write(
        f.system().join("unsupported.cue"),
        b"FILE missing.bin BINARY",
    )
    .unwrap();
    f.xml("<gameList><game><path>demo.nes</path></game><game><path>unsupported.cue</path></game></gameList>");
    let (_, summary) = f.json("GET", "/api/v1/catalog/summary").await;
    assert_eq!(summary[0]["games"], 1);
    f.reload().await;
    let (status, summary) = f.json("GET", "/api/v1/catalog/summary").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        summary,
        json!([{"id":"nes", "name":"nes", "games":2, "installable":1}])
    );
    f.xml("<gameList/>");
    f.reload().await;
    let (_, summary) = f.json("GET", "/api/v1/catalog/summary").await;
    assert_eq!(summary[0]["games"], 0);
    assert_eq!(summary[0]["installable"], 0);
}

#[test]
fn formatted_xml_with_215_entries_is_not_doubled_by_parsing_or_a_warm_reindex() {
    let f = Fixture::new();
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gameList>\n<!-- <game>not an entry</game> -->\n");
    for index in 0..215 {
        let name = format!("rom{index}.z64");
        fs::write(f.system().join(&name), b"rom").unwrap();
        xml.push_str(&format!("  <game>\n    <path>./{name}</path>\n    <desc><![CDATA[<game>not an entry</game>]]></desc>\n  </game>\n"));
    }
    xml.push_str("</gameList>\n");
    f.xml(&xml);
    let cold = f.catalog.reindex().unwrap();
    assert_eq!(cold.games, 215);
    assert_eq!(cold.calculated, 215);
    assert!(cold.diagnostics.is_empty());
    let warm = f.catalog.reindex().unwrap();
    assert_eq!(warm.games, 215);
    assert_eq!(warm.reused, 215);
    assert_eq!(warm.calculated, 0);
    assert!(
        warm.diagnostics.is_empty(),
        "Cached ROMs must never become ignored entries"
    );
    assert_eq!(f.catalog.page(None, "", 0, 1).unwrap().total, 215);
}

#[tokio::test]
async fn issues_show_skipped_paths_line_numbers_and_unavailable_dreamcast_games() {
    let f = Fixture::new();
    f.xml("<gameList>\n<game><path>demo.nes</path></game>\n<game><path>./demo.nes</path></game>\n<game><path>missing.nes</path></game>\n<game><path>../escape.nes</path></game>\n</gameList>");
    let dreamcast = f.catalog.config.library.join("dreamcast");
    fs::create_dir(&dreamcast).unwrap();
    fs::write(dreamcast.join("Sonic.gdi"), b"gdi").unwrap();
    fs::write(
        dreamcast.join("gamelist.xml"),
        "<gameList><game><path>Sonic.gdi</path><name>Sonic Adventure</name></game></gameList>",
    )
    .unwrap();
    f.catalog.reindex().unwrap();
    let (_, page) = f
        .json("GET", "/api/v1/catalog/issues?system=nes&limit=2")
        .await;
    assert_eq!(page["total"], 3);
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let (_, page) = f
        .json("GET", "/api/v1/catalog/issues?system=nes&offset=2&limit=2")
        .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let (_, page) = f.json("GET", "/api/v1/catalog/issues?system=nes").await;
    let issues = page["items"].as_array().unwrap();
    assert!(issues.iter().any(|issue| issue["path"] == "./demo.nes"
        && issue["message"] == "Entrée dupliquée (ligne XML 3)"));
    assert!(issues.iter().any(|issue| issue["path"] == "missing.nes"
        && issue["message"] == "Fichier ROM absent (ligne XML 4)"));
    assert!(issues.iter().any(|issue| issue["path"] == "../escape.nes"
        && issue["message"]
            .as_str()
            .unwrap()
            .contains("Chemin de ROM invalide")));
    let (_, page) = f
        .json("GET", "/api/v1/catalog/issues?system=dreamcast")
        .await;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["name"], "Sonic Adventure");
    assert_eq!(page["items"][0]["path"], "Sonic.gdi");
    assert_eq!(page["items"][0]["kind"], "unavailable");
    assert_eq!(
        page["items"][0]["message"],
        "Format multi-fichiers non pris en charge"
    );
    let (_, page) = f.json("GET", "/api/v1/catalog/issues").await;
    assert!(!page.to_string().contains(f.temp.path().to_str().unwrap()));
    for query in [
        "limit=0",
        "limit=201",
        "offset=-1",
        "offset=18446744073709551615",
    ] {
        assert_eq!(
            f.json("GET", &format!("/api/v1/catalog/issues?{query}"))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}

#[test]
fn configuration_guards_and_relative_path_normalization_match_legacy() {
    let f = Fixture::new();
    let root = &f.catalog.config.library;
    assert!(Config::new(root, root.join("database.sqlite3"), None).is_err());
    assert!(Config::new(
        root,
        &f.catalog.config.database,
        Some(&root.join("nes/demo.nes"))
    )
    .is_err());
    assert!(Config::new(
        root,
        &f.catalog.config.database,
        Some(&f.catalog.config.database)
    )
    .is_err());
    assert_eq!(
        relative_path("./nested//game.nes").unwrap(),
        "nested/game.nes"
    );
    assert_eq!(identifier(&["nes", "demo.nes"]), f.game().id);
    assert_eq!(
        python_json(&json!({"b":"🎮é\u{7f}","a":[1,null,true]})),
        "{\"a\": [1, null, true], \"b\": \"\\ud83c\\udfae\\u00e9\\u007f\"}"
    );
    assert!(Path::new(&f.catalog.config.system_icon).ends_with("system-icons"));
}

#[test]
fn ten_thousand_entries_remain_paged_and_warm_hashes_are_reused() {
    let f = Fixture::new();
    let mut xml = String::from("<gameList>");
    for index in 0..10_050 {
        // Distinct hard links keep this scale test fast while exercising unique catalogue paths.
        let name = format!("game{index:05}.nes");
        fs::hard_link(f.system().join("demo.nes"), f.system().join(&name)).unwrap();
        xml.push_str(&format!("<game><path>{name}</path></game>"));
    }
    xml.push_str("</gameList>");
    f.xml(&xml);
    let report = f.catalog.reindex().unwrap();
    assert_eq!(report.games, 10_050);
    let report = f.catalog.reindex().unwrap();
    assert_eq!(report.reused, 10_050);
    assert_eq!(report.calculated, 0);
    let page = f.catalog.page(None, "", 10_000, 50).unwrap();
    assert_eq!(page.total, 10_050);
    assert_eq!(page.items.len(), 50);
}

#[test]
fn global_and_platform_search_span_multiple_systems_without_duplicates() {
    let f = Fixture::new();
    f.xml(&XML.replace("Démo", "Éclair &amp; Mario"));
    let other = f.catalog.config.library.join("snes");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("demo.sfc"), b"synthetic second platform").unwrap();
    fs::write(
        other.join("gamelist.xml"),
        "<gameList><game><path>demo.sfc</path><name>éCLAIR &amp; Mario</name></game></gameList>",
    )
    .unwrap();
    f.catalog.reindex().unwrap();
    let first = f.catalog.page(None, "ÉCLAIR & Mario", 0, 1).unwrap();
    let second = f.catalog.page(None, "ÉCLAIR & Mario", 1, 1).unwrap();
    assert_eq!(first.total, 2);
    assert_eq!(second.total, 2);
    assert_ne!(first.items[0].id, second.items[0].id);
    assert_ne!(first.items[0].system, second.items[0].system);
    assert_eq!(
        f.catalog
            .page(Some("nes"), "ÉCLAIR & Mario", 0, 50)
            .unwrap()
            .total,
        1
    );
    assert_eq!(f.catalog.systems().unwrap().len(), 2);
}

#[tokio::test]
async fn identical_icon_and_game_media_paths_have_distinct_identifiers() {
    let f = Fixture::new();
    fs::write(f.system().join("nes.png"), b"identical bytes").unwrap();
    f.xml(&XML.replace("./images/demo.png", "./nes.png"));
    f.catalog.reindex().unwrap();
    fs::create_dir(&f.catalog.config.system_icon).unwrap();
    fs::write(
        f.catalog.config.system_icon.join("nes.png"),
        b"identical bytes",
    )
    .unwrap();
    let image = f.catalog.systems().unwrap().remove(0).image.unwrap();
    let game = f.game();
    let media = game.files.iter().find(|file| file.role == "image").unwrap();
    assert_ne!(image.id, media.id);
    fs::write(
        f.catalog.config.system_icon.join("nes.png"),
        b"changed icon",
    )
    .unwrap();
    assert_eq!(
        f.request("GET", &format!("/api/v1/files/{}", image.id), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.request("GET", &format!("/api/v1/files/{}", media.id), None)
            .await
            .2,
        b"identical bytes"
    );
}

#[test]
fn failed_publication_rolls_back_all_tables_and_can_retry() {
    let f = Fixture::new();
    let original = f.game();
    let db = f.catalog.connect().unwrap();
    db.execute_batch("CREATE TRIGGER reject_new_game BEFORE INSERT ON games BEGIN SELECT RAISE(FAIL, 'simulated publication failure'); END;").unwrap();
    f.xml(&XML.replace("Démo", "Renamed"));
    assert!(f.catalog.reindex().is_err());
    assert_eq!(f.game(), original);
    assert!(f
        .catalog
        .get::<StoredFile>("files", &original.files[0].id)
        .unwrap()
        .is_some());
    db.execute_batch("DROP TRIGGER reject_new_game;").unwrap();
    assert_eq!(f.catalog.reindex().unwrap().reused, 2);
    assert_eq!(f.game().name, "Renamed");
}

#[test]
fn chunked_hashing_matches_sha256_and_rejects_changed_signatures() {
    use romstore_server::{paths::Signature, progress::hash_file};
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.iso");
    let content = b"synthetic bytes ".repeat(256 * 1024);
    fs::write(&path, &content).unwrap();
    let signature = Signature::of(&fs::metadata(&path).unwrap());
    assert_eq!(
        hash_file(&path, &signature, "psx", "test.iso").unwrap(),
        hex::encode(Sha256::digest(&content))
    );
    fs::write(&path, b"changed").unwrap();
    assert!(hash_file(&path, &signature, "psx", "test.iso").is_err());
}
