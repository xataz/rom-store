use crate::{
    config::Config,
    identity::{casefold, identifier, python_json},
    models::{
        CatalogIssue, Diagnostic, File, Game, GamePage, IndexReport, IssuePage, System,
        SystemSummary,
    },
    paths::{regular_file, reject_symlink_root, relative_path, source_file, Signature},
    progress,
};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use rusqlite::{functions::FunctionFlags, params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, OpenOptions},
    io::Read,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tracing::{debug, info, warn};

const MAX_XML: u64 = 16 * 1024 * 1024;
const MAX_ICON: u64 = 8 * 1024 * 1024;
const ICON_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "pnm", "ppm"];
pub const MEDIA_TAGS: &[&str] = &[
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
const LOCAL_TAGS: &[&str] = &["favorite", "hidden", "playcount", "lastplayed", "gametime"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredFile {
    pub id: String,
    pub system: String,
    pub relative: String,
    pub signature: Signature,
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl StoredFile {
    fn descriptor(&self, role: &str) -> File {
        File {
            id: self.id.clone(),
            role: role.into(),
            destination: self.relative.clone(),
            size: self.size,
            sha256: self.sha256.clone(),
        }
    }
}

#[derive(Clone)]
pub struct Catalog {
    pub config: Config,
    icons: Arc<Mutex<()>>,
}

type HashCache = HashMap<String, (Signature, String)>;

impl Catalog {
    pub fn new(config: Config) -> Result<Self> {
        info!(
            "Ouverture du catalogue SQLite : {}",
            config.database.display()
        );
        let catalog = Self {
            config,
            icons: Arc::new(Mutex::new(())),
        };
        let mut db = catalog.connect()?;
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS systems (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS games (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS files (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS system_images (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS diagnostics (payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS hashes (path TEXT PRIMARY KEY, signature TEXT NOT NULL, sha256 TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS games_by_system_name ON games (json_extract(payload, '$.system'), CASEFOLD(json_extract(payload, '$.name')), id);
            CREATE INDEX IF NOT EXISTS games_by_name ON games (CASEFOLD(json_extract(payload, '$.name')), id);")?;
        let version: u32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 1 {
            bail!("Le catalogue SQLite provient d'une version plus récente du serveur");
        }
        if version == 0 {
            // ICU may know newer Unicode characters than Python. Rebuild only
            // the expression index, without discarding payloads or hash caches.
            info!("Migration de l'index de recherche SQLite (données et empreintes conservées)…");
            let tx = db.transaction()?;
            tx.execute_batch(
                "REINDEX games_by_system_name; REINDEX games_by_name; PRAGMA user_version=1;",
            )?;
            tx.commit()?;
        }
        Ok(catalog)
    }

    pub fn connect(&self) -> Result<Connection> {
        let db = Connection::open(&self.config.database)?;
        db.busy_timeout(Duration::from_secs(30))?;
        db.create_scalar_function(
            "CASEFOLD",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |context| Ok(casefold(&context.get::<String>(0)?)),
        )?;
        Ok(db)
    }

    pub fn get<T: DeserializeOwned>(&self, table: &str, id: &str) -> Result<Option<T>> {
        if !matches!(table, "games" | "files" | "system_images") {
            bail!("Table invalide");
        }
        let db = self.connect()?;
        let payload: Option<String> = db
            .query_row(
                &format!("SELECT payload FROM {table} WHERE id = ?1"),
                [id],
                |row| row.get(0),
            )
            .optional()?;
        payload
            .map(|payload| Ok(serde_json::from_str(&payload)?))
            .transpose()
    }

    /// Counts from the published snapshot, without reading ROMs or refreshing icons.
    pub fn summary(&self) -> Result<Vec<SystemSummary>> {
        let db = self.connect()?;
        let mut query = db.prepare(
            "SELECT s.id, json_extract(s.payload, '$.name'), count(g.id),
                    coalesce(sum(json_extract(g.payload, '$.installable')), 0)
             FROM systems s LEFT JOIN games g ON json_extract(g.payload, '$.system') = s.id
             GROUP BY s.id ORDER BY CASEFOLD(json_extract(s.payload, '$.name')), s.id",
        )?;
        let rows = query.query_map([], |row| {
            Ok(SystemSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                games: row.get(2)?,
                installable: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Read diagnostics and unavailable games from one published SQLite snapshot.
    pub fn issues(&self, system: Option<&str>, offset: u64, limit: u64) -> Result<IssuePage> {
        if !(1..=200).contains(&limit) || offset > i64::MAX as u64 {
            bail!("Paramètres invalides");
        }
        let mut db = self.connect()?;
        let tx = db.transaction()?;
        let cte = "WITH issues AS (
            SELECT json_extract(payload, '$.system') AS system,
                   json_extract(payload, '$.game') AS path, NULL AS name,
                   'warning' AS kind, json_extract(payload, '$.message') AS message
            FROM diagnostics
            UNION ALL
            SELECT json_extract(payload, '$.system'), json_extract(payload, '$.path'),
                   json_extract(payload, '$.name'), 'unavailable',
                   coalesce(json_extract(payload, '$.unavailable_reason'), 'Jeu non installable')
            FROM games WHERE json_extract(payload, '$.installable') = 0
        )";
        let filter = "WHERE (?1 IS NULL OR system = ?1)";
        let total = tx.query_row(
            &format!("{cte} SELECT count(*) FROM issues {filter}"),
            [system],
            |row| row.get(0),
        )?;
        let items = {
            let mut query = tx.prepare(&format!(
                "{cte} SELECT system, path, name, kind, message FROM issues {filter}
                ORDER BY system, path, kind, message LIMIT ?2 OFFSET ?3"
            ))?;
            let rows = query.query_map(params![system, limit, offset], |row| {
                Ok(CatalogIssue {
                    system: row.get(0)?,
                    path: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    message: row.get(4)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        tx.commit()?;
        Ok(IssuePage {
            items,
            total,
            offset,
            limit,
        })
    }

    pub fn page(
        &self,
        system: Option<&str>,
        search: &str,
        offset: u64,
        limit: u64,
    ) -> Result<GamePage> {
        if !(1..=200).contains(&limit) || offset > i64::MAX as u64 || search.chars().count() > 256 {
            bail!("Paramètres invalides");
        }
        let mut db = self.connect()?;
        let transaction = db.transaction()?;
        // One read snapshot for the count and selected items, even during publication.
        // Avoid optional-parameter ORs (which hide the system index from SQLite)
        // and avoid folding every name just to count an unfiltered catalogue.
        let mut filters = Vec::new();
        let mut values: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(system) = system {
            filters.push("json_extract(payload, '$.system') = ?");
            values.push(system.to_owned().into());
        }
        if !search.is_empty() {
            filters.push("instr(CASEFOLD(json_extract(payload, '$.name')), ?) > 0");
            values.push(casefold(search).into());
        }
        let filter = if filters.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", filters.join(" AND "))
        };
        let total = transaction.query_row(
            &format!("SELECT count(*) FROM games{filter}"),
            rusqlite::params_from_iter(values.iter()),
            |row| row.get(0),
        )?;
        values.push((limit as i64).into());
        values.push((offset as i64).into());
        let items = transaction.prepare(&format!("SELECT payload FROM games{filter} ORDER BY CASEFOLD(json_extract(payload, '$.name')), id LIMIT ? OFFSET ?"))?
            .query_map(rusqlite::params_from_iter(values.iter()), |row| row.get::<_, String>(0))?
            .map(|payload| Ok(serde_json::from_str(&payload?)?)).collect::<Result<Vec<Game>>>()?;
        transaction.commit()?;
        Ok(GamePage {
            items,
            total,
            offset,
            limit,
        })
    }

    pub fn systems(&self) -> Result<Vec<System>> {
        let _guard = self.icons.lock().unwrap_or_else(|e| e.into_inner());
        let mut db = self.connect()?;
        let mut systems: Vec<System> = db
            .prepare("SELECT payload FROM systems ORDER BY id")?
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect::<Result<_>>()?;
        let cached: HashMap<_, _> = db
            .prepare("SELECT payload FROM system_images")?
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|row| serde_json::from_str::<StoredFile>(&row.ok()?).ok())
            .map(|file| (file.system.clone(), file))
            .collect();
        let mut images = Vec::new();
        for system in &mut systems {
            system.image = None; // Ignore legacy embedded library icons.
            if reject_symlink_root(&self.config.system_icon).is_err() {
                continue;
            }
            for extension in ICON_EXTENSIONS {
                let result = (|| -> Result<StoredFile> {
                    // A stored system ID must never become an arbitrary path.
                    if relative_path(&system.id)? != system.id
                        || Path::new(&system.id).components().count() != 1
                    {
                        bail!("Système invalide");
                    }
                    let name = format!("{}.{extension}", system.id);
                    let (relative, path) = source_file(&self.config.system_icon, &name)?;
                    let mut stream = regular_file(&path)?;
                    let signature = Signature::of(&stream.metadata()?);
                    if signature.2 > MAX_ICON {
                        bail!("Icône trop volumineuse");
                    }
                    let previous = cached.get(&system.id).filter(|old| {
                        old.relative == relative
                            && old.signature == signature
                            && old.source.as_deref() == Some("system-icon")
                    });
                    let sha256 = if let Some(previous) = previous {
                        previous.sha256.clone()
                    } else {
                        use sha2::{Digest, Sha256};
                        let mut bytes = Vec::new();
                        (&mut stream).take(MAX_ICON + 1).read_to_end(&mut bytes)?;
                        if bytes.len() as u64 > MAX_ICON {
                            bail!("Icône trop volumineuse");
                        }
                        hex::encode(Sha256::digest(&bytes))
                    };
                    if signature != Signature::of(&stream.metadata()?)
                        || signature != Signature::of(&fs::metadata(path)?)
                    {
                        bail!("Icône modifiée");
                    }
                    Ok(StoredFile {
                        id: identifier(&["system-icon", &system.id, &relative, &sha256]),
                        system: system.id.clone(),
                        relative,
                        size: signature.2,
                        signature,
                        sha256,
                        source: Some("system-icon".into()),
                    })
                })();
                if let Ok(file) = result {
                    system.image = Some(file.descriptor("image"));
                    images.push(file);
                    break;
                }
            }
        }
        let transaction = db.transaction()?;
        transaction.execute("DELETE FROM system_images", [])?;
        for image in images {
            transaction.execute(
                "INSERT INTO system_images VALUES (?1, ?2)",
                params![image.id, serde_json::to_string(&image)?],
            )?;
        }
        transaction.commit()?;
        Ok(systems)
    }

    pub fn download(&self, id: &str) -> Result<Option<(StoredFile, bool)>> {
        let image: Option<serde_json::Value> = self.get("system_images", id)?;
        if let Some(image) = image.filter(|image| image["source"] == "system-icon") {
            return Ok(Some((serde_json::from_value(image)?, true)));
        }
        Ok(self.get("files", id)?.map(|file| (file, false)))
    }

    pub fn reindex(&self) -> Result<IndexReport> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.config.database.with_extension("index.lock"))?;
        lock.try_lock_exclusive()
            .context("Une indexation est déjà en cours")?;
        let mut scan = Scan::new(self)?;
        info!("Recherche des dossiers de plateformes…");
        let mut roots = fs::read_dir(&self.config.library)?.collect::<std::io::Result<Vec<_>>>()?;
        roots.sort_by_key(|entry| entry.file_name());
        let roots: Vec<_> = roots
            .into_iter()
            .filter(|entry| {
                !entry.file_name().to_string_lossy().starts_with('.')
                    && entry
                        .file_type()
                        .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            })
            .collect();
        info!("{} dossiers de plateformes à examiner", roots.len());
        for (index, root) in roots.iter().enumerate() {
            let name = root.file_name();
            let Some(name) = name.to_str() else {
                scan.diagnose("", "", "Nom de système non UTF-8");
                continue;
            };
            if relative_path(name).is_err() {
                scan.diagnose(name, "", "Nom de système non pris en charge");
                continue;
            }
            let path = root.path();
            if !path.join("gamelist.xml").exists() {
                debug!("[{name}] Dossier ignoré : pas de gamelist.xml");
                continue;
            }
            info!(
                "Dossier {}/{} [{name}] : lecture de gamelist.xml",
                index + 1,
                roots.len()
            );
            if let Err(error) = scan.platform(&path, name) {
                warn!("[{name}] Plateforme ignorée : {error:#}");
                scan.diagnose(name, "", "gamelist.xml invalide ou inaccessible");
            }
        }
        info!(
            "Publication SQLite : {} plateformes, {} ROMs, {} fichiers…",
            scan.systems.len(),
            scan.games.len(),
            scan.files.len()
        );
        let publication = Instant::now();
        let mut db = self.connect()?;
        let tx = db.transaction()?;
        // Only publish once the entire scan succeeded; failures leave the old snapshot intact.
        for table in ["systems", "games", "files", "diagnostics", "hashes"] {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
        for (id, value) in &scan.systems {
            tx.execute(
                "INSERT INTO systems VALUES (?1, ?2)",
                params![id, serde_json::to_string(value)?],
            )?;
        }
        for (id, value) in &scan.games {
            tx.execute(
                "INSERT INTO games VALUES (?1, ?2)",
                params![id, serde_json::to_string(value)?],
            )?;
        }
        for (id, value) in &scan.files {
            tx.execute(
                "INSERT INTO files VALUES (?1, ?2)",
                params![id, serde_json::to_string(value)?],
            )?;
        }
        for value in &scan.diagnostics {
            tx.execute(
                "INSERT INTO diagnostics VALUES (?1)",
                [serde_json::to_string(value)?],
            )?;
        }
        for (path, (signature, sha256)) in &scan.hashes {
            tx.execute(
                "INSERT INTO hashes VALUES (?1, ?2, ?3)",
                params![path, python_json(&serde_json::to_value(signature)?), sha256],
            )?;
        }
        tx.commit()?;
        info!(
            "Publication SQLite terminée en {:.1} s",
            publication.elapsed().as_secs_f64()
        );
        summary(scan.diagnostics.len(), format_args!("Indexation terminée en {:.1} s : {} plateformes, {} entrées ROM trouvées, {} ROMs indexées ({} installables), {} ignorées, {} fichiers — {} empreintes réutilisées, {} calculées ({:.1} Gio lus), {} avertissements", scan.started.elapsed().as_secs_f64(), scan.systems.len(), scan.found, scan.games.len(), scan.systems.values().map(|s|s.game_count).sum::<u64>(), scan.found - scan.games.len(), scan.files.len(), scan.reused, scan.calculated, scan.hashed_bytes as f64 / 1073741824.0, scan.diagnostics.len()));
        Ok(IndexReport {
            systems: scan.systems.len() as u64,
            games: scan.games.len() as u64,
            diagnostics: scan.diagnostics,
            reused: scan.reused,
            calculated: scan.calculated,
        })
    }
}

struct Scan {
    started: Instant,
    cache: HashCache,
    hashes: HashCache,
    systems: BTreeMap<String, System>,
    games: BTreeMap<String, Game>,
    files: BTreeMap<String, StoredFile>,
    diagnostics: Vec<Diagnostic>,
    found: usize,
    reused: u64,
    calculated: u64,
    hashed_bytes: u64,
}

impl Scan {
    fn new(catalog: &Catalog) -> Result<Self> {
        info!(
            "Indexation démarrée : bibliothèque={}",
            catalog.config.library.display()
        );
        info!("Chargement du cache des empreintes…");
        let db = catalog.connect()?;
        let cache = db
            .prepare("SELECT path, signature, sha256 FROM hashes")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .filter_map(|row| {
                let (path, signature, sha) = row.ok()?;
                let signature = serde_json::from_str(&signature).ok()?;
                Some((path, (signature, sha)))
            })
            .collect::<HashCache>();
        info!("Cache chargé : {} empreintes disponibles", cache.len());
        Ok(Self {
            started: Instant::now(),
            cache,
            hashes: HashMap::new(),
            systems: BTreeMap::new(),
            games: BTreeMap::new(),
            files: BTreeMap::new(),
            diagnostics: Vec::new(),
            found: 0,
            reused: 0,
            calculated: 0,
            hashed_bytes: 0,
        })
    }

    fn diagnose(&mut self, system: &str, game: &str, message: &str) {
        debug!("[{system}] Entrée {game:?} : {message}");
        self.diagnostics.push(Diagnostic {
            system: system.into(),
            game: game.into(),
            message: message.into(),
        });
    }

    fn file(&mut self, root: &Path, system: &str, raw: &str, role: &str) -> Result<File> {
        let (relative, path) = source_file(root, raw)?;
        let signature = Signature::of(&fs::metadata(&path)?);
        let key = path.to_str().context("Chemin non UTF-8")?;
        let cached = self
            .hashes
            .get(key)
            .or_else(|| self.cache.get(key))
            .filter(|(old, _)| old == &signature);
        let sha256 = if let Some((_, sha256)) = cached {
            if !self.hashes.contains_key(key) {
                self.reused += 1;
            }
            debug!("[{system}] Empreinte réutilisée : {relative:?}");
            sha256.clone()
        } else {
            let sha = progress::hash_file(&path, &signature, system, &relative)?;
            self.calculated += 1;
            self.hashed_bytes += signature.2;
            sha
        };
        self.hashes
            .insert(key.into(), (signature.clone(), sha256.clone()));
        let stored = StoredFile {
            id: identifier(&[system, &relative, &sha256]),
            system: system.into(),
            size: signature.2,
            relative,
            signature,
            sha256,
            source: None,
        };
        let descriptor = stored.descriptor(role);
        self.files.insert(stored.id.clone(), stored);
        Ok(descriptor)
    }

    fn platform(&mut self, root: &Path, system: &str) -> Result<()> {
        let started = Instant::now();
        reject_symlink_root(root)?;
        let path = root.join("gamelist.xml");
        let mut file = regular_file(&path)?;
        if file.metadata()?.len() > MAX_XML {
            bail!("XML trop volumineux (limite 16 Mio)");
        }
        let mut bytes = Vec::new();
        (&mut file).take(MAX_XML + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_XML {
            bail!("XML trop volumineux");
        }
        let xml = crate::xml::decode(&bytes)?;
        let document = roxmltree::Document::parse(&xml)?; // DTDs/entities are disabled by default.
        let tree = document.root_element();
        if tree.tag_name().name() != "gameList" || tree.tag_name().namespace().is_some() {
            bail!("La racine XML doit être gameList");
        }
        let entries: Vec<_> = tree.children().filter(|node| tag(*node, "game")).collect();
        self.found += entries.len();
        info!(
            "[{system}] {} entrées <game> lues dans {} ({} octets)",
            entries.len(),
            path.display(),
            bytes.len()
        );
        let before = self.games.len();
        let warnings_before = self.diagnostics.len();
        let mut count = 0;
        let mut skipped = 0;
        let mut unavailable = 0;
        let mut last_log = Instant::now();
        for (position, entry) in entries.iter().enumerate() {
            if position > 0
                && (position % progress::BATCH == 0 || last_log.elapsed() >= progress::INTERVAL)
            {
                info!("[{system}] Progression : {position}/{} entrées traitées — plateforme : {} ROMs indexées, {skipped} ignorées, {:.1} s — cumul global : {} ROMs, {} empreintes réutilisées, {} calculées, {:.1} s", entries.len(), self.games.len() - before, started.elapsed().as_secs_f64(), self.games.len(), self.reused, self.calculated, self.started.elapsed().as_secs_f64());
                last_log = Instant::now();
            }
            let raw = entry
                .children()
                .find(|node| tag(*node, "path"))
                .map(text)
                .unwrap_or_default();
            match self.game(root, system, *entry, &raw) {
                Ok(game) => {
                    count += u64::from(game.installable);
                    if !game.installable {
                        if unavailable < 5 {
                            warn!(
                                "[{system}] Jeu non installable {:?} ({:?}) : {}",
                                game.name,
                                game.path,
                                game.unavailable_reason
                                    .as_deref()
                                    .unwrap_or("Raison inconnue")
                            );
                        }
                        unavailable += 1;
                    }
                    self.games.insert(game.id.clone(), game);
                }
                Err(error) => {
                    debug!("[{system}] ROM ignorée {raw:?} : {error:#}");
                    let reason = match error
                        .downcast_ref::<std::io::Error>()
                        .map(|error| error.kind())
                    {
                        Some(std::io::ErrorKind::NotFound) => "Fichier ROM absent".to_owned(),
                        Some(std::io::ErrorKind::PermissionDenied) => {
                            "Fichier ROM inaccessible (permissions)".to_owned()
                        }
                        _ => error.to_string(),
                    };
                    let line = document.text_pos_at(entry.range().start).row;
                    let reason = format!("{reason} (ligne XML {line})");
                    if skipped < 5 {
                        warn!("[{system}] Entrée XML ignorée {raw:?} : {reason}");
                    }
                    skipped += 1;
                    self.diagnose(system, &raw, &reason);
                }
            }
        }
        self.systems.insert(
            system.into(),
            System {
                id: system.into(),
                name: system.into(),
                game_count: count,
                image: None,
            },
        );
        summary(self.diagnostics.len() - warnings_before + unavailable, format_args!("[{system}] Terminé : {}/{} entrées traitées, {} ROMs indexées ({count} installables, {unavailable} non installables), {skipped} ignorées, {} avertissements — {:.1} s ; détails : / (Anomalies du catalogue)", entries.len(), entries.len(), self.games.len() - before, self.diagnostics.len() - warnings_before, started.elapsed().as_secs_f64()));
        Ok(())
    }

    fn game(
        &mut self,
        root: &Path,
        system: &str,
        entry: roxmltree::Node<'_, '_>,
        raw: &str,
    ) -> Result<Game> {
        let path = relative_path(raw).context("Chemin de ROM invalide ou interdit")?;
        let id = identifier(&[system, &path]);
        if self.games.contains_key(&id) {
            bail!("Entrée dupliquée");
        }
        let rom = self
            .file(root, system, raw, "rom")
            .context("ROM absente, inaccessible, interdite ou modifiée")?;
        let mut files = vec![rom];
        let unavailable_reason = if crate::playlist::is_playlist(&path) {
            let result = (|| -> Result<Vec<File>> {
                use sha2::{Digest, Sha256};
                let (_, source) = source_file(root, &path)?;
                let bytes = crate::playlist::read(&source)?;
                if hex::encode(Sha256::digest(&bytes)) != files[0].sha256 {
                    bail!("M3U modifié pendant l'indexation");
                }
                crate::playlist::parse(&path, &bytes)?
                    .iter()
                    .map(|disc| {
                        self.file(root, system, disc, "disk")
                            .with_context(|| format!("Disque absent, interdit ou modifié : {disc}"))
                    })
                    .collect()
            })();
            match result {
                Ok(discs) => {
                    files.extend(discs);
                    None
                }
                Err(error) => {
                    let reason = format!("M3U non installable : {error}");
                    self.diagnose(system, raw, &reason);
                    Some(reason)
                }
            }
        } else if crate::playlist::dependent(&path) {
            Some("Format multi-fichiers non pris en charge".into())
        } else {
            None
        };
        let mut metadata = BTreeMap::new();
        for child in entry.children().filter(|node| node.is_element()) {
            let name = child.tag_name().name();
            if name == "path"
                || LOCAL_TAGS.contains(&name)
                || child.children().any(|node| node.is_element())
            {
                continue;
            }
            if child.tag_name().namespace().is_some() || !metadata_name(name) {
                self.diagnose(system, raw, "Métadonnée XML non prise en charge");
                continue;
            }
            if metadata.contains_key(name) {
                self.diagnose(
                    system,
                    raw,
                    &format!("Métadonnée dupliquée ignorée : {name}"),
                );
                continue;
            }
            let mut value = text(child);
            if MEDIA_TAGS.contains(&name) {
                if value.is_empty() {
                    continue;
                }
                match self.file(root, system, &value, name) {
                    Ok(file) => {
                        value = format!("./{}", file.destination);
                        files.push(file);
                    }
                    Err(error) => {
                        debug!("[{system}] Média ignoré {value:?} : {error:#}");
                        self.diagnose(system, raw, &format!("Média absent ou interdit : {name}"));
                        continue;
                    }
                }
            }
            metadata.insert(name.into(), value);
        }
        let name = metadata
            .entry("name".into())
            .or_insert_with(|| {
                Path::new(&path)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .clone();
        let installable = unavailable_reason.is_none();
        let mut game = Game {
            id,
            system: system.into(),
            path,
            name,
            metadata,
            installable,
            unavailable_reason,
            files,
            revision: String::new(),
        };
        let mut payload = serde_json::to_value(&game)?;
        payload.as_object_mut().unwrap().remove("revision");
        game.revision = identifier(&[&python_json(&payload)]);
        Ok(game)
    }
}

fn tag(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name && node.tag_name().namespace().is_none()
}
fn text(node: roxmltree::Node<'_, '_>) -> String {
    node.children()
        .filter_map(|child| child.is_text().then(|| child.text()).flatten())
        .collect()
}
fn metadata_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().enumerate().all(|(index, c)| {
            c == b'_'
                || c.is_ascii_alphabetic()
                || (index > 0 && (c.is_ascii_digit() || c == b'-' || c == b'.'))
        })
}

fn summary(warnings: usize, message: std::fmt::Arguments<'_>) {
    if warnings == 0 {
        info!("{message}");
    } else {
        warn!("{message}");
    }
}
