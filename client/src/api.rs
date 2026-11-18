use anyhow::{bail, Context, Result};
use reqwest::{Client, Response};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, Cursor, Read},
    sync::Arc,
    time::Duration,
};
use tokio::runtime::Runtime;

#[derive(Debug, Deserialize)]
pub struct ReloadStatus {
    pub status: String,
    pub systems: usize,
    pub games: usize,
    pub diagnostics: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct System {
    pub id: String,
    pub name: String,
    pub game_count: u64,
    pub image: Option<File>,
}

impl System {
    pub fn display_name(&self) -> &str {
        crate::systems::display_name(&self.id, &self.name)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct File {
    pub id: String,
    pub role: String,
    pub destination: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Game {
    /// Local ROM presence, populated by the client rather than the server.
    #[serde(skip)]
    pub installed: bool,
    pub id: String,
    pub system: String,
    pub path: String,
    pub name: String,
    pub metadata: BTreeMap<String, String>,
    pub installable: bool,
    pub unavailable_reason: Option<String>,
    pub files: Vec<File>,
}

#[derive(Debug, Deserialize)]
pub struct Page {
    pub items: Vec<Game>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub version: u32,
    pub game_id: String,
    pub system: String,
    pub path: String,
    pub revision: String,
    pub metadata: BTreeMap<String, String>,
    pub files: Vec<File>,
}

/// Synchronous facade used only by background workers (never the SDL thread).
#[derive(Clone)]
pub struct Api {
    client: Client,
    runtime: Arc<Runtime>,
    base: String,
}

impl Api {
    pub fn new(base: &str) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(24 * 60 * 60))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            runtime: Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()?,
            ),
            base: base.trim_end_matches('/').to_owned(),
        })
    }

    fn json<T: DeserializeOwned>(&self, endpoint: &str, query: &[(&str, String)]) -> Result<T> {
        self.json_request(reqwest::Method::GET, endpoint, query)
    }

    fn json_request<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        self.runtime.block_on(async {
            let response = self
                .client
                .request(method, format!("{}/api/v1/{endpoint}", self.base))
                .query(query)
                .timeout(Duration::from_secs(20))
                .send()
                .await?;
            let mut response = checked(response).await?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                    bail!("Réponse API trop volumineuse");
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).context("Réponse API invalide")
        })
    }

    pub fn systems(&self) -> Result<Vec<System>> {
        let mut systems: Vec<System> = self.json("systems", &[])?;
        systems.retain(|system| system.game_count > 0);
        systems
            .sort_by_cached_key(|system| (system.display_name().to_lowercase(), system.id.clone()));
        Ok(systems)
    }

    pub fn reload_status(&self) -> Result<ReloadStatus> {
        self.json("catalog/reload", &[])
    }

    pub fn start_reload(&self) -> Result<ReloadStatus> {
        self.json_request(reqwest::Method::POST, "catalog/reload", &[])
    }

    /// Short HTTP requests keep large-library rebuilds independent of JSON timeouts.
    pub fn reload_catalog(&self) -> Result<ReloadStatus> {
        let mut state = self.start_reload()?;
        let started = std::time::Instant::now();
        loop {
            match state.status.as_str() {
                "completed" => return Ok(state),
                "failed" => bail!(
                    "{}",
                    state.error.as_deref().unwrap_or("Échec du rechargement")
                ),
                "running" if started.elapsed() < Duration::from_secs(3600) => (),
                "running" => bail!(
                    "Délai de rechargement dépassé ; le serveur peut continuer son indexation"
                ),
                _ => bail!("État de rechargement serveur invalide"),
            }
            std::thread::sleep(Duration::from_millis(500));
            state = self.reload_status()?;
        }
    }

    pub fn games(&self, system: Option<&str>, offset: usize, search: &str) -> Result<Page> {
        self.games_filtered(system, offset, search, &BTreeMap::new())
    }

    pub fn filter_fields(&self) -> Result<Vec<String>> {
        self.json("games/fields", &[])
    }

    pub fn games_filtered(
        &self,
        system: Option<&str>,
        offset: usize,
        search: &str,
        filters: &BTreeMap<String, String>,
    ) -> Result<Page> {
        self.games_page(system, offset, search, 50, filters)
    }

    fn games_page(
        &self,
        system: Option<&str>,
        offset: usize,
        search: &str,
        limit: usize,
        filters: &BTreeMap<String, String>,
    ) -> Result<Page> {
        let mut query = vec![
            ("offset", offset.to_string()),
            ("limit", limit.to_string()),
            ("search", search.into()),
        ];
        if let Some(system) = system {
            query.push(("system", system.into()));
        }
        if !filters.is_empty() {
            query.push(("filters", serde_json::to_string(filters)?));
        }
        self.json("games", &query)
    }

    /// Locate an initial in the server's case-folded catalogue without downloading
    /// every preceding page. Keep the current platform and search constraints.
    pub fn games_at_letter(
        &self,
        system: Option<&str>,
        search: &str,
        letter: char,
    ) -> Result<Option<(Page, usize)>> {
        self.games_at_letter_filtered(system, search, letter, &BTreeMap::new())
    }

    pub fn games_at_letter_filtered(
        &self,
        system: Option<&str>,
        search: &str,
        letter: char,
        filters: &BTreeMap<String, String>,
    ) -> Result<Option<(Page, usize)>> {
        // Use the same Unicode folding as the server's ORDER BY, not lowercase
        // (e.g. long s, ſ, sorts under s on the server).
        let mapper = icu_casemap::CaseMapper::new();
        let target = mapper.fold_string(&letter.to_string()).into_owned();
        let first = self.games_page(system, 0, search, 1, filters)?;
        let (mut low, mut high) = (0, first.total);
        while low < high {
            let middle = low + (high - low) / 2;
            let probe = self.games_page(system, middle, search, 1, filters)?;
            if probe.total != first.total || probe.offset != middle {
                bail!("Le catalogue a changé pendant le saut alphabétique ; réessayer");
            }
            let Some(game) = probe.items.first() else {
                bail!("Le catalogue a changé pendant le saut alphabétique ; réessayer");
            };
            if mapper.fold_string(&game.name).as_ref() < target.as_str() {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        if low == first.total {
            return Ok(None);
        }
        let offset = low / 50 * 50;
        let page = self.games_filtered(system, offset, search, filters)?;
        if page.total != first.total || page.offset != offset {
            bail!("Le catalogue a changé pendant le saut alphabétique ; réessayer");
        }
        let index = low - offset;
        Ok(page
            .items
            .get(index)
            .is_some_and(|game| mapper.fold_string(&game.name).starts_with(&target))
            .then_some((page, index)))
    }

    pub fn manifest(&self, id: &str) -> Result<Manifest> {
        validate_id(id)?;
        self.json(&format!("games/{id}/manifest"), &[])
    }

    pub fn file(&self, file: &File) -> Result<RemoteFile> {
        validate_id(&file.id)?;
        validate_id(&file.sha256)?;
        let response = self.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), async {
                checked(
                    self.client
                        .get(format!("{}/api/v1/files/{}", self.base, file.id))
                        .header("If-Match", format!("\"{}\"", file.sha256))
                        .send()
                        .await?,
                )
                .await
            })
            .await
            .context("Délai de réponse du serveur dépassé")?
        })?;
        Ok(RemoteFile {
            response,
            runtime: self.runtime.clone(),
            buffer: Cursor::new(Vec::new()),
        })
    }
}

/// Bound every network read separately, so a stalled ISO does not wait 24 hours
/// before cancellation can be observed by the installer.
pub struct RemoteFile {
    response: Response,
    runtime: Arc<Runtime>,
    buffer: Cursor<Vec<u8>>,
}

impl RemoteFile {
    pub fn content_length(&self) -> Option<u64> {
        self.response.content_length()
    }
}

impl Read for RemoteFile {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        while self.buffer.position() as usize >= self.buffer.get_ref().len() {
            let chunk = self
                .runtime
                .block_on(async {
                    tokio::time::timeout(Duration::from_secs(15), self.response.chunk()).await
                })
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Aucune donnée reçue depuis 15 secondes",
                    )
                })?
                .map_err(io::Error::other)?;
            match chunk {
                Some(chunk) => self.buffer = Cursor::new(chunk.to_vec()),
                None => return Ok(0),
            }
        }
        self.buffer.read(output)
    }
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.len() != 64 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("Identifiant ou empreinte invalide");
    }
    Ok(())
}

async fn checked(mut response: Response) -> Result<Response> {
    if !response.status().is_success() {
        let status = response.status();
        let mut bytes = Vec::new();
        while bytes.len() < 2048 {
            let Some(chunk) = response.chunk().await? else {
                break;
            };
            bytes.extend_from_slice(&chunk[..chunk.len().min(2048 - bytes.len())]);
        }
        bail!(
            "Erreur serveur {status} : {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn empty_platforms_are_hidden_on_every_system_list_refresh() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for body in [
                r#"[{"id":"empty","name":"Empty","game_count":0},{"id":"psx","name":"AAA","game_count":2},{"id":"switch","name":"ZZZ","game_count":3},{"id":"nes","name":"NES","game_count":12},{"id":"custom","name":"alpha","game_count":1}]"#,
                r#"[{"id":"nes","name":"NES","game_count":0}]"#,
                r#"[{"id":"snes","name":"SNES","game_count":2}]"#,
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line.trim(), "GET /api/v1/systems HTTP/1.1");
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let api = Api::new(&format!("http://{address}")).unwrap();
        let systems = api.systems().unwrap();
        assert_eq!(
            systems
                .iter()
                .map(|system| system.id.as_str())
                .collect::<Vec<_>>(),
            ["custom", "nes", "switch", "psx"]
        );
        assert!(api.systems().unwrap().is_empty());
        assert_eq!(api.systems().unwrap()[0].id, "snes");
        server.join().unwrap();
    }

    #[test]
    fn reload_starts_once_then_polls_until_completion_or_failure() {
        for failed in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                for (method, status) in [
                    ("POST", "running"),
                    ("GET", "running"),
                    ("GET", if failed { "failed" } else { "completed" }),
                ] {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    assert_eq!(
                        line.trim(),
                        format!("{method} /api/v1/catalog/reload HTTP/1.1")
                    );
                    loop {
                        line.clear();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let body = format!(
                        r#"{{"status":"{status}","systems":2,"games":123,"diagnostics":1,"error":null}}"#
                    );
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            });
            let api = Api::new(&format!("http://{address}")).unwrap();
            let result = api.reload_catalog();
            if failed {
                assert!(result.is_err());
            } else {
                let state = result.unwrap();
                assert_eq!(state.games, 123);
                assert_eq!(state.systems, 2);
                assert_eq!(state.diagnostics, 1);
            }
            server.join().unwrap();
        }
    }

    #[test]
    fn global_and_scoped_search_encode_queries_and_preserve_pagination() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for expected_system in [None, Some("nes")] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                loop {
                    let mut header = String::new();
                    assert!(reader.read_line(&mut header).unwrap() > 0);
                    if header == "\r\n" {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap();
                let url = reqwest::Url::parse(&format!("http://localhost{path}")).unwrap();
                let query: BTreeMap<_, _> = url
                    .query_pairs()
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect();
                assert_eq!(query.get("system").map(String::as_str), expected_system);
                assert_eq!(query["search"], "Éclair & Mario %_'");
                assert_eq!(query["offset"], "50");
                let body = r#"{"items":[],"total":0,"offset":50,"limit":50}"#;
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let api = Api::new(&format!("http://{address}")).unwrap();
        for system in [None, Some("nes")] {
            assert_eq!(
                api.games(system, 50, "Éclair & Mario %_'").unwrap().offset,
                50
            );
        }
        server.join().unwrap();
    }

    #[test]
    fn letter_jump_finds_first_match_beyond_loaded_pages_and_keeps_scope() {
        for (letter, expected, offsets) in [
            ('M', Some((50, 15)), vec![0, 65, 32, 49, 57, 61, 63, 64, 50]),
            ('B', None, vec![0, 65, 32, 49, 57, 61, 63, 64, 50]),
            (
                'Z',
                Some((100, 20)),
                vec![0, 65, 98, 114, 122, 118, 120, 119, 100],
            ),
            ('0', None, vec![0, 65, 32, 16, 8, 4, 2, 1, 0, 0]),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                for (request, expected_offset) in offsets.iter().enumerate() {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let path = line.split_whitespace().nth(1).unwrap();
                    let url = reqwest::Url::parse(&format!("http://localhost{path}")).unwrap();
                    let query: BTreeMap<_, _> = url
                        .query_pairs()
                        .map(|(k, v)| (k.into_owned(), v.into_owned()))
                        .collect();
                    assert_eq!(query["system"], "nes");
                    assert_eq!(query["search"], "demo & test");
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&query["filters"]).unwrap(),
                        serde_json::json!({"year": ">=1990", "players": "2"})
                    );
                    let offset: usize = query["offset"].parse().unwrap();
                    assert_eq!(offset, *expected_offset);
                    let limit: usize = query["limit"].parse().unwrap();
                    assert_eq!(limit, if request == offsets.len() - 1 { 50 } else { 1 });
                    loop {
                        line.clear();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let items: Vec<_> = (offset..(offset + limit).min(130)).map(|index| {
                        let initial = if index < 65 { 'a' } else if index < 90 { 'M' } else if index < 120 { 'ſ' } else { 'z' };
                        serde_json::json!({"id": index.to_string(), "name": format!("{initial}{index:03}"), "system": "nes", "path": "demo.nes", "metadata": {}, "files": [], "installable": true})
                    }).collect();
                    let body = serde_json::json!({"items": items, "offset": offset, "total": 130, "limit": limit}).to_string();
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            });
            let api = Api::new(&format!("http://{address}")).unwrap();
            let found = api
                .games_at_letter_filtered(
                    Some("nes"),
                    "demo & test",
                    letter,
                    &BTreeMap::from([
                        ("year".into(), ">=1990".into()),
                        ("players".into(), "2".into()),
                    ]),
                )
                .unwrap();
            assert_eq!(found.map(|(page, index)| (page.offset, index)), expected);
            server.join().unwrap();
        }
    }
}
