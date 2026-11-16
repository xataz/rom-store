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
        let mut query = vec![
            ("offset", offset.to_string()),
            ("limit", "50".into()),
            ("search", search.into()),
        ];
        if let Some(system) = system {
            query.push(("system", system.into()));
        }
        self.json("games", &query)
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
                r#"[{"id":"empty","name":"Empty","game_count":0},{"id":"nes","name":"NES","game_count":12}]"#,
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
        assert_eq!(systems.len(), 1);
        assert_eq!(systems[0].id, "nes");
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
}
