mod advanced;
mod alphabet;
mod artwork;
mod fonts;
mod input;
mod menu;
mod search;
mod selection;
mod settings;
mod smoke;
mod ui;
mod views;

use alphabet::{AlphabetDialog, Outcome as AlphabetOutcome};
use anyhow::{anyhow, bail, Context, Result};
use artwork::{ArtRequest, ArtTarget, ArtworkLoader};
use image::RgbaImage;
use input::{Action, Input};
use menu::{Item as MenuItem, Menu};
use romstore_client::{
    api::{Api, File, Game, Page, ReloadStatus, System},
    config::{Config, ViewMode},
    frontend,
    i18n::Language,
    install::{self, Progress},
    uninstall,
};
use sdl2::video::FullscreenType;
use search::{SearchDialog, SearchOutcome};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashSet, VecDeque},
    fs,
    io::{Cursor, Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
use ui::Painter;

enum Message {
    ReloadStarted(u64, Result<ReloadStatus>),
    ReloadState(u64, Result<ReloadStatus>),
    Reloaded(u64, Result<(ReloadStatus, Vec<System>)>),
    Systems(u64, Result<Vec<System>>),
    Games(u64, usize, Result<Page>),
    CatalogueJump(u64, usize, Result<Page>),
    Letter(u64, char, Result<Option<(Page, usize)>>),
    FilterFields(u64, Result<Vec<String>>),
    FrontendRefreshed(Result<()>),
    Art(u64, Result<RgbaImage>),
    Preview(u64, String, Result<RgbaImage>),
    Marquee(u64, String, Result<RgbaImage>),
    RetainThumbnails(u64, Vec<String>),
    Thumbnail(u64, String, Result<RgbaImage>),
    Started(usize),
    Progress(usize, Progress),
    Finished(usize, Result<()>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Screen {
    Systems,
    Games,
    Detail,
    Queue,
}

struct Job {
    id: usize,
    kind: JobKind,
    game_id: String,
    name: String,
    cancel: Arc<AtomicBool>,
    state: String,
    done: bool,
    succeeded: bool,
    started: Option<Instant>,
    progress: Option<Progress>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobKind {
    Install,
    Uninstall,
}

enum Operation {
    Install(String),
    Uninstall(Box<Game>),
}
type InstallRequest = (usize, Operation, Arc<AtomicBool>, Api, Config);

struct UninstallConfirmation {
    game: Game,
    confirmed: bool,
}

#[derive(Default)]
struct QuitConfirmation {
    confirmed: bool,
}

const GRID_COLUMNS: usize = 5;
const GRID_ROWS: usize = 2;

fn smoke_page() -> Page {
    Page {
        items: (0..12)
            .map(|index| Game {
                installed: false,
                id: format!("{index:064x}"),
                system: "nes".into(),
                path: format!("demo{index}.nes"),
                name: format!("Jeu de démonstration {}", index + 1),
                metadata: Default::default(),
                installable: index != 2,
                unavailable_reason: (index == 2).then(|| "Format multi-fichiers".into()),
                files: Vec::new(),
            })
            .collect(),
        total: 12,
        offset: 0,
        limit: 50,
    }
}

fn grid_start(selected: usize) -> usize {
    (selected / GRID_COLUMNS).saturating_sub(GRID_ROWS - 1) * GRID_COLUMNS
}

fn cover_file(game: &Game) -> Option<&romstore_client::api::File> {
    // Batocera commonly pairs <thumbnail> (cover) with <image> (screenshot).
    ["thumbnail", "boxart"]
        .iter()
        .find_map(|role| game.files.iter().find(|file| file.role == *role))
}

fn screenshot_file(game: &Game) -> Option<&File> {
    ["image", "screenshot", "titleshot"]
        .iter()
        .find_map(|role| {
            game.files.iter().find(|file| {
                file.role == *role
                    && Some(file.id.as_str()) != cover_file(game).map(|cover| cover.id.as_str())
            })
        })
}

fn media_file<'a>(game: &'a Game, role: &str) -> Option<&'a File> {
    game.files.iter().find(|file| file.role == role)
}

fn move_selection(index: usize, len: usize, action: Action, view: ViewMode) -> usize {
    move_grid_selection(
        index,
        len,
        action,
        if view == ViewMode::Grid {
            GRID_COLUMNS
        } else {
            1
        },
    )
}

fn move_grid_selection(index: usize, len: usize, action: Action, columns: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let index = index.min(len - 1);
    match action {
        Action::Up if index >= columns => index - columns,
        Action::Up => (((len - 1) / columns) * columns + index % columns).min(len - 1),
        Action::Down if index / columns < (len - 1) / columns => (index + columns).min(len - 1),
        Action::Down => index % columns,
        Action::Left if columns > 1 => {
            if index == 0 {
                len - 1
            } else {
                index - 1
            }
        }
        Action::Right if columns > 1 => (index + 1) % len,
        _ => index,
    }
}

struct App {
    config: Config,
    config_path: PathBuf,
    settings: Option<settings::SettingsDialog>,
    connection_generation: u64,
    ui_dirty: bool,
    clear_artwork: bool,
    api: Api,
    tx: mpsc::Sender<Message>,
    installer: mpsc::Sender<InstallRequest>,
    generation: u64,
    catalog_generation: u64,
    catalog_pending: Option<usize>,
    gallery_generation: u64,
    preview_generation: u64,
    preview_requests: ArtworkLoader,
    gallery_requests: ArtworkLoader,
    gallery_cancel: Arc<AtomicBool>,
    gallery_ids: HashSet<String>,
    loading: bool,
    reloading: bool,
    reload_poll_pending: bool,
    next_reload_poll: Instant,
    screen: Screen,
    before_queue: Screen,
    systems: Vec<System>,
    system_index: usize,
    page: Option<Page>,
    game_index: usize,
    detail: Option<Game>,
    queue_index: usize,
    next_job_id: usize,
    jobs: Vec<Job>,
    status: String,
    quit_requested: bool,
    quit_confirmation: Option<QuitConfirmation>,
    selection: selection::Selection,
    catalog_system: Option<String>,
    search_query: String,
    search: Option<SearchDialog>,
    alphabet: Option<AlphabetDialog>,
    menu: Option<Menu>,
    advanced: Option<advanced::AdvancedDialog>,
    advanced_generation: u64,
    filters: BTreeMap<String, String>,
    frontend_refreshing: bool,
    uninstall_confirmation: Option<UninstallConfirmation>,
}

impl App {
    fn reload_catalog(&mut self) {
        if self.reloading || self.reload_poll_pending {
            return;
        }
        self.reloading = true;
        self.reload_poll_pending = true;
        self.status = self.config.language.text("reloading_catalog").into();
        let (api, tx, generation) = (
            self.api.clone(),
            self.tx.clone(),
            self.connection_generation,
        );
        thread::spawn(move || {
            let _ = tx.send(Message::ReloadStarted(generation, api.start_reload()));
        });
    }

    fn poll_reload(&mut self) {
        if self.reload_poll_pending || Instant::now() < self.next_reload_poll {
            return;
        }
        self.reload_poll_pending = true;
        let (api, tx, generation) = (
            self.api.clone(),
            self.tx.clone(),
            self.connection_generation,
        );
        thread::spawn(move || {
            let _ = tx.send(Message::ReloadState(generation, api.reload_status()));
        });
    }

    fn receive_reload_status(&mut self, result: Result<ReloadStatus>) {
        self.reload_poll_pending = false;
        self.next_reload_poll = Instant::now() + Duration::from_secs(3);
        match result {
            Ok(state) => match state.status.as_str() {
                "running" => self.reloading = true,
                "completed" if self.reloading => {
                    // Refresh once, without blocking navigation or starting another scan.
                    self.reload_poll_pending = true;
                    let (api, tx, generation) = (
                        self.api.clone(),
                        self.tx.clone(),
                        self.connection_generation,
                    );
                    thread::spawn(move || {
                        let result = api.systems().map(|systems| (state, systems));
                        let _ = tx.send(Message::Reloaded(generation, result));
                    });
                }
                "failed" if self.reloading => self.finish_reload(Err(anyhow!(
                    "{}",
                    state.error.as_deref().unwrap_or("Échec du rechargement")
                ))),
                "idle" => self.reloading = false,
                _ => (),
            },
            Err(error) => {
                // Keep the last known running state: the server may still be scanning.
                self.status = self
                    .config
                    .language
                    .format("reload_failed", &[&format!("{error:#}")]);
            }
        }
    }

    fn finish_reload(&mut self, result: Result<(ReloadStatus, Vec<System>)>) {
        self.reloading = false;
        self.reload_poll_pending = false;
        self.next_reload_poll = Instant::now() + Duration::from_secs(3);
        let (state, systems) = match result {
            Ok(result) => result,
            Err(error) => {
                self.status = self
                    .config
                    .language
                    .format("reload_failed", &[&format!("{error:#}")]);
                return;
            }
        };
        self.selection.clear();
        let selected = self
            .systems
            .get(self.system_index)
            .map(|system| system.id.clone());
        self.systems = systems;
        self.system_index = selected
            .and_then(|id| self.systems.iter().position(|system| system.id == id))
            .unwrap_or(0);
        // Discard requests against the old snapshot, including pending infinite-scroll batches.
        self.generation += 1;
        self.catalog_generation += 1;
        self.catalog_pending = None;
        self.loading = false;
        self.gallery_cancel.store(true, Ordering::Relaxed);
        self.gallery_generation += 1;
        self.preview_generation += 1;
        self.preview_requests.replace(VecDeque::new());
        self.detail = None;
        self.uninstall_confirmation = None;
        self.alphabet = None;
        self.advanced = None;
        self.advanced_generation += 1;
        let screen = if self.screen == Screen::Queue {
            &mut self.before_queue
        } else {
            &mut self.screen
        };
        if self
            .catalog_system
            .as_ref()
            .is_some_and(|id| !self.systems.iter().any(|system| &system.id == id))
        {
            *screen = Screen::Systems;
            self.catalog_system = None;
            self.search_query.clear();
        }
        if *screen == Screen::Detail {
            *screen = Screen::Games;
        }
        if *screen == Screen::Games {
            self.games();
        } else {
            self.page = None;
            self.load_system_thumbnails();
            self.load_preview();
        }
        self.status = self.config.language.format(
            "reload_complete",
            &[&state.systems, &state.games, &state.diagnostics],
        );
    }

    fn load_system_art(&mut self) {
        self.load_preview();
    }

    fn selected_game(&self) -> Option<&Game> {
        if self.screen == Screen::Detail {
            self.detail.as_ref()
        } else {
            self.page
                .as_ref()
                .and_then(|page| page.items.get(self.game_index))
        }
    }

    fn refresh_local_presence(&mut self, game_id: &str) {
        for game in self
            .page
            .iter_mut()
            .flat_map(|page| &mut page.items)
            .chain(self.detail.iter_mut())
        {
            if game.id == game_id {
                game.installed = self.config.rom_exists(&game.system, &game.path);
            }
        }
    }

    fn load_preview(&mut self) {
        self.preview_generation += 1;
        let screen = if self.screen == Screen::Queue {
            self.before_queue
        } else {
            self.screen
        };
        let file = match screen {
            Screen::Systems if self.config.systems_view == ViewMode::List => self
                .systems
                .get(self.system_index)
                .and_then(|system| system.image.clone()),
            Screen::Games if self.config.games_view == ViewMode::List => self
                .page
                .as_ref()
                .and_then(|page| page.items.get(self.game_index))
                .and_then(screenshot_file)
                .cloned(),
            _ => None,
        };
        let mut requests: VecDeque<_> = file
            .into_iter()
            .map(|file| ArtRequest {
                generation: self.preview_generation,
                id: file.id.clone(),
                file,
                cancel: None,
                target: ArtTarget::Preview,
            })
            .collect();
        if screen == Screen::Detail {
            if let Some(file) = self
                .detail
                .as_ref()
                .and_then(|game| media_file(game, "image"))
                .cloned()
            {
                requests.push_front(ArtRequest {
                    generation: self.generation,
                    id: file.id.clone(),
                    file,
                    cancel: None,
                    target: ArtTarget::Detail,
                });
            }
        }
        if screen == Screen::Games && self.config.games_view == ViewMode::List {
            if let Some(file) = self
                .page
                .as_ref()
                .and_then(|page| page.items.get(self.game_index))
                .and_then(|game| media_file(game, "marquee"))
                .cloned()
            {
                requests.push_back(ArtRequest {
                    generation: self.preview_generation,
                    id: file.id.clone(),
                    file,
                    cancel: None,
                    target: ArtTarget::Marquee,
                });
            }
        }
        self.preview_requests.replace(requests);
    }

    fn systems(&mut self) {
        self.generation += 1;
        self.loading = true;
        self.status = self.config.language.text("connecting_server").into();
        let (api, tx, generation) = (self.api.clone(), self.tx.clone(), self.generation);
        thread::spawn(move || {
            let _ = tx.send(Message::Systems(generation, api.systems()));
        });
    }

    fn games(&mut self) {
        self.selection.clear();
        self.generation += 1;
        self.catalog_generation += 1;
        self.page = None;
        self.game_index = 0;
        self.loading = true;
        self.status = self.config.language.text("loading_catalog").into();
        self.preview_generation += 1;
        self.preview_requests.replace(VecDeque::new());
        self.gallery_cancel.store(true, Ordering::Relaxed);
        self.gallery_generation += 1;
        self.gallery_ids.clear();
        self.fetch_games(0);
    }

    fn fetch_games(&mut self, offset: usize) {
        self.catalog_pending = Some(offset);
        let (api, tx, generation, system, query) = (
            self.api.clone(),
            self.tx.clone(),
            self.catalog_generation,
            self.catalog_system.clone(),
            self.search_query.clone(),
        );
        let config = self.config.clone();
        let filters = self.filters.clone();
        thread::spawn(move || {
            let result = api
                .games_filtered(system.as_deref(), offset, &query, &filters)
                .map(|mut page| {
                    for game in &mut page.items {
                        game.installed = config.rom_exists(&game.system, &game.path);
                    }
                    page
                });
            let _ = tx.send(Message::Games(generation, offset, result));
        });
    }

    fn load_more(&mut self) {
        if self.catalog_pending.is_some() {
            return;
        }
        if let Some(page) = &self.page {
            let loaded = page.items.len();
            if page.offset > 0 && self.game_index < 10 {
                self.fetch_games(page.offset.saturating_sub(50));
            } else if page.offset + loaded < page.total
                && self.game_index.saturating_add(20) >= loaded
            {
                self.fetch_games(page.offset + loaded);
            }
        }
    }

    fn navigate_games(&mut self, action: Action) {
        let Some(page) = &self.page else {
            return;
        };
        if page.items.is_empty() {
            return;
        }
        let current = page.offset + self.game_index;
        let target = move_selection(current, page.total, action, self.config.games_view);
        let wrapping = match action {
            Action::Up | Action::Left => target > current,
            Action::Down | Action::Right => target < current,
            _ => false,
        };
        if target >= page.offset && target < page.offset + page.items.len() {
            let previous = self.game_index;
            self.game_index = target - page.offset;
            if previous != self.game_index {
                self.load_thumbnails();
                self.load_preview();
            }
            self.load_more();
        } else if !wrapping {
            // At a loaded-page boundary keep the current row until scrolling
            // finishes, rather than cancelling and duplicating its request.
            self.load_more();
        } else {
            // Navigate over the entire filtered catalogue, not only loaded pages.
            // Invalidate any scroll request that was already in flight.
            self.catalog_generation += 1;
            self.catalog_pending = None;
            self.loading = true;
            let (api, config, tx, generation, system, query, filters) = (
                self.api.clone(),
                self.config.clone(),
                self.tx.clone(),
                self.catalog_generation,
                self.catalog_system.clone(),
                self.search_query.clone(),
                self.filters.clone(),
            );
            thread::spawn(move || {
                let result = api
                    .games_filtered(system.as_deref(), target / 50 * 50, &query, &filters)
                    .map(|mut page| {
                        for game in &mut page.items {
                            game.installed = config.rom_exists(&game.system, &game.path);
                        }
                        page
                    });
                let _ = tx.send(Message::CatalogueJump(generation, target, result));
            });
        }
    }

    fn receive_catalogue_jump(
        &mut self,
        generation: u64,
        target: usize,
        result: Result<Page>,
    ) -> bool {
        if generation != self.catalog_generation {
            return false;
        }
        self.loading = false;
        match result {
            Ok(mut page) if target >= page.offset && target - page.offset < page.items.len() => {
                for game in &mut page.items {
                    game.installed = self.config.rom_exists(&game.system, &game.path);
                }
                self.game_index = target - page.offset;
                self.page = Some(page);
                self.status = self.config.language.text("navigation_hint").into();
                true
            }
            result => {
                let detail = result.err().map_or_else(
                    || "Catalogue modifié pendant la navigation".into(),
                    |error| format!("{error:#}"),
                );
                self.status = self.config.language.format("scroll_retry", &[&detail]);
                false
            }
        }
    }

    fn focus_current_job(&mut self) {
        self.queue_index = self
            .jobs
            .iter()
            .position(|job| !job.done && job.started.is_some() && job.kind == JobKind::Install)
            .or_else(|| {
                self.jobs
                    .iter()
                    .position(|job| !job.done && job.started.is_some())
            })
            .or_else(|| self.jobs.iter().position(|job| !job.done))
            .unwrap_or_else(|| self.queue_index.min(self.jobs.len().saturating_sub(1)));
    }

    fn clear_completed_jobs(&mut self) {
        let selected = self.jobs.get(self.queue_index).map(|job| job.id);
        let count = self.jobs.len();
        self.jobs.retain(|job| !job.done);
        if let Some(index) = self.jobs.iter().position(|job| Some(job.id) == selected) {
            self.queue_index = index;
        } else {
            self.focus_current_job();
        }
        self.status = self
            .config
            .language
            .format("queue_cleaned", &[&(count - self.jobs.len())]);
    }

    fn receive_games(&mut self, generation: u64, offset: usize, result: Result<Page>) -> bool {
        if generation != self.catalog_generation || self.catalog_pending != Some(offset) {
            return false;
        }
        self.catalog_pending = None;
        self.loading = false;
        match result {
            Ok(mut batch) if batch.offset == offset => {
                // A download can finish while this batch is still in flight.
                for game in &mut batch.items {
                    // History may already have been cleared; disk remains authoritative.
                    game.installed = self.config.rom_exists(&game.system, &game.path);
                }
                // An empty batch ends scrolling even if the server total is stale.
                if batch.items.is_empty() {
                    batch.total = offset;
                }
                if self.page.is_none() {
                    self.page = Some(batch);
                } else if let Some(page) = &mut self.page {
                    if offset < page.offset && offset + batch.items.len() >= page.offset {
                        batch.items.truncate(page.offset - offset);
                        self.game_index += batch.items.len();
                        batch.items.append(&mut page.items);
                        page.items = batch.items;
                        page.offset = offset;
                        page.total = batch.total;
                    } else if page.offset + page.items.len() == offset {
                        page.total = batch.total;
                        page.items.extend(batch.items);
                    } else {
                        return false;
                    }
                }
                self.status = self.config.language.text("navigation_hint").into();
                true
            }
            result => {
                let error = result.err().map_or_else(
                    || "Décalage de catalogue invalide".into(),
                    |e| format!("{e:#}"),
                );
                self.status = self.config.language.format(
                    if offset == 0 {
                        "retry_back"
                    } else {
                        "scroll_retry"
                    },
                    &[&error],
                );
                false
            }
        }
    }

    fn open_detail(&mut self) {
        let Some(game) = self
            .page
            .as_ref()
            .and_then(|p| p.items.get(self.game_index))
            .cloned()
        else {
            return;
        };
        self.screen = Screen::Detail;
        self.generation += 1;
        self.detail = Some(game);
        let id = self.detail.as_ref().unwrap().id.clone();
        self.refresh_local_presence(&id);
        self.load_preview();
        self.status = self
            .config
            .language
            .text(if self.detail.as_ref().unwrap().installed {
                "uninstall_hint"
            } else {
                "add_to_queue_hint"
            })
            .into();
    }

    fn load_thumbnails(&mut self) {
        let start = if self.config.games_view == ViewMode::Grid {
            grid_start(self.game_index).saturating_sub(GRID_COLUMNS)
        } else {
            self.game_index.saturating_sub(8)
        };
        let mut requests: VecDeque<_> = self
            .page
            .as_ref()
            .into_iter()
            .flat_map(|page| &page.items)
            .skip(start)
            .take(20)
            .filter_map(|game| cover_file(game).map(|file| (game.id.clone(), file.clone())))
            .collect();
        // Keep the focused cover responsive when rapid scrolling replaces work.
        if let Some(game) = self
            .page
            .as_ref()
            .and_then(|page| page.items.get(self.game_index))
        {
            requests
                .make_contiguous()
                .sort_by_key(|(id, _)| id != &game.id);
        }
        self.load_gallery(requests);
    }

    fn load_system_thumbnails(&mut self) {
        if self.config.systems_view != ViewMode::Grid {
            self.load_gallery(VecDeque::new());
            return;
        }
        let requests = self
            .systems
            .iter()
            .skip(grid_start(self.system_index))
            .take(GRID_COLUMNS * GRID_ROWS)
            .filter_map(|system| {
                system
                    .image
                    .clone()
                    .map(|file| (format!("system:{}", system.id), file))
            })
            .collect();
        self.load_gallery(requests);
    }

    fn load_gallery(&mut self, requests: VecDeque<(String, File)>) {
        let ids: HashSet<_> = requests.iter().map(|(id, _)| id.clone()).collect();
        if ids == self.gallery_ids && !self.gallery_cancel.load(Ordering::Relaxed) {
            return;
        }
        self.gallery_cancel.store(true, Ordering::Relaxed);
        self.gallery_generation += 1;
        self.gallery_cancel = Arc::new(AtomicBool::new(false));
        let _ = self.tx.send(Message::RetainThumbnails(
            self.gallery_generation,
            ids.iter().cloned().collect(),
        ));
        self.gallery_ids = ids;
        self.gallery_requests.replace(
            requests
                .into_iter()
                .map(|(id, file)| ArtRequest {
                    generation: self.gallery_generation,
                    id,
                    file,
                    cancel: Some(self.gallery_cancel.clone()),
                    target: ArtTarget::Thumbnail,
                })
                .collect(),
        );
    }

    fn enqueue(&mut self) -> Result<()> {
        if self.screen != Screen::Detail {
            return Ok(());
        }
        let Some(game) = self.selected_game().cloned() else {
            return Ok(());
        };
        if !game.installable {
            self.status = self
                .config
                .language
                .unavailable_reason(game.unavailable_reason.as_deref())
                .into();
            return Ok(());
        }
        if game.installed {
            return Ok(());
        }
        self.enqueue_job(game, JobKind::Install)
    }

    fn enqueue_job(&mut self, game: Game, kind: JobKind) -> Result<()> {
        if self.frontend_refreshing {
            self.status = self.config.language.text("refresh_running").into();
            return Ok(());
        }
        if self.jobs.iter().any(|j| j.game_id == game.id && !j.done) {
            self.status = self.config.language.text("already_queued").into();
            return Ok(());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let id = self.next_job_id;
        let job = Job {
            id,
            kind,
            game_id: game.id.clone(),
            name: if kind == JobKind::Uninstall {
                self.config.language.format("uninstall_job", &[&game.name])
            } else {
                game.name.clone()
            },
            cancel: cancel.clone(),
            state: self.config.language.text("waiting").into(),
            done: false,
            succeeded: false,
            started: None,
            progress: None,
        };
        let operation = match kind {
            JobKind::Install => Operation::Install(game.id),
            JobKind::Uninstall => Operation::Uninstall(Box::new(game)),
        };
        self.installer
            .send((id, operation, cancel, self.api.clone(), self.config.clone()))?;
        self.jobs.push(job);
        self.next_job_id += 1;
        self.status = self.config.language.text("queued_hint").into();
        Ok(())
    }

    fn catalogue_input_active(&self) -> bool {
        self.screen == Screen::Games
            && !self.loading
            && !self.quit_requested
            && self.menu.is_none()
            && self.settings.is_none()
            && self.search.is_none()
            && self.alphabet.is_none()
            && self.advanced.is_none()
            && self.uninstall_confirmation.is_none()
            && self.quit_confirmation.is_none()
    }

    fn toggle_selected(&mut self) {
        let Some(mut game) = self.selected_game().cloned() else {
            return;
        };
        if self.selection.contains(&game.id) {
            self.selection.toggle(game);
        } else {
            game.installed = self.config.rom_exists(&game.system, &game.path);
            if !game.installable
                || game.installed
                || self
                    .jobs
                    .iter()
                    .any(|job| job.game_id == game.id && !job.done)
            {
                self.status = self.config.language.text("selection_unavailable").into();
                return;
            }
            self.selection.toggle(game);
        }
        self.status = self
            .config
            .language
            .format("selection_status", &[&self.selection.games.len()]);
    }

    fn download_selected(&mut self) {
        if self.frontend_refreshing {
            self.status = self.config.language.text("refresh_running").into();
            return;
        }
        if self.selection.games.is_empty() {
            self.status = self.config.language.text("selection_empty").into();
            return;
        }
        let mut selected: VecDeque<_> = std::mem::take(&mut self.selection.games).into();
        let mut added = 0;
        let mut skipped = 0;
        while let Some(game) = selected.pop_front() {
            if !game.installable
                || self.config.rom_exists(&game.system, &game.path)
                || self
                    .jobs
                    .iter()
                    .any(|job| job.game_id == game.id && !job.done)
            {
                skipped += 1;
                continue;
            }
            if let Err(error) = self.enqueue_job(game.clone(), JobKind::Install) {
                self.selection.games = std::iter::once(game).chain(selected).collect();
                self.status = self
                    .config
                    .language
                    .format("selection_failed", &[&added, &format!("{error:#}")]);
                return;
            }
            added += 1;
        }
        self.selection.clear();
        self.status = self
            .config
            .language
            .format("selection_queued", &[&added, &skipped]);
    }

    fn begin_shutdown(&mut self) -> bool {
        self.quit_confirmation = None;
        if self.jobs.iter().all(|job| job.done) && !self.frontend_refreshing {
            return false;
        }
        self.quit_requested = true;
        for job in &self.jobs {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.screen = Screen::Queue;
        self.status = self.config.language.text("closing").into();
        true
    }

    fn action(&mut self, action: Action) -> Result<bool> {
        if self.quit_requested {
            return Ok(self.frontend_refreshing || self.jobs.iter().any(|job| !job.done));
        }
        if self.quit_confirmation.is_some() {
            match action {
                Action::Left | Action::Up => {
                    self.quit_confirmation.as_mut().unwrap().confirmed = false
                }
                Action::Right | Action::Down => {
                    self.quit_confirmation.as_mut().unwrap().confirmed = true
                }
                Action::Back | Action::Menu => self.quit_confirmation = None,
                Action::Confirm => {
                    let confirmation = self.quit_confirmation.take().unwrap();
                    if confirmation.confirmed {
                        return Ok(self.begin_shutdown());
                    }
                }
                _ => (),
            }
            return Ok(true);
        }
        if self.settings.is_some() {
            if matches!(action, Action::Menu | Action::Quit) {
                if self.close_settings() && action == Action::Quit {
                    return self.action(Action::Quit);
                }
            } else if self.settings.as_mut().unwrap().action(action) {
                self.close_settings();
            }
            return Ok(true);
        }
        if action == Action::Menu {
            if self.menu.is_some() {
                self.menu = None;
            } else {
                self.search = None;
                self.alphabet = None;
                self.advanced = None;
                self.uninstall_confirmation = None;
                self.menu = Some(Menu::default());
            }
            return Ok(true);
        }
        if self.menu.is_some() && action != Action::Quit {
            return self.menu_action(action);
        }
        if let Some(dialog) = &mut self.advanced {
            if action != Action::Quit {
                let outcome = dialog.action(action);
                self.advanced_outcome(outcome);
                return Ok(true);
            }
        }
        if self.uninstall_confirmation.is_some() && action != Action::Quit {
            match action {
                Action::Left | Action::Up => {
                    self.uninstall_confirmation.as_mut().unwrap().confirmed = false
                }
                Action::Right | Action::Down => {
                    self.uninstall_confirmation.as_mut().unwrap().confirmed = true
                }
                Action::Back => self.uninstall_confirmation = None,
                Action::Confirm => {
                    let confirmation = self.uninstall_confirmation.take().unwrap();
                    if confirmation.confirmed {
                        self.enqueue_job(confirmation.game, JobKind::Uninstall)?;
                    }
                }
                _ => (),
            }
            return Ok(true);
        }
        if self.search.is_some() && action != Action::Quit {
            let outcome = self.search.as_mut().unwrap().action(action);
            self.search_outcome(outcome);
            return Ok(true);
        }
        if self.alphabet.is_some() && action != Action::Quit {
            let outcome = self.alphabet.as_mut().unwrap().action(action);
            self.alphabet_outcome(outcome);
            return Ok(true);
        }
        if action == Action::Quit {
            self.search = None;
            self.alphabet = None;
            self.menu = None;
            self.advanced = None;
            self.uninstall_confirmation = None;
            if self.jobs.iter().any(|job| !job.done) || self.frontend_refreshing {
                self.quit_confirmation = Some(QuitConfirmation::default());
                return Ok(true);
            }
            return Ok(false);
        }
        match action {
            Action::Select if self.catalogue_input_active() => self.toggle_selected(),
            Action::DownloadSelection
                if self.catalogue_input_active() && self.selection.enabled =>
            {
                self.download_selected()
            }
            Action::DownloadSelection if self.screen == Screen::Queue => {
                self.clear_completed_jobs()
            }
            Action::Alphabet
                if matches!(self.screen, Screen::Systems | Screen::Games) && !self.loading =>
            {
                let name = if self.screen == Screen::Systems {
                    self.systems
                        .get(self.system_index)
                        .map(System::display_name)
                } else {
                    self.selected_game().map(|game| game.name.as_str())
                };
                self.alphabet = Some(AlphabetDialog::new(name.unwrap_or("A")));
            }
            Action::Search if matches!(self.screen, Screen::Systems | Screen::Games) => {
                self.open_search()
            }
            Action::Back if self.screen == Screen::Games && self.selection.enabled => {
                self.selection.clear()
            }
            Action::Back => match self.screen {
                Screen::Queue => self.screen = self.before_queue,
                Screen::Detail => {
                    self.screen = Screen::Games;
                    self.generation += 1;
                    self.load_preview();
                }
                Screen::Games => {
                    self.catalog_generation += 1;
                    self.catalog_pending = None;
                    self.screen = Screen::Systems;
                    self.search_query.clear();
                    self.filters.clear();
                    self.catalog_system = None;
                    self.gallery_cancel.store(true, Ordering::Relaxed);
                    self.gallery_generation += 1;
                    self.systems();
                }
                Screen::Systems => (),
            },
            Action::Up | Action::Down | Action::Left | Action::Right
                if matches!(self.screen, Screen::Systems | Screen::Games) && !self.loading =>
            {
                if self.screen == Screen::Games {
                    self.navigate_games(action);
                } else {
                    let previous = self.system_index;
                    self.system_index = move_selection(
                        previous,
                        self.systems.len(),
                        action,
                        self.config.systems_view,
                    );
                    if previous != self.system_index {
                        if self.config.systems_view == ViewMode::Grid
                            && grid_start(previous) != grid_start(self.system_index)
                        {
                            self.load_system_thumbnails();
                        }
                        self.load_preview();
                    }
                }
            }
            Action::Up | Action::Down => {
                let (index, len) = match self.screen {
                    Screen::Queue => (&mut self.queue_index, self.jobs.len()),
                    _ => return Ok(true),
                };
                *index = move_selection(*index, len, action, ViewMode::List);
            }
            Action::Confirm => match self.screen {
                Screen::Systems if !self.loading => {
                    if !self.systems.is_empty() {
                        self.catalog_system = self
                            .systems
                            .get(self.system_index)
                            .map(|system| system.id.clone());
                        self.search_query.clear();
                        self.filters.clear();
                        self.screen = Screen::Games;
                        self.page = None;
                        self.game_index = 0;
                        self.games();
                    } else {
                        self.systems();
                    }
                }
                Screen::Games if !self.loading && self.selection.enabled => self.toggle_selected(),
                Screen::Games if !self.loading => self.open_detail(),
                Screen::Queue => {
                    if let Some(job) = self.jobs.get_mut(self.queue_index).filter(|job| !job.done) {
                        job.cancel.store(true, Ordering::Relaxed);
                        job.state = self.config.language.text("cancel_requested").into();
                    }
                }
                Screen::Detail => {
                    if let Some(game) = self.detail.as_ref().filter(|game| game.installed).cloned()
                    {
                        if self
                            .jobs
                            .iter()
                            .any(|job| job.game_id == game.id && !job.done)
                        {
                            self.status = self.config.language.text("already_queued").into();
                        } else {
                            self.uninstall_confirmation = Some(UninstallConfirmation {
                                game,
                                confirmed: false,
                            });
                        }
                    } else {
                        self.enqueue()?;
                    }
                }
                _ => (),
            },
            _ => (),
        }
        Ok(true)
    }

    fn message(&mut self, message: Message, painter: &mut Painter) -> Result<()> {
        match message {
            Message::FrontendRefreshed(result) => {
                self.frontend_refreshing = false;
                self.status = match result {
                    Ok(()) => self.config.language.text("refresh_complete").into(),
                    Err(error) => self
                        .config
                        .language
                        .format("refresh_failed", &[&format!("{error:#}")]),
                };
            }
            Message::FilterFields(generation, result) if generation == self.advanced_generation => {
                if let Some(dialog) = &mut self.advanced {
                    match result {
                        Ok(fields) => dialog.set_fields(fields),
                        Err(error) => {
                            dialog.loading = false;
                            dialog.error = Some(
                                self.config
                                    .language
                                    .format("advanced_fields_failed", &[&format!("{error:#}")]),
                            );
                        }
                    }
                }
            }
            Message::ReloadStarted(generation, result)
                if generation == self.connection_generation =>
            {
                match result {
                    Err(error) => self.finish_reload(Err(error)),
                    Ok(state) => self.receive_reload_status(Ok(state)),
                }
            }
            Message::ReloadState(generation, result)
                if generation == self.connection_generation =>
            {
                self.receive_reload_status(result)
            }
            Message::Reloaded(generation, result) if generation == self.connection_generation => {
                self.finish_reload(result)
            }
            Message::Systems(generation, result) if generation == self.generation => {
                self.loading = false;
                match result {
                    Ok(systems) => {
                        self.systems = systems;
                        painter.clear_preview();
                        self.system_index =
                            self.system_index.min(self.systems.len().saturating_sub(1));
                        self.load_system_art();
                        if self.config.systems_view == ViewMode::Grid {
                            self.load_system_thumbnails();
                        }
                        self.status = if self.systems.is_empty() {
                            self.config.language.text("empty_catalog").into()
                        } else {
                            self.config.language.text("choose_system").into()
                        };
                    }
                    Err(error) => {
                        self.status = self
                            .config
                            .language
                            .format("retry_confirm", &[&format!("{error:#}")])
                    }
                }
            }
            Message::Games(generation, offset, result) => {
                if self.receive_games(generation, offset, result) {
                    self.load_thumbnails();
                    if offset == 0 {
                        self.load_preview();
                    }
                }
            }
            Message::CatalogueJump(generation, target, result) => {
                if self.receive_catalogue_jump(generation, target, result) {
                    self.load_thumbnails();
                    self.load_preview();
                }
            }
            Message::Letter(generation, letter, result)
                if generation == self.catalog_generation =>
            {
                self.loading = false;
                match result {
                    Ok(Some((mut page, index))) => {
                        for game in &mut page.items {
                            game.installed = self.config.rom_exists(&game.system, &game.path);
                        }
                        self.page = Some(page);
                        self.game_index = index;
                        self.load_thumbnails();
                        self.load_preview();
                        self.status = self.config.language.text("navigation_hint").into();
                    }
                    Ok(None) => {
                        self.status = self.config.language.format("alphabet_missing", &[&letter])
                    }
                    Err(error) => {
                        self.status = self
                            .config
                            .language
                            .format("retry_back", &[&format!("{error:#}")])
                    }
                }
            }
            Message::Art(generation, result) if generation == self.generation => match result {
                Ok(art) => painter.set_art(Some(art))?,
                Err(error) => eprintln!("Image du jeu indisponible : {error:#}"),
            },
            Message::Preview(generation, id, result) if generation == self.preview_generation => {
                match result {
                    Ok(art) => painter.set_preview(id, art)?,
                    Err(error) => eprintln!("Aperçu indisponible ({id}) : {error:#}"),
                }
            }
            Message::Marquee(generation, id, result) if generation == self.preview_generation => {
                match result {
                    Ok(art) => painter.set_marquee(id, art)?,
                    Err(error) => eprintln!("Logo indisponible : {error:#}"),
                }
            }
            Message::RetainThumbnails(generation, ids) if generation == self.gallery_generation => {
                painter.retain_thumbnails(&ids)
            }
            Message::Thumbnail(generation, id, result) if generation == self.gallery_generation => {
                match result {
                    Ok(art) => painter.set_thumbnail(id, art)?,
                    Err(error) => eprintln!("Jaquette indisponible ({id}) : {error:#}"),
                }
            }
            Message::Started(id) => {
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    job.started = Some(Instant::now());
                    job.state = self
                        .config
                        .language
                        .text(if job.kind == JobKind::Uninstall {
                            "uninstalling"
                        } else {
                            "installing"
                        })
                        .into();
                }
            }
            Message::Progress(id, progress) => {
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    job.progress = Some(progress);
                }
            }
            Message::Finished(id, result) => self.finish_job(id, result),
            _ => (),
        }
        Ok(())
    }

    fn finish_job(&mut self, id: usize, result: Result<()>) {
        if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
            job.done = true;
            job.succeeded = result.is_ok();
            job.state = match result {
                Ok(()) => self
                    .config
                    .language
                    .text(if job.kind == JobKind::Uninstall {
                        "uninstalled_hint"
                    } else {
                        "installed_hint"
                    })
                    .into(),
                Err(error) => {
                    eprintln!("Opération locale : {error:#}");
                    if job.cancel.load(Ordering::Relaxed) {
                        self.config.language.text("cancelled_hint").into()
                    } else {
                        let detail = if let Some(space) =
                            error.downcast_ref::<romstore_client::space::InsufficientSpace>()
                        {
                            self.config.language.format(
                                "disk_space_failed",
                                &[
                                    &space.path.display(),
                                    &format!("{:.2}", space.required as f64 / 1048576.0),
                                    &format!("{:.2}", space.available as f64 / 1048576.0),
                                ],
                            )
                        } else {
                            format!("{error:#}")
                        };
                        self.config.language.format(
                            if job.kind == JobKind::Uninstall {
                                "uninstall_failed"
                            } else {
                                "install_failed"
                            },
                            &[&detail],
                        )
                    }
                }
            };
            self.status = job.state.clone();
        }
        if let Some(game_id) = self
            .jobs
            .iter()
            .find(|job| job.id == id)
            .map(|job| job.game_id.clone())
        {
            self.refresh_local_presence(&game_id);
        }
    }

    fn open_search(&mut self) {
        let platform = if self.screen == Screen::Games {
            self.catalog_system
                .clone()
                .or_else(|| self.selected_game().map(|game| game.system.clone()))
        } else {
            None
        };
        self.search = Some(SearchDialog::new(
            if self.screen == Screen::Games {
                self.search_query.clone()
            } else {
                String::new()
            },
            if self.screen == Screen::Games {
                self.catalog_system.clone()
            } else {
                None
            },
            platform,
        ));
    }

    fn menu_action(&mut self, action: Action) -> Result<bool> {
        let items = Menu::items(self.config.auto_refresh);
        let menu = self.menu.as_mut().unwrap();
        menu.selected = menu.selected.min(items.len() - 1);
        match action {
            Action::Up | Action::Down => {
                menu.selected = move_selection(menu.selected, items.len(), action, ViewMode::List)
            }
            Action::Back => self.menu = None,
            Action::Confirm | Action::Left | Action::Right => {
                let item = items[menu.selected];
                match item {
                    _ if action != Action::Confirm => (),
                    MenuItem::Settings => {
                        match settings::SettingsDialog::new(&self.config, self.config_path.clone())
                        {
                            Ok(dialog) => self.settings = Some(dialog),
                            Err(error) => self.status = format!("{error:#}"),
                        }
                    }
                    MenuItem::Downloads => {
                        self.menu = None;
                        if self.screen != Screen::Queue {
                            self.before_queue = self.screen;
                        }
                        self.screen = Screen::Queue;
                        self.focus_current_job();
                    }
                    MenuItem::AdvancedSearch => {
                        self.menu = None;
                        self.open_advanced();
                    }
                    MenuItem::Reload => {
                        self.menu = None;
                        self.reload_catalog();
                    }
                    MenuItem::Refresh => {
                        self.menu = None;
                        self.refresh_frontend();
                    }
                    MenuItem::Quit => {
                        self.menu = None;
                        return self.action(Action::Quit);
                    }
                }
            }
            _ => (),
        }
        Ok(true)
    }

    fn close_settings(&mut self) -> bool {
        let prepared = (|| -> Result<(Config, Api)> {
            let dialog = self.settings.as_mut().unwrap();
            dialog.finish_input()?;
            dialog.draft.validate()?;
            let config = dialog.draft.clone();
            let api = if config.server_url == self.config.server_url {
                self.api.clone()
            } else {
                Api::new(&config.server_url)?
            };
            dialog.save()?;
            Ok((config, api))
        })();
        match prepared {
            Ok((config, api)) => {
                self.settings = None;
                self.apply_config(config, api);
                self.status = self
                    .config
                    .language
                    .format("settings_saved", &[&self.config_path.display()]);
                true
            }
            Err(error) => {
                self.settings.as_mut().unwrap().error = Some(format!("{error:#}"));
                false
            }
        }
    }

    fn apply_config(&mut self, config: Config, api: Api) {
        let server_changed = config.server_url != self.config.server_url;
        let cache_changed = config.cache_dir != self.config.cache_dir;
        let library_changed = config.roms_dir != self.config.roms_dir
            || config.system_mapping != self.config.system_mapping;
        self.ui_dirty |= config.language != self.config.language
            || config.font_path != self.config.font_path
            || config.fullscreen != self.config.fullscreen;
        self.config = config;
        if server_changed || library_changed {
            self.selection.clear();
        }
        if server_changed || cache_changed {
            self.api = api;
            self.gallery_cancel.store(true, Ordering::Relaxed);
            self.generation += 1;
            self.gallery_generation += 1;
            self.preview_generation += 1;
            self.gallery_ids.clear();
            self.preview_requests = ArtworkLoader::new(
                1,
                self.api.clone(),
                self.config.cache_dir.clone(),
                self.tx.clone(),
            );
            self.gallery_requests = ArtworkLoader::new(
                2,
                self.api.clone(),
                self.config.cache_dir.clone(),
                self.tx.clone(),
            );
            self.clear_artwork = true;
        }
        if server_changed {
            self.connection_generation += 1;
            self.catalog_generation += 1;
            self.catalog_pending = None;
            self.reloading = false;
            self.reload_poll_pending = false;
            self.next_reload_poll = Instant::now();
            self.systems.clear();
            self.system_index = 0;
            self.page = None;
            self.detail = None;
            self.game_index = 0;
            self.catalog_system = None;
            self.search_query.clear();
            self.filters.clear();
            if self.screen == Screen::Queue {
                self.before_queue = Screen::Systems;
            } else {
                self.screen = Screen::Systems;
            }
            self.systems();
            return;
        }
        if library_changed {
            self.catalog_generation += 1;
            self.catalog_pending = None;
            for game in self
                .page
                .iter_mut()
                .flat_map(|page| &mut page.items)
                .chain(self.detail.iter_mut())
            {
                game.installed = self.config.rom_exists(&game.system, &game.path);
            }
        }
        let screen = if self.screen == Screen::Queue {
            self.before_queue
        } else {
            self.screen
        };
        if self.loading && screen == Screen::Systems && cache_changed {
            self.systems();
        }
        if self.loading && screen == Screen::Games && library_changed {
            self.games();
        }
        match screen {
            Screen::Systems => self.load_system_thumbnails(),
            Screen::Games => self.load_thumbnails(),
            _ => (),
        }
        self.load_preview();
    }

    fn refresh_frontend(&mut self) {
        if !self.config.auto_refresh || self.frontend_refreshing {
            return;
        }
        if self.jobs.iter().any(|job| !job.done) {
            self.status = self.config.language.text("refresh_busy").into();
            return;
        }
        self.frontend_refreshing = true;
        self.status = self.config.language.text("refresh_running").into();
        let (config, tx) = (self.config.clone(), self.tx.clone());
        thread::spawn(move || {
            let _ = tx.send(Message::FrontendRefreshed(frontend::refresh(&config)));
        });
    }

    fn open_advanced(&mut self) {
        self.advanced_generation += 1;
        self.advanced = Some(advanced::AdvancedDialog::new(
            &self.systems,
            self.catalog_system.clone(),
            &self.search_query,
            &self.filters,
        ));
        let (api, tx, generation) = (self.api.clone(), self.tx.clone(), self.advanced_generation);
        thread::spawn(move || {
            let _ = tx.send(Message::FilterFields(generation, api.filter_fields()));
        });
    }

    fn advanced_outcome(&mut self, outcome: Option<advanced::Outcome>) {
        match outcome {
            Some(advanced::Outcome::Cancel) => self.advanced = None,
            Some(advanced::Outcome::Submit) => {
                if self
                    .advanced
                    .as_ref()
                    .is_some_and(|dialog| dialog.loading || dialog.error.is_some())
                {
                    return;
                }
                let dialog = self.advanced.take().unwrap();
                self.filters = dialog
                    .values
                    .into_iter()
                    .filter(|(_, value)| !value.trim().is_empty())
                    .collect();
                self.search_query.clear();
                self.catalog_system = dialog.scope;
                self.screen = Screen::Games;
                self.games();
            }
            None => (),
        }
    }

    fn alphabet_outcome(&mut self, outcome: Option<AlphabetOutcome>) {
        match outcome {
            Some(AlphabetOutcome::Cancel) => self.alphabet = None,
            Some(AlphabetOutcome::Select(letter)) => {
                self.alphabet = None;
                if self.screen == Screen::Systems {
                    let initial = letter.to_ascii_lowercase();
                    if let Some(index) = self.systems.iter().position(|system| {
                        system
                            .display_name()
                            .chars()
                            .next()
                            .is_some_and(|c| c.to_ascii_lowercase() == initial)
                    }) {
                        self.system_index = index;
                        self.load_system_thumbnails();
                        self.load_preview();
                    } else {
                        self.status = self.config.language.format("alphabet_missing", &[&letter]);
                    }
                } else if self.screen == Screen::Games {
                    // Invalidate any in-flight scroll, but retain the current page
                    // if this initial has no match or the network request fails.
                    self.catalog_generation += 1;
                    self.catalog_pending = None;
                    self.loading = true;
                    self.status = self.config.language.text("loading_catalog").into();
                    let (api, tx, generation, system, query) = (
                        self.api.clone(),
                        self.tx.clone(),
                        self.catalog_generation,
                        self.catalog_system.clone(),
                        self.search_query.clone(),
                    );
                    let config = self.config.clone();
                    let filters = self.filters.clone();
                    thread::spawn(move || {
                        let result = api
                            .games_at_letter_filtered(system.as_deref(), &query, letter, &filters)
                            .map(|found| {
                                found.map(|(mut page, index)| {
                                    for game in &mut page.items {
                                        game.installed =
                                            config.rom_exists(&game.system, &game.path);
                                    }
                                    (page, index)
                                })
                            });
                        let _ = tx.send(Message::Letter(generation, letter, result));
                    });
                }
            }
            None => (),
        }
    }

    fn search_outcome(&mut self, outcome: Option<SearchOutcome>) {
        match outcome {
            Some(SearchOutcome::Cancel) => self.search = None,
            Some(SearchOutcome::Submit) => {
                let search = self.search.take().unwrap();
                self.filters.remove("name");
                self.search_query = search.query.trim().to_owned();
                self.catalog_system = search.scope;
                self.screen = Screen::Games;
                self.page = None;
                self.game_index = 0;
                self.games();
            }
            None => (),
        }
    }
}

fn load_art(api: &Api, file: &romstore_client::api::File, cache: PathBuf) -> Result<RgbaImage> {
    load_art_sized(api, file, cache, (640, 640))
}

fn load_art_sized(api: &Api, file: &File, cache: PathBuf, bounds: (u32, u32)) -> Result<RgbaImage> {
    let bytes = art_bytes(file, cache, || {
        let mut bytes = Vec::new();
        api.file(file)?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    })?;
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader
        .decode()
        .context("Décodage de la jaquette impossible")?
        .thumbnail(bounds.0, bounds.1)
        .into_rgba8())
}

fn art_bytes(
    file: &romstore_client::api::File,
    cache: PathBuf,
    fetch: impl FnOnce() -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    const LIMIT: u64 = 8 * 1024 * 1024;
    romstore_client::api::validate_id(&file.sha256)?;
    if file.size > LIMIT {
        bail!("Jaquette trop volumineuse");
    }
    // Keep launch-directory semantics, but report an absolute path in diagnostics.
    let cache = if cache.is_absolute() {
        cache
    } else {
        std::env::current_dir()?.join(cache)
    };
    let target = cache.join(format!("{}.image", file.sha256));
    let mut bytes = Vec::new();
    match fs::File::open(&target) {
        Ok(stream) => {
            if let Err(error) = stream.take(LIMIT + 1).read_to_end(&mut bytes) {
                eprintln!(
                    "Cache jaquette illisible ({}), téléchargement direct : {error}",
                    target.display()
                );
                bytes.clear();
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => eprintln!(
            "Cache jaquette inaccessible ({}), téléchargement direct : {error}",
            target.display()
        ),
    }
    if bytes.len() as u64 != file.size || hex::encode(Sha256::digest(&bytes)) != file.sha256 {
        bytes = fetch().with_context(|| {
            format!(
                "Téléchargement de la jaquette depuis le serveur (fichier {})",
                file.id
            )
        })?;
        if bytes.len() as u64 != file.size || hex::encode(Sha256::digest(&bytes)) != file.sha256 {
            bail!("Jaquette incomplète ou corrompue");
        }
        let cached = (|| -> Result<()> {
            fs::create_dir_all(&cache)?;
            let mut temporary = tempfile::NamedTempFile::new_in(&cache)?;
            temporary.write_all(&bytes)?;
            temporary.persist(&target)?;
            Ok(())
        })();
        if let Err(error) = cached {
            eprintln!(
                "Cache jaquette non inscriptible ({}), affichage sans cache : {error:#}",
                cache.display()
            );
        }
    }
    Ok(bytes)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut config_path = None;
    let mut install_id = None;
    let mut recovery = None;
    let mut smoke_test = false;
    let mut screenshots = None;
    let mut help = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                config_path = Some(PathBuf::from(
                    args.next().context("--config nécessite un chemin")?,
                ))
            }
            "--install" => {
                install_id = Some(args.next().context("--install nécessite un identifiant")?)
            }
            "--recover" => {
                recovery = Some(PathBuf::from(
                    args.next()
                        .context("--recover nécessite le chemin d'un journal")?,
                ))
            }
            "--smoke-test" => smoke_test = true,
            "--screenshots" => {
                screenshots = Some(PathBuf::from(
                    args.next().context("--screenshots nécessite un dossier")?,
                ));
            }
            "--help" | "-h" => {
                help = true;
            }
            _ => bail!("Option inconnue : {arg}"),
        }
    }
    if help {
        let help_path = config_path.clone().unwrap_or_else(Config::default_path);
        let language = if config_path.is_some() || help_path.exists() {
            Config::load(&help_path)?.language
        } else {
            Language::default()
        };
        println!("{}", language.text("usage"));
        return Ok(());
    }
    if install_id.is_some() && recovery.is_some() {
        bail!("Choisir --install ou --recover");
    }
    if screenshots.is_some() && !smoke_test {
        bail!("--screenshots nécessite --smoke-test");
    }
    if let Some(directory) = &screenshots {
        fs::create_dir_all(directory)?;
    }
    let explicit_config = config_path.is_some();
    let config_path = config_path.unwrap_or_else(Config::default_path);
    let config_path = if config_path.is_absolute() {
        config_path
    } else {
        std::env::current_dir()?.join(config_path)
    };
    let config = if explicit_config || config_path.exists() {
        Config::load(&config_path)?
    } else {
        Config::default()
    };
    let api = Api::new(&config.server_url)?;
    let language = config.language;
    if install_id.is_some() || recovery.is_some() {
        let manifest = if let Some(path) = recovery {
            let bytes = fs::read(path)?;
            if bytes.len() > 16 * 1024 * 1024 {
                bail!("Journal trop volumineux");
            }
            serde_json::from_slice(&bytes)?
        } else {
            api.manifest(&install_id.context("Identifiant manquant")?)?
        };
        install::install_configured(&api, &config, &manifest, &AtomicBool::new(false), |p| {
            let message = if p.completed == p.total {
                language.text("progress_done")
            } else {
                &p.message
            };
            eprintln!(
                "{}",
                language.format("cli_progress", &[&p.completed, &p.total, &message])
            );
        })?;
        frontend::refresh_on_exit(&config);
        return Ok(());
    }
    let sdl = sdl2::init().map_err(|e| anyhow!(e))?;
    let video = sdl.video().map_err(|e| anyhow!(e))?;
    let mut window = video
        .window("ROM Store", 1280, 720)
        .position_centered()
        .resizable()
        .allow_highdpi()
        .build()?;
    if config.fullscreen {
        window
            .set_fullscreen(FullscreenType::Desktop)
            .map_err(|e| anyhow!(e))?;
    }
    let mut canvas = window.into_canvas().build()?;
    canvas.set_logical_size(1280, 720)?;
    let creator = canvas.texture_creator();
    let mut painter = Painter::new(&creator, &config.font_path, language)?;
    let mut input = Input::new(sdl.game_controller().map_err(|e| anyhow!(e))?);
    let mut events = sdl.event_pump().map_err(|e| anyhow!(e))?;
    let (tx, rx) = mpsc::channel();
    let (installer, requests) = mpsc::channel::<InstallRequest>();
    let install_tx = tx.clone();
    thread::spawn(move || {
        for (id, operation, cancel, install_api, install_config) in requests {
            let _ = install_tx.send(Message::Started(id));
            let last = RefCell::new(Instant::now() - Duration::from_secs(1));
            let result = (|| {
                if cancel.load(Ordering::Relaxed) {
                    bail!("Opération annulée");
                }
                let progress = |progress: Progress| {
                    if last.borrow().elapsed() >= Duration::from_millis(100)
                        || progress.completed == progress.total
                    {
                        *last.borrow_mut() = Instant::now();
                        let _ = install_tx.send(Message::Progress(id, progress));
                    }
                };
                match operation {
                    Operation::Uninstall(game) => {
                        uninstall::remove_configured(&install_config, &game, &cancel, progress)
                    }
                    Operation::Install(game) => {
                        let manifest = install_api.manifest(&game)?;
                        install::install_configured(
                            &install_api,
                            &install_config,
                            &manifest,
                            &cancel,
                            progress,
                        )
                    }
                }
            })();
            let _ = install_tx.send(Message::Finished(id, result));
        }
    });
    let preview_requests = ArtworkLoader::new(1, api.clone(), config.cache_dir.clone(), tx.clone());
    let gallery_requests = ArtworkLoader::new(2, api.clone(), config.cache_dir.clone(), tx.clone());
    let mut app = App {
        config,
        config_path,
        settings: None,
        connection_generation: 0,
        ui_dirty: false,
        clear_artwork: false,
        api,
        tx,
        installer,
        generation: 0,
        catalog_generation: 0,
        catalog_pending: None,
        gallery_generation: 0,
        preview_generation: 0,
        preview_requests,
        gallery_requests,
        gallery_cancel: Arc::new(AtomicBool::new(false)),
        gallery_ids: HashSet::new(),
        loading: false,
        reloading: false,
        reload_poll_pending: false,
        next_reload_poll: Instant::now(),
        screen: Screen::Systems,
        before_queue: Screen::Systems,
        systems: Vec::new(),
        system_index: 0,
        page: None,
        game_index: 0,
        detail: None,
        queue_index: 0,
        next_job_id: 0,
        jobs: Vec::new(),
        status: String::new(),
        quit_requested: false,
        quit_confirmation: None,
        selection: selection::Selection::default(),
        catalog_system: None,
        search_query: String::new(),
        search: None,
        alphabet: None,
        menu: None,
        advanced: None,
        advanced_generation: 0,
        filters: BTreeMap::new(),
        frontend_refreshing: false,
        uninstall_confirmation: None,
    };
    if !smoke_test {
        app.systems();
    }
    let mut frames = 0;
    video.text_input().stop();
    let mut text_input_active = false;
    'running: loop {
        if !smoke_test {
            app.poll_reload();
        }
        let wants_text_input = app.search.is_some()
            || app
                .settings
                .as_ref()
                .is_some_and(|dialog| dialog.text_input())
            || app
                .advanced
                .as_ref()
                .is_some_and(|dialog| dialog.text_input());
        if wants_text_input != text_input_active {
            text_input_active = wants_text_input;
            if text_input_active {
                video.text_input().start();
            } else {
                video.text_input().stop();
            }
        }
        for event in events.poll_iter() {
            if let Some(action) = input.catalogue_event(&event, app.catalogue_input_active()) {
                if let Some(action) = action {
                    if action == Action::Confirm && !app.selection.enabled {
                        painter.set_art(None)?;
                    }
                    if !app.action(action)? {
                        break 'running;
                    }
                }
                continue;
            }
            if matches!(
                &event,
                sdl2::event::Event::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Escape),
                    repeat: false,
                    ..
                } | sdl2::event::Event::ControllerButtonDown {
                    button: sdl2::controller::Button::Start,
                    ..
                }
            ) {
                app.action(Action::Menu)?;
                continue;
            }
            if let Some(dialog) = app.settings.as_mut().filter(|dialog| dialog.text_input()) {
                if !matches!(
                    &event,
                    sdl2::event::Event::Quit { .. }
                        | sdl2::event::Event::ControllerDeviceAdded { .. }
                        | sdl2::event::Event::ControllerDeviceRemoved { .. }
                ) {
                    dialog.event(&event);
                    continue;
                }
            }
            if app.menu.is_none() {
                if let Some(dialog) = app.advanced.as_mut().filter(|dialog| dialog.text_input()) {
                    if !matches!(
                        &event,
                        sdl2::event::Event::Quit { .. }
                            | sdl2::event::Event::ControllerDeviceAdded { .. }
                            | sdl2::event::Event::ControllerDeviceRemoved { .. }
                    ) {
                        dialog.event(&event);
                        continue;
                    }
                }
            }
            if let Some(dialog) = &mut app.alphabet {
                if matches!(&event, sdl2::event::Event::KeyDown { keycode: Some(key), .. }
                    if key.name().len() == 1 || *key == sdl2::keyboard::Keycode::Escape || *key == sdl2::keyboard::Keycode::Backspace)
                {
                    let outcome = dialog.event(&event);
                    app.alphabet_outcome(outcome);
                    continue;
                }
            }
            if app.search.is_some()
                && !matches!(
                    &event,
                    sdl2::event::Event::Quit { .. }
                        | sdl2::event::Event::ControllerDeviceAdded { .. }
                        | sdl2::event::Event::ControllerDeviceRemoved { .. }
                )
            {
                let outcome = app.search.as_mut().unwrap().event(&event);
                app.search_outcome(outcome);
                continue;
            }
            if let Some(action) = input.event(event) {
                if action == Action::Confirm
                    && app.screen == Screen::Games
                    && app.uninstall_confirmation.is_none()
                {
                    painter.set_art(None)?;
                }
                if !app.action(action)? {
                    break 'running;
                }
            }
        }
        if let Some(action) = input.poll_direction() {
            app.action(action)?;
        }
        if let Some(action) = input.poll_confirm(app.catalogue_input_active()) {
            app.action(action)?;
        }
        if app.ui_dirty {
            let updated = (|| -> Result<()> {
                painter.reconfigure(&app.config.font_path, app.config.language)?;
                canvas
                    .window_mut()
                    .set_fullscreen(if app.config.fullscreen {
                        FullscreenType::Desktop
                    } else {
                        FullscreenType::Off
                    })
                    .map_err(|error| anyhow!(error))?;
                Ok(())
            })();
            if let Err(error) = updated {
                app.status = app
                    .config
                    .language
                    .format("settings_display_failed", &[&format!("{error:#}")]);
            }
            app.ui_dirty = false;
        }
        if app.clear_artwork {
            painter.clear_media();
            app.clear_artwork = false;
        }
        for message in rx.try_iter().take(100) {
            app.message(message, &mut painter)?;
        }
        if app.quit_requested && app.jobs.iter().all(|job| job.done) && !app.frontend_refreshing {
            break;
        }
        let scene = if smoke_test {
            Some(smoke::prepare(&mut app, &mut painter, frames)?)
        } else {
            None
        };
        app.draw(&mut canvas, &mut painter)?;
        if let (Some(directory), Some(scene)) = (&screenshots, scene) {
            let (width, height) = canvas.output_size().map_err(|e| anyhow!(e))?;
            let pixels = canvas
                .read_pixels(None, sdl2::pixels::PixelFormatEnum::RGBA32)
                .map_err(|e| anyhow!(e))?;
            image::save_buffer(
                directory.join(format!("{frames:02}-{scene}.png")),
                &pixels,
                width,
                height,
                image::ColorType::Rgba8,
            )?;
        }
        canvas.present();
        frames += 1;
        if smoke_test && frames >= smoke::SCENES {
            break;
        }
        thread::sleep(Duration::from_millis(16));
    }
    for job in &app.jobs {
        job.cancel.store(true, Ordering::Relaxed);
    }
    app.gallery_cancel.store(true, Ordering::Relaxed);
    let refresh_config = app.config.clone();
    // Release the display and controllers before asking the frontend to refresh.
    // Normal quit has already waited for every local installation/removal job.
    drop(app);
    drop(painter);
    drop(creator);
    drop(canvas);
    drop(events);
    drop(input);
    drop(video);
    drop(sdl);
    if !smoke_test {
        frontend::refresh_on_exit(&refresh_config);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art_file(bytes: &[u8]) -> romstore_client::api::File {
        romstore_client::api::File {
            id: "a".repeat(64),
            role: "image".into(),
            destination: "images/demo.ppm".into(),
            size: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
        }
    }

    #[test]
    fn unavailable_art_cache_does_not_block_display() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("not-a-directory");
        fs::write(&cache, b"file").unwrap();
        let bytes = b"verified image";
        assert_eq!(
            art_bytes(&art_file(bytes), cache, || Ok(bytes.to_vec())).unwrap(),
            bytes
        );
    }

    #[test]
    fn art_cache_is_verified_and_reused() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"verified image";
        let file = art_file(bytes);
        let target = root.path().join(format!("{}.image", file.sha256));
        fs::write(&target, b"corrupt").unwrap();
        assert_eq!(
            art_bytes(&file, root.path().into(), || Ok(bytes.to_vec())).unwrap(),
            bytes
        );
        assert_eq!(fs::read(target).unwrap(), bytes);
        assert_eq!(
            art_bytes(&file, root.path().into(), || panic!(
                "cache should be reused"
            ))
            .unwrap(),
            bytes
        );
    }

    #[test]
    fn direct_art_download_still_requires_valid_checksum() {
        let root = tempfile::tempdir().unwrap();
        assert!(art_bytes(&art_file(b"image"), root.path().into(), || Ok(
            b"wrong".to_vec()
        ))
        .is_err());
    }

    fn app() -> App {
        let (tx, _) = mpsc::channel();
        let (installer, _) = mpsc::channel();
        let api = Api::new("http://127.0.0.1:1").unwrap();
        let cache = Config::default().cache_dir;
        let preview_requests = ArtworkLoader::new(0, api.clone(), cache.clone(), tx.clone());
        let gallery_requests = ArtworkLoader::new(0, api.clone(), cache, tx.clone());
        App {
            config: Config::default(),
            config_path: PathBuf::from("/nonexistent-romstore-test/config.yaml"),
            settings: None,
            connection_generation: 0,
            ui_dirty: false,
            clear_artwork: false,
            api,
            tx,
            installer,
            generation: 0,
            catalog_generation: 0,
            catalog_pending: None,
            gallery_generation: 0,
            preview_generation: 0,
            preview_requests,
            gallery_requests,
            gallery_cancel: Arc::new(AtomicBool::new(false)),
            gallery_ids: HashSet::new(),
            loading: false,
            reloading: false,
            reload_poll_pending: false,
            next_reload_poll: Instant::now(),
            screen: Screen::Systems,
            before_queue: Screen::Systems,
            systems: Vec::new(),
            system_index: 0,
            page: None,
            game_index: 0,
            detail: None,
            queue_index: 0,
            next_job_id: 0,
            jobs: Vec::new(),
            status: String::new(),
            quit_requested: false,
            quit_confirmation: None,
            selection: selection::Selection::default(),
            catalog_system: None,
            search_query: String::new(),
            search: None,
            alphabet: None,
            menu: None,
            advanced: None,
            advanced_generation: 0,
            filters: BTreeMap::new(),
            frontend_refreshing: false,
            uninstall_confirmation: None,
        }
    }

    #[test]
    fn queue_returns_to_previous_screen() {
        let mut app = app();
        app.screen = Screen::Detail;
        choose_menu(&mut app, MenuItem::Downloads);
        assert_eq!(app.screen, Screen::Queue);
        app.action(Action::Back).unwrap();
        assert_eq!(app.screen, Screen::Detail);
    }

    #[test]
    fn back_never_quits_and_menu_owns_quit_and_independent_view_settings() {
        let mut app = app();
        let directory = tempfile::tempdir().unwrap();
        app.config_path = directory.path().join("config.yaml");
        assert!(app.action(Action::Back).unwrap());
        assert!(!app.quit_requested);
        app.action(Action::Menu).unwrap();
        assert!(app.menu.is_some());
        app.action(Action::Confirm).unwrap();
        assert!(app.settings.is_some());
        app.action(Action::Right).unwrap();
        assert_eq!(
            app.settings.as_ref().unwrap().draft.systems_view,
            ViewMode::Grid
        );
        assert_eq!(app.config.systems_view, ViewMode::List);
        assert_eq!(app.config.games_view, ViewMode::Grid);
        app.action(Action::Down).unwrap();
        app.action(Action::Confirm).unwrap();
        assert_eq!(
            app.settings.as_ref().unwrap().draft.games_view,
            ViewMode::List
        );
        app.action(Action::Back).unwrap();
        assert!(app.settings.is_none());
        assert_eq!(app.config.systems_view, ViewMode::Grid);
        assert_eq!(app.config.games_view, ViewMode::List);
        assert_eq!(Config::load(&app.config_path).unwrap(), app.config);
        app.action(Action::Back).unwrap();
        assert!(app.menu.is_none());
        assert!(app.action(Action::Back).unwrap());
        assert!(!choose_menu(&mut app, MenuItem::Quit));
    }

    #[test]
    fn failed_settings_save_keeps_submenu_open_without_applying_partial_values() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = app();
        app.config_path = directory.path().join("blocked/config.yaml");
        choose_menu(&mut app, MenuItem::Settings);
        app.settings.as_mut().unwrap().draft.games_view = ViewMode::List;
        fs::write(directory.path().join("blocked"), b"not a directory").unwrap();
        app.action(Action::Menu).unwrap();
        assert!(app.settings.is_some());
        assert!(app.settings.as_ref().unwrap().error.is_some());
        assert_eq!(app.config.games_view, ViewMode::Grid);
        assert_eq!(
            fs::read(directory.path().join("blocked")).unwrap(),
            b"not a directory"
        );
    }

    #[test]
    fn quitting_from_settings_saves_all_fields_first() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = app();
        app.config_path = directory.path().join("config.yaml");
        choose_menu(&mut app, MenuItem::Settings);
        let draft = &mut app.settings.as_mut().unwrap().draft;
        draft.language = Language::En;
        draft.fullscreen = true;
        draft.system_mapping.insert("gamecube".into(), "gc".into());
        assert!(!app.action(Action::Quit).unwrap());
        assert_eq!(Config::load(&app.config_path).unwrap(), app.config);
        assert!(app.ui_dirty);
    }

    #[test]
    fn queued_operations_keep_their_config_snapshot_when_server_or_paths_change() {
        let mut app = app();
        let (installer, requests) = mpsc::channel();
        app.installer = installer;
        let old_config = app.config.clone();
        app.enqueue_job(smoke_page().items.remove(0), JobKind::Install)
            .unwrap();
        let config = Config {
            server_url: "http://127.0.0.1:2".into(),
            roms_dir: "/new-roms".into(),
            ..old_config.clone()
        };
        let generation = app.connection_generation;
        app.apply_config(config.clone(), Api::new(&config.server_url).unwrap());
        app.enqueue_job(smoke_page().items.remove(1), JobKind::Install)
            .unwrap();
        let (_, _, _, _, first) = requests.try_recv().unwrap();
        let (_, _, _, _, second) = requests.try_recv().unwrap();
        assert_eq!(first, old_config);
        assert_eq!(second, config);
        assert_eq!(app.connection_generation, generation + 1);
        assert!(app.clear_artwork);
        assert_eq!(app.screen, Screen::Systems);
    }

    #[test]
    fn changing_rom_paths_restarts_initial_batches_and_rejects_old_presence_results() {
        let mut app = app();
        app.screen = Screen::Games;
        app.loading = true;
        app.catalog_pending = Some(0);
        let generation = app.catalog_generation;
        let config = Config {
            roms_dir: "/new-roms".into(),
            ..app.config.clone()
        };
        app.apply_config(config.clone(), Api::new(&config.server_url).unwrap());
        assert!(app.catalog_generation > generation);
        assert_eq!(app.catalog_pending, Some(0));
        assert!(!app.receive_games(generation, 0, Ok(smoke_page())));
        assert!(app.page.is_none());
    }

    #[test]
    fn manual_refresh_is_conditional_and_cannot_overlap_local_operations() {
        let mut app = app();
        assert!(!Menu::items(false).contains(&MenuItem::Refresh));
        app.refresh_frontend();
        assert!(!app.frontend_refreshing);
        app.config.auto_refresh = true;
        assert!(Menu::items(true).contains(&MenuItem::Refresh));
        let (installer, _requests) = mpsc::channel();
        app.installer = installer;
        app.enqueue_job(smoke_page().items.remove(0), JobKind::Install)
            .unwrap();
        app.refresh_frontend();
        assert!(!app.frontend_refreshing);
        assert_eq!(app.status, app.config.language.text("refresh_busy"));
        app.frontend_refreshing = true;
        app.enqueue_job(smoke_page().items.remove(1), JobKind::Install)
            .unwrap();
        assert_eq!(app.jobs.len(), 1);
        app.jobs[0].done = true;
        assert!(app.action(Action::Quit).unwrap());
        assert!(app.quit_confirmation.is_some());
        assert!(!app.quit_requested);
        app.action(Action::Right).unwrap();
        app.action(Action::Confirm).unwrap();
        assert!(app.quit_requested);
        app.frontend_refreshing = false;
        assert!(!app.action(Action::Quit).unwrap());
    }

    #[test]
    fn advanced_filters_apply_together_survive_reload_and_cancel_does_not_change_results() {
        let mut app = app();
        choose_menu(&mut app, MenuItem::AdvancedSearch);
        let dialog = app.advanced.as_mut().unwrap();
        dialog.set_fields(vec!["custom".into()]);
        dialog.scope = Some("nes".into());
        dialog.values.insert("name".into(), "=Mario".into());
        dialog.values.insert("year".into(), ">=1990".into());
        dialog.values.insert("players".into(), "2".into());
        app.advanced_outcome(Some(advanced::Outcome::Submit));
        assert_eq!(app.catalog_system.as_deref(), Some("nes"));
        assert!(app.search_query.is_empty());
        assert_eq!(app.filters["name"], "=Mario");
        let filters = app.filters.clone();
        app.open_advanced();
        app.advanced.as_mut().unwrap().values.clear();
        app.advanced_outcome(Some(advanced::Outcome::Cancel));
        assert_eq!(app.filters, filters);
        app.finish_reload(Ok((reload_state(), vec![nes()])));
        assert_eq!(app.filters, filters);
        assert_eq!(app.catalog_system.as_deref(), Some("nes"));
    }

    fn choose_menu(app: &mut App, item: MenuItem) -> bool {
        app.action(Action::Menu).unwrap();
        app.menu.as_mut().unwrap().selected = Menu::items(app.config.auto_refresh)
            .iter()
            .position(|candidate| *candidate == item)
            .unwrap();
        let running = app.action(Action::Confirm).unwrap();
        if app.settings.is_none() {
            app.menu = None;
        }
        running
    }

    fn configure_view(app: &mut App, systems: bool) {
        let directory = tempfile::tempdir().unwrap();
        let old_path =
            std::mem::replace(&mut app.config_path, directory.path().join("config.yaml"));
        choose_menu(app, MenuItem::Settings);
        let draft = &mut app.settings.as_mut().unwrap().draft;
        if systems {
            draft.systems_view = draft.systems_view.toggled();
        } else {
            draft.games_view = draft.games_view.toggled();
        }
        app.action(Action::Menu).unwrap();
        assert!(app.settings.is_none());
        app.menu = None;
        app.config_path = old_path;
    }

    fn reload_state() -> ReloadStatus {
        ReloadStatus {
            status: "completed".into(),
            systems: 1,
            games: 12,
            diagnostics: 0,
            error: None,
        }
    }

    fn nes() -> System {
        System {
            id: "nes".into(),
            name: "NES".into(),
            game_count: 12,
            image: None,
        }
    }

    #[test]
    fn reload_is_available_from_catalogue_and_platforms_without_blocking_navigation() {
        let mut app = app();
        app.systems = vec![nes()];
        choose_menu(&mut app, MenuItem::Reload);
        assert!(app.reloading);
        choose_menu(&mut app, MenuItem::Reload);
        assert!(app.reloading);
        assert!(!app.loading);
        app.screen = Screen::Games;
        app.page = Some(smoke_page());
        app.action(Action::Right).unwrap();
        assert_eq!(app.game_index, 1);
        app.finish_reload(Err(anyhow!("offline")));
        assert!(!app.reloading);
        assert_eq!(app.page.as_ref().unwrap().items.len(), 12);
        assert_eq!(app.game_index, 1);
        assert!(app.status.contains("offline"));
    }

    #[test]
    fn server_indexation_is_detected_without_post_and_survives_transient_errors() {
        let mut app = app();
        let mut state = reload_state();
        state.status = "running".into();
        app.receive_reload_status(Ok(state));
        assert!(app.reloading);
        assert!(!app.reload_poll_pending);
        app.reload_catalog();
        assert!(
            !app.reload_poll_pending,
            "No POST while the server is indexing"
        );
        app.receive_reload_status(Err(anyhow!("offline")));
        assert!(app.reloading);
        assert!(!app.reload_poll_pending);
        let mut state = reload_state();
        state.status = "failed".into();
        state.error = Some("Scan failed".into());
        app.receive_reload_status(Ok(state));
        assert!(!app.reloading);
        assert!(app.status.contains("Scan failed"));
    }

    #[test]
    fn completed_server_indexation_triggers_one_background_catalogue_refresh() {
        let mut app = app();
        app.receive_reload_status(Ok(reload_state()));
        assert!(
            !app.reload_poll_pending,
            "Ignore jobs finished before connecting"
        );
        let mut state = reload_state();
        state.status = "running".into();
        app.receive_reload_status(Ok(state));
        app.receive_reload_status(Ok(reload_state()));
        assert!(app.reloading);
        assert!(app.reload_poll_pending);
        app.poll_reload();
        assert!(app.reload_poll_pending);
        app.finish_reload(Ok((reload_state(), vec![nes()])));
        assert!(!app.reloading);
        assert!(!app.reload_poll_pending);
        assert_eq!(app.systems[0].id, "nes");
    }

    #[test]
    fn reload_refreshes_active_search_and_rejects_old_scroll_results_even_in_queue() {
        let mut app = app();
        app.systems = vec![nes()];
        app.screen = Screen::Queue;
        app.before_queue = Screen::Detail;
        app.page = Some(batch(0, 50, 100));
        app.detail = Some(smoke_page().items.remove(0));
        app.catalog_system = Some("nes".into());
        app.search_query = "Mario".into();
        app.catalog_pending = Some(50);
        app.reloading = true;
        let generation = app.catalog_generation;
        app.finish_reload(Ok((reload_state(), vec![nes()])));
        assert!(!app.reloading);
        assert_eq!(app.screen, Screen::Queue);
        assert_eq!(app.before_queue, Screen::Games);
        assert_eq!(app.catalog_system.as_deref(), Some("nes"));
        assert_eq!(app.search_query, "Mario");
        assert_eq!(app.catalog_pending, Some(0));
        assert!(app.page.is_none());
        assert!(app.detail.is_none());
        assert!(!app.receive_games(generation, 50, Ok(batch(50, 50, 100))));
    }

    #[test]
    fn reload_returns_to_platforms_if_selected_system_was_removed() {
        let mut app = app();
        app.systems = vec![nes()];
        app.screen = Screen::Games;
        app.page = Some(smoke_page());
        app.catalog_system = Some("nes".into());
        app.finish_reload(Ok((reload_state(), Vec::new())));
        assert_eq!(app.screen, Screen::Systems);
        assert!(app.page.is_none());
        assert!(app.catalog_system.is_none());
        assert!(!app.loading);
    }

    #[test]
    fn closing_waits_for_installation_writes() {
        let mut app = app();
        let cancel = Arc::new(AtomicBool::new(false));
        app.jobs.push(Job {
            id: 0,
            kind: JobKind::Install,
            game_id: "a".repeat(64),
            name: "Demo".into(),
            cancel: cancel.clone(),
            state: "En cours".into(),
            done: false,
            succeeded: false,
            started: None,
            progress: None,
        });
        assert!(app.action(Action::Quit).unwrap());
        assert!(app.quit_confirmation.is_some());
        assert!(!app.quit_requested);
        assert!(!cancel.load(Ordering::Relaxed));
        app.action(Action::Right).unwrap();
        app.action(Action::Confirm).unwrap();
        assert!(app.quit_requested);
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(app.screen, Screen::Queue);
        app.jobs[0].done = true;
        assert!(!app.action(Action::Quit).unwrap());
    }

    #[test]
    fn multi_selection_only_queues_on_explicit_batch_action_in_both_views() {
        for view in [ViewMode::Grid, ViewMode::List] {
            let mut app = app();
            app.screen = Screen::Games;
            app.config.games_view = view;
            app.page = Some(smoke_page());
            let (installer, requests) = mpsc::channel();
            app.installer = installer;
            app.action(Action::Select).unwrap();
            assert!(app.selection.enabled);
            assert_eq!(app.screen, Screen::Games);
            assert_eq!(app.selection.games.len(), 1);
            app.action(Action::Confirm).unwrap();
            assert!(app.selection.games.is_empty());
            app.action(Action::Confirm).unwrap();
            app.game_index = 1;
            app.action(Action::Confirm).unwrap();
            assert_eq!(app.selection.games.len(), 2);
            app.game_index = 2; // Unsupported game: never select or uninstall it.
            app.action(Action::Confirm).unwrap();
            assert_eq!(app.selection.games.len(), 2);
            assert!(app.jobs.is_empty());
            assert!(requests.try_recv().is_err());
            app.action(Action::DownloadSelection).unwrap();
            assert_eq!(app.jobs.len(), 2);
            assert!(!app.selection.enabled);
            for index in 0..2 {
                assert!(
                    matches!(requests.try_recv().unwrap().1, Operation::Install(id) if id == format!("{index:064x}"))
                );
            }
            assert!(requests.try_recv().is_err());
        }
    }

    #[test]
    fn batch_rechecks_local_presence_and_duplicate_jobs_and_retains_failed_sends() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("nes")).unwrap();
        let mut app = app();
        app.config.roms_dir = root.path().into();
        app.screen = Screen::Games;
        app.page = Some(smoke_page());
        let (installer, requests) = mpsc::channel();
        app.installer = installer;
        app.action(Action::Select).unwrap();
        app.game_index = 1;
        app.action(Action::Confirm).unwrap();
        fs::write(root.path().join("nes/demo0.nes"), b"installed elsewhere").unwrap();
        app.enqueue_job(smoke_page().items.remove(1), JobKind::Install)
            .unwrap();
        app.action(Action::DownloadSelection).unwrap();
        assert_eq!(app.jobs.len(), 1);
        assert!(requests.try_recv().is_ok());
        assert!(requests.try_recv().is_err());
        fs::remove_file(root.path().join("nes/demo0.nes")).unwrap();
        app.game_index = 0;
        app.action(Action::Select).unwrap();
        drop(requests);
        assert!(app.action(Action::DownloadSelection).unwrap());
        assert_eq!(
            app.jobs.len(),
            1,
            "A disconnected worker must not leave a phantom job"
        );
        assert_eq!(app.selection.games.len(), 1);
        assert!(app.selection.enabled);
        app.action(Action::Back).unwrap();
        assert!(!app.selection.enabled);
        assert_eq!(app.screen, Screen::Games);
    }

    #[test]
    fn quitting_defaults_to_continue_and_never_cancels_before_explicit_confirmation() {
        let mut app = app();
        let (installer, _requests) = mpsc::channel();
        app.installer = installer;
        app.enqueue_job(smoke_page().items.remove(0), JobKind::Install)
            .unwrap();
        for cancel_action in [Action::Confirm, Action::Back, Action::Menu] {
            app.action(Action::Quit).unwrap();
            assert!(!app.quit_confirmation.as_ref().unwrap().confirmed);
            app.action(Action::Quit).unwrap(); // Repeated window-close is not consent.
            assert!(!app.quit_requested);
            assert!(!app.jobs[0].cancel.load(Ordering::Relaxed));
            app.action(cancel_action).unwrap();
            assert!(app.quit_confirmation.is_none());
            assert!(!app.jobs[0].cancel.load(Ordering::Relaxed));
        }
        app.action(Action::Quit).unwrap();
        app.jobs[0].done = true; // Completion while the question is open is safe too.
        app.action(Action::Right).unwrap();
        assert!(!app.action(Action::Confirm).unwrap());
    }

    #[test]
    fn lists_grids_and_menus_wrap_at_both_ends() {
        for len in [0_usize, 1, 2, 5, 12] {
            let last = len.saturating_sub(1);
            assert_eq!(move_selection(0, len, Action::Up, ViewMode::List), last);
            assert_eq!(move_selection(last, len, Action::Down, ViewMode::List), 0);
        }
        assert_eq!(move_selection(2, 12, Action::Up, ViewMode::Grid), 11);
        assert_eq!(move_selection(11, 12, Action::Down, ViewMode::Grid), 1);
        assert_eq!(move_selection(0, 12, Action::Left, ViewMode::Grid), 11);
        assert_eq!(move_selection(11, 12, Action::Right, ViewMode::Grid), 0);
        let mut app = app();
        app.menu = Some(Menu::default());
        app.action(Action::Up).unwrap();
        assert_eq!(
            app.menu.as_ref().unwrap().selected,
            Menu::items(false).len() - 1
        );
        app.action(Action::Down).unwrap();
        assert_eq!(app.menu.as_ref().unwrap().selected, 0);
        app.menu = None;
        app.config.systems_view = ViewMode::List;
        app.systems = vec![nes(), nes()];
        app.action(Action::Up).unwrap();
        assert_eq!(app.system_index, 1);
        app.action(Action::Down).unwrap();
        assert_eq!(app.system_index, 0);
    }

    #[test]
    fn catalogue_wrap_fetches_the_real_last_page_and_rejects_stale_results() {
        let mut app = app();
        app.screen = Screen::Games;
        app.config.games_view = ViewMode::List;
        app.page = Some(batch(0, 50, 123));
        app.selection.toggle(smoke_page().items.remove(0));
        app.catalog_pending = Some(50);
        app.action(Action::Up).unwrap();
        assert!(app.loading);
        assert!(app.catalog_pending.is_none());
        assert!(!app.receive_games(0, 50, Ok(batch(50, 50, 123))));
        assert!(!app.receive_catalogue_jump(0, 122, Ok(batch(100, 23, 123))));
        assert!(app.receive_catalogue_jump(1, 122, Ok(batch(100, 23, 123))));
        assert_eq!(app.game_index, 22);
        assert_eq!(app.page.as_ref().unwrap().offset, 100);
        assert_eq!(app.selection.games.len(), 1);
        app.action(Action::Down).unwrap();
        assert!(app.loading);
        assert!(app.receive_catalogue_jump(2, 0, Ok(batch(0, 50, 123))));
        assert_eq!(app.game_index, 0);
        assert_eq!(app.page.as_ref().unwrap().offset, 0);
        assert!(!app.receive_catalogue_jump(2, 122, Err(anyhow!("offline"))));
        assert!(!app.loading);
        assert_eq!(app.page.as_ref().unwrap().offset, 0);
    }

    #[test]
    fn queue_focus_cleanup_and_worker_ids_survive_history_removal() {
        let mut app = app();
        let (installer, requests) = mpsc::channel();
        app.installer = installer;
        for index in [0, 1, 3, 4] {
            app.enqueue_job(smoke_page().items.remove(index), JobKind::Install)
                .unwrap();
        }
        let ids: Vec<_> = requests.try_iter().map(|request| request.0).collect();
        assert_eq!(ids, [0, 1, 2, 3]);
        app.finish_job(0, Ok(()));
        app.finish_job(1, Err(anyhow!("failed")));
        app.jobs[2].started = Some(Instant::now());
        choose_menu(&mut app, MenuItem::Downloads);
        assert_eq!(app.queue_index, 2);
        app.action(Action::DownloadSelection).unwrap();
        assert_eq!(
            app.jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(app.queue_index, 0);
        assert!(app
            .jobs
            .iter()
            .all(|job| !job.cancel.load(Ordering::Relaxed)));
        app.action(Action::Up).unwrap();
        assert_eq!(app.queue_index, 1);
        app.action(Action::Down).unwrap();
        assert_eq!(app.queue_index, 0);
        app.finish_job(0, Ok(())); // Late message for deleted history is ignored.
        assert!(app.jobs.iter().all(|job| !job.done));
        app.finish_job(2, Ok(()));
        assert!(app.jobs[0].succeeded);
        assert!(!app.jobs[1].done);
        app.action(Action::DownloadSelection).unwrap();
        assert_eq!(app.jobs[0].id, 3);
        assert_eq!(app.queue_index, 0);
        app.finish_job(3, Ok(()));
        app.action(Action::DownloadSelection).unwrap();
        assert!(app.jobs.is_empty());
        assert_eq!(app.queue_index, 0);
        app.action(Action::DownloadSelection).unwrap();
        app.enqueue_job(smoke_page().items.remove(0), JobKind::Install)
            .unwrap();
        assert_eq!(requests.try_recv().unwrap().0, 4);
        assert_eq!(app.jobs[0].id, 4);
        app.focus_current_job(); // Waiting task is selected when none is running.
        assert_eq!(app.queue_index, 0);
    }

    #[test]
    fn empty_navigation_is_safe() {
        let mut app = app();
        assert!(app.action(Action::Up).unwrap());
        assert!(app.action(Action::Down).unwrap());
        assert_eq!(app.system_index, 0);
        assert!(app.action(Action::Back).unwrap());
    }

    #[test]
    fn grid_navigation_moves_by_column_and_scrolls_by_row() {
        let mut app = app();
        app.screen = Screen::Games;
        app.page = Some(smoke_page());
        app.action(Action::Right).unwrap();
        assert_eq!(app.game_index, 1);
        app.action(Action::Up).unwrap();
        assert_eq!(app.game_index, 11);
        app.action(Action::Down).unwrap();
        assert_eq!(app.game_index, 1);
        app.action(Action::Down).unwrap();
        assert_eq!(app.game_index, 6);
        assert_eq!(grid_start(app.game_index), 0);
        app.action(Action::Down).unwrap();
        assert_eq!(app.game_index, 11);
        assert_eq!(grid_start(app.game_index), 5);
        app.action(Action::Right).unwrap();
        assert_eq!(app.game_index, 0);
        app.action(Action::Down).unwrap();
        assert_eq!(app.game_index, 5);
        app.action(Action::Up).unwrap();
        assert_eq!(app.game_index, 0);
        app.game_index = 9;
        app.action(Action::Down).unwrap();
        assert_eq!(app.game_index, 11);
    }

    #[test]
    fn empty_grid_and_end_of_catalogue_navigation_are_safe() {
        let mut app = app();
        app.screen = Screen::Games;
        app.page = Some(Page {
            items: Vec::new(),
            total: 0,
            offset: 0,
            limit: 50,
        });
        for action in [Action::Up, Action::Down, Action::Left, Action::Right] {
            app.action(action).unwrap();
        }
        assert_eq!(app.game_index, 0);
        assert!(!app.loading);
    }

    fn batch(offset: usize, count: usize, total: usize) -> Page {
        Page {
            items: (offset..offset + count)
                .map(|index| {
                    let mut game = smoke_page().items.remove(0);
                    game.id = format!("{index:064x}");
                    game.path = format!("demo{index}.nes");
                    game
                })
                .collect(),
            total,
            offset,
            limit: 50,
        }
    }

    #[test]
    fn infinite_scroll_appends_without_resetting_selection_or_blocking_navigation() {
        for view in [ViewMode::Grid, ViewMode::List] {
            let mut app = app();
            app.screen = Screen::Games;
            app.config.games_view = view;
            app.catalog_pending = Some(0);
            assert!(app.receive_games(0, 0, Ok(batch(0, 50, 105))));
            app.game_index = 30;
            app.load_more();
            assert_eq!(app.catalog_pending, Some(50));
            assert!(!app.loading);
            app.action(Action::Down).unwrap();
            assert!(app.game_index > 30);
            app.load_more();
            assert_eq!(
                app.catalog_pending,
                Some(50),
                "Only one request may be in flight"
            );
            let selected = app.game_index;
            app.open_detail();
            assert!(app.receive_games(0, 50, Ok(batch(50, 50, 105))));
            assert_eq!(app.screen, Screen::Detail);
            app.action(Action::Back).unwrap();
            assert_eq!(app.game_index, selected);
            assert_eq!(app.page.as_ref().unwrap().items.len(), 100);
            app.game_index = 99;
            app.load_more();
            assert!(app.receive_games(0, 100, Ok(batch(100, 5, 105))));
            app.action(Action::Down).unwrap();
            assert!(
                app.game_index >= 100,
                "Navigation crosses the old page boundary"
            );
            app.game_index = 104;
            app.load_more();
            assert!(app.catalog_pending.is_none());
            let ids: HashSet<_> = app
                .page
                .as_ref()
                .unwrap()
                .items
                .iter()
                .map(|g| &g.id)
                .collect();
            assert_eq!(ids.len(), 105);
        }
    }

    #[test]
    fn scrolling_after_a_letter_jump_prepends_and_appends_without_losing_selection() {
        let mut app = app();
        app.screen = Screen::Games;
        app.page = Some(batch(100, 50, 230));
        app.game_index = 0;
        app.load_more();
        assert_eq!(app.catalog_pending, Some(50));
        assert!(app.receive_games(0, 50, Ok(batch(50, 50, 230))));
        assert_eq!(app.page.as_ref().unwrap().offset, 50);
        assert_eq!(app.game_index, 50);
        assert_eq!(app.selected_game().unwrap().id, format!("{:064x}", 100));
        app.game_index = 99;
        app.load_more();
        assert_eq!(app.catalog_pending, Some(150));
        assert!(app.receive_games(0, 150, Ok(batch(150, 50, 230))));
        assert_eq!(app.game_index, 99);
        app.game_index = 0;
        app.load_more();
        assert_eq!(app.catalog_pending, Some(0));
        assert!(app.receive_games(0, 0, Ok(batch(0, 50, 230))));
        assert_eq!(app.page.as_ref().unwrap().offset, 0);
        assert_eq!(app.page.as_ref().unwrap().items.len(), 200);
        assert_eq!(app.game_index, 50);
        assert_eq!(app.selected_game().unwrap().id, format!("{:064x}", 50));
    }

    #[test]
    fn platform_letter_jump_uses_display_names_and_missing_letters_keep_selection() {
        let mut app = app();
        app.systems = vec![
            nes(),
            System {
                id: "psx".into(),
                name: "PlayStation".into(),
                ..nes()
            },
        ];
        app.action(Action::Alphabet).unwrap();
        assert!(app.alphabet.is_some());
        app.alphabet_outcome(Some(AlphabetOutcome::Select('S')));
        assert_eq!(app.system_index, 1);
        assert!(app.alphabet.is_none());
        app.alphabet_outcome(Some(AlphabetOutcome::Select('Z')));
        assert_eq!(app.system_index, 1);
        assert!(app.status.contains('Z'));
        app.action(Action::Alphabet).unwrap();
        app.action(Action::Back).unwrap();
        assert!(app.alphabet.is_none());
        assert_eq!(app.screen, Screen::Systems);
    }

    #[test]
    fn infinite_scroll_ignores_stale_results_and_retries_failed_batches() {
        let mut app = app();
        app.screen = Screen::Games;
        app.page = Some(batch(0, 50, 200));
        app.game_index = 49;
        app.load_more();
        assert!(!app.receive_games(99, 50, Ok(batch(50, 50, 200))));
        assert_eq!(app.catalog_pending, Some(50));
        assert!(!app.receive_games(0, 0, Ok(batch(0, 50, 200))));
        assert!(!app.receive_games(0, 50, Err(anyhow!("offline"))));
        assert!(app.catalog_pending.is_none());
        assert_eq!(app.page.as_ref().unwrap().items.len(), 50);
        assert_eq!(app.game_index, 49);
        app.action(Action::Right).unwrap();
        assert_eq!(app.catalog_pending, Some(50));
        assert!(app.receive_games(0, 50, Ok(batch(50, 0, 200))));
        app.load_more();
        assert!(
            app.catalog_pending.is_none(),
            "An empty batch must not trigger an endless request loop"
        );
        app.action(Action::Back).unwrap();
        assert!(!app.receive_games(0, 50, Ok(batch(50, 50, 200))));
    }

    #[test]
    fn gallery_requests_stay_bounded_during_long_scrolls() {
        let mut app = app();
        let mut page = batch(0, 1000, 1000);
        for game in &mut page.items {
            game.files.push(File {
                role: "thumbnail".into(),
                ..art_file(b"cover")
            });
        }
        app.page = Some(page);
        for view in [ViewMode::Grid, ViewMode::List] {
            app.config.games_view = view;
            for selected in [0, 49, 50, 500, 999, 0] {
                app.game_index = selected;
                app.load_thumbnails();
                assert!(app.gallery_ids.len() <= 20);
                assert!(app.gallery_ids.contains(&format!("{selected:064x}")));
            }
        }
    }

    #[test]
    fn catalogue_worker_detects_existing_local_roms_with_system_mapping() {
        use std::{
            io::{BufRead, BufReader},
            net::TcpListener,
        };
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("famicom")).unwrap();
        fs::write(root.path().join("famicom/demo0.nes"), b"already installed").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            let body = r#"{"items":[{"id":"a","system":"nes","path":"demo0.nes","name":"Local","metadata":{},"installable":true,"files":[]},{"id":"b","system":"nes","path":"missing.nes","name":"Missing","metadata":{},"installable":true,"files":[]}],"total":2,"offset":0,"limit":50}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        app.tx = tx;
        app.api = Api::new(&format!("http://{address}")).unwrap();
        app.config.roms_dir = root.path().into();
        app.config
            .system_mapping
            .insert("nes".into(), "famicom".into());
        app.games();
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Message::Games(generation, offset, result) => {
                assert!(app.receive_games(generation, offset, result));
                let page = app.page.unwrap();
                assert!(page.items[0].installed);
                assert!(!page.items[1].installed);
            }
            _ => panic!("Expected a catalogue batch"),
        }
        server.join().unwrap();
    }

    #[test]
    fn local_presence_refresh_updates_catalogue_and_open_details() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("nes")).unwrap();
        let mut app = app();
        app.config.roms_dir = root.path().into();
        app.page = Some(smoke_page());
        app.open_detail();
        let id = app.detail.as_ref().unwrap().id.clone();
        fs::write(root.path().join("nes/demo0.nes"), b"newly installed").unwrap();
        app.refresh_local_presence(&id);
        assert!(app.page.as_ref().unwrap().items[0].installed);
        assert!(app.detail.as_ref().unwrap().installed);
        assert!(!app.page.as_ref().unwrap().items[1].installed);
        fs::remove_file(root.path().join("nes/demo0.nes")).unwrap();
        app.refresh_local_presence(&id);
        assert!(!app.detail.as_ref().unwrap().installed);
    }

    #[test]
    fn returning_from_detail_preserves_gallery_and_selection() {
        let mut app = app();
        app.page = Some(smoke_page());
        app.game_index = 6;
        let generation = app.gallery_generation;
        app.open_detail();
        assert_eq!(app.screen, Screen::Detail);
        app.action(Action::Back).unwrap();
        assert_eq!(app.screen, Screen::Games);
        assert_eq!(app.game_index, 6);
        assert_eq!(app.gallery_generation, generation);
        assert!(!app.gallery_cancel.load(Ordering::Relaxed));
        app.action(Action::Back).unwrap();
        assert!(app.gallery_cancel.load(Ordering::Relaxed));
        assert_ne!(app.gallery_generation, generation);
    }

    #[test]
    fn thumbnails_are_loaded_in_background_from_verified_cache() {
        let mut app = app();
        let root = tempfile::tempdir().unwrap();
        app.config.cache_dir = root.path().into();
        let bytes = b"P3\n2 2\n255\n98 212 173  24 36 59\n24 36 59  98 212 173\n";
        let file = File {
            role: "thumbnail".into(),
            ..art_file(bytes)
        };
        fs::write(root.path().join(format!("{}.image", file.sha256)), bytes).unwrap();
        let mut page = smoke_page();
        page.items.truncate(2);
        for game in &mut page.items {
            game.files = vec![file.clone()];
        }
        let ids: Vec<_> = page.items.iter().map(|game| game.id.clone()).collect();
        app.page = Some(page);
        let (tx, rx) = mpsc::channel();
        app.tx = tx;
        app.gallery_requests = ArtworkLoader::new(
            2,
            app.api.clone(),
            app.config.cache_dir.clone(),
            app.tx.clone(),
        );
        app.load_thumbnails();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Message::RetainThumbnails(_, _)
        ));
        let mut received = Vec::new();
        for _ in 0..2 {
            match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                Message::Thumbnail(generation, id, art) => {
                    assert_eq!(generation, app.gallery_generation);
                    let art = art.unwrap();
                    assert!(art.width() <= 256 && art.height() <= 256);
                    received.push(id);
                }
                _ => panic!("Réponse inattendue"),
            }
        }
        received.sort();
        assert_eq!(received, ids);
    }

    #[test]
    fn system_images_are_optional_for_older_servers() {
        let system: System =
            serde_json::from_str(r#"{"id":"nes","name":"NES","game_count":1}"#).unwrap();
        assert!(system.image.is_none());
    }

    #[test]
    fn configured_language_applies_to_navigation_and_unavailable_games() {
        let mut app = app();
        app.config.language = Language::En;
        app.page = Some(smoke_page());
        app.open_detail();
        assert_eq!(app.status, "Confirm to add to the download queue");
        app.detail.as_mut().unwrap().installable = false;
        app.detail.as_mut().unwrap().unavailable_reason =
            Some("Format multi-fichiers non pris en charge".into());
        app.enqueue().unwrap();
        assert_eq!(app.status, "Multi-file format not supported");
    }

    #[test]
    fn search_is_global_from_platforms_and_local_from_the_game_list() {
        let mut app = app();
        assert!(app.action(Action::Search).unwrap());
        assert_eq!(app.search.as_ref().unwrap().scope, None);
        assert!(
            app.action(Action::Back).unwrap(),
            "B closes the dialog instead of quitting"
        );
        assert!(app.search.is_none());
        app.screen = Screen::Games;
        app.catalog_system = Some("nes".into());
        app.search_query = "Mario".into();
        app.page = Some(smoke_page());
        app.game_index = 6;
        let generation = app.generation;
        app.action(Action::Search).unwrap();
        assert_eq!(app.search.as_ref().unwrap().scope.as_deref(), Some("nes"));
        app.search.as_mut().unwrap().query = "Éclair & Mario".into();
        app.action(Action::Confirm).unwrap();
        assert!(
            app.jobs.is_empty(),
            "Typing on the virtual keyboard must not install a game"
        );
        app.action(Action::Back).unwrap();
        assert_eq!(app.generation, generation);
        assert_eq!(app.search_query, "Mario");
        assert_eq!(app.game_index, 6);
        app.action(Action::Search).unwrap();
        app.search.as_mut().unwrap().query = "  Éclair & Mario  ".into();
        app.search.as_mut().unwrap().scope = None;
        app.search_outcome(Some(SearchOutcome::Submit));
        assert_eq!(app.search_query, "Éclair & Mario");
        assert_eq!(app.catalog_system, None);
        assert!(app.search.is_none());
        assert!(app.loading);
        assert_eq!(app.game_index, 0);
        assert_eq!(app.generation, generation + 1);
        app.page = Some(Page {
            items: smoke_page().items,
            total: 200,
            offset: 0,
            limit: 50,
        });
        app.loading = false;
        app.catalog_pending = None;
        app.load_more();
        assert_eq!(app.search_query, "Éclair & Mario");
        assert_eq!(app.catalog_system, None);
        assert_eq!(app.generation, generation + 1);
        assert_eq!(app.catalog_pending, Some(12));
    }

    #[test]
    fn catalogue_confirmation_only_opens_details_and_installation_requires_another_action() {
        for view in [ViewMode::Grid, ViewMode::List] {
            let mut app = app();
            app.screen = Screen::Games;
            app.config.games_view = view;
            let page = smoke_page();
            let expected = page.items[1].id.clone();
            // A previously opened detail must not override the catalogue selection.
            app.detail = Some(page.items[5].clone());
            app.page = Some(page);
            app.game_index = 1;
            let (installer, requests) = mpsc::channel();
            app.installer = installer;
            app.enqueue().unwrap();
            assert!(
                requests.try_recv().is_err(),
                "Catalogue actions must never install"
            );
            app.action(Action::Confirm).unwrap();
            assert_eq!(app.screen, Screen::Detail);
            assert_eq!(app.detail.as_ref().unwrap().id, expected);
            assert!(app.jobs.is_empty());
            assert!(requests.try_recv().is_err());
            app.action(Action::Confirm).unwrap();
            assert!(
                matches!(requests.try_recv().unwrap().1, Operation::Install(id) if id == expected)
            );
            assert_eq!(app.jobs[0].game_id, expected);
            app.action(Action::Confirm).unwrap();
            assert_eq!(app.jobs.len(), 1);
            assert!(requests.try_recv().is_err());
            app.action(Action::Confirm).unwrap();
            assert_eq!(app.screen, Screen::Detail);
            assert_eq!(app.detail.as_ref().unwrap().id, expected);
            assert_eq!(app.jobs.len(), 1);
            app.action(Action::Back).unwrap();
            assert_eq!(app.game_index, 1);
            assert_eq!(app.config.games_view, view);
            choose_menu(&mut app, MenuItem::Downloads);
            app.action(Action::Confirm).unwrap();
            assert!(app.jobs[0].cancel.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn installed_game_requires_explicit_uninstall_confirmation_with_cancel_selected_by_default() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("nes")).unwrap();
        let rom = root.path().join("nes/demo0.nes");
        fs::write(&rom, b"already installed").unwrap();
        let mut app = app();
        app.config.roms_dir = root.path().into();
        app.screen = Screen::Games;
        app.page = Some(smoke_page());
        let (installer, requests) = mpsc::channel();
        app.installer = installer;
        app.action(Action::Confirm).unwrap();
        assert!(app.detail.as_ref().unwrap().installed);
        assert!(app.jobs.is_empty());
        app.action(Action::Confirm).unwrap();
        assert!(!app.uninstall_confirmation.as_ref().unwrap().confirmed);
        app.action(Action::Confirm).unwrap(); // Default selection cancels.
        assert!(app.uninstall_confirmation.is_none());
        assert!(requests.try_recv().is_err());
        app.action(Action::Confirm).unwrap();
        app.action(Action::Right).unwrap();
        app.action(Action::Back).unwrap();
        assert!(requests.try_recv().is_err());
        app.action(Action::Confirm).unwrap();
        app.action(Action::Right).unwrap();
        app.action(Action::Search).unwrap(); // Modal must not change its target/screen.
        assert_eq!(app.screen, Screen::Detail);
        app.action(Action::Confirm).unwrap();
        let (_, operation, cancel, _, _) = requests.try_recv().unwrap();
        assert!(matches!(&operation, Operation::Uninstall(game) if game.path == "demo0.nes"));
        assert_eq!(app.jobs.len(), 1);
        assert_eq!(app.jobs[0].kind, JobKind::Uninstall);
        app.action(Action::Confirm).unwrap();
        assert!(
            app.uninstall_confirmation.is_none(),
            "Do not schedule a second operation on the same game"
        );
        assert!(
            rom.exists(),
            "Only the worker may perform the confirmed removal"
        );
        if let Operation::Uninstall(game) = operation {
            let result = uninstall::remove(root.path(), &game, &cancel, |_| {});
            app.finish_job(0, result);
        }
        assert!(!rom.exists());
        assert!(!app.detail.as_ref().unwrap().installed);
        assert!(!app.page.as_ref().unwrap().items[0].installed);
        assert!(app.jobs[0].succeeded);
        assert_eq!(app.status, app.config.language.text("uninstalled_hint"));
        app.action(Action::Confirm).unwrap();
        assert!(matches!(
            requests.try_recv().unwrap().1,
            Operation::Install(_)
        ));
    }

    #[test]
    fn loading_and_unavailable_games_cannot_be_queued_from_the_catalogue() {
        let mut app = app();
        app.screen = Screen::Games;
        app.page = Some(smoke_page());
        app.loading = true;
        app.action(Action::Confirm).unwrap();
        app.action(Action::Confirm).unwrap();
        assert!(app.jobs.is_empty());
        assert_eq!(app.screen, Screen::Games);
        app.loading = false;
        app.game_index = 2;
        app.action(Action::Confirm).unwrap();
        assert!(app.jobs.is_empty());
        app.action(Action::Alphabet).unwrap();
        assert_eq!(app.screen, Screen::Detail);
        app.action(Action::Confirm).unwrap();
        assert!(app.jobs.is_empty());
    }

    #[test]
    fn view_toggles_preserve_selection_and_have_independent_navigation() {
        let mut app = app();
        app.screen = Screen::Games;
        app.page = Some(smoke_page());
        app.game_index = 6;
        configure_view(&mut app, false);
        assert_eq!(app.config.games_view, ViewMode::List);
        assert_eq!(app.game_index, 6);
        app.action(Action::Down).unwrap();
        assert_eq!(app.game_index, 7);
        app.action(Action::Right).unwrap();
        assert_eq!(app.game_index, 7);
        configure_view(&mut app, false);
        app.action(Action::Up).unwrap();
        assert_eq!(app.game_index, 2);
        assert_eq!(app.config.systems_view, ViewMode::List);
        app.screen = Screen::Systems;
        app.systems = (0..12)
            .map(|id| System {
                id: id.to_string(),
                name: id.to_string(),
                game_count: 1,
                image: None,
            })
            .collect();
        app.system_index = 6;
        configure_view(&mut app, true);
        assert_eq!(app.system_index, 6);
        assert_eq!(app.config.systems_view, ViewMode::Grid);
        app.action(Action::Down).unwrap();
        assert_eq!(app.system_index, 11);
        app.action(Action::Up).unwrap();
        assert_eq!(app.system_index, 6);
        configure_view(&mut app, true);
        app.action(Action::Up).unwrap();
        assert_eq!(app.system_index, 5);
    }

    #[test]
    fn covers_and_screenshots_have_distinct_priorities_and_safe_fallbacks() {
        let mut game = smoke_page().items.remove(0);
        let make = |role: &str| File {
            id: role.into(),
            role: role.into(),
            ..art_file(b"art")
        };
        game.files = vec![
            make("thumbnail"),
            make("image"),
            make("titleshot"),
            make("screenshot"),
            make("boxart"),
        ];
        assert_eq!(cover_file(&game).unwrap().role, "thumbnail");
        assert_eq!(screenshot_file(&game).unwrap().role, "image");
        game.files.retain(|file| file.role != "image");
        assert_eq!(screenshot_file(&game).unwrap().role, "screenshot");
        game.files.retain(|file| file.role != "screenshot");
        assert_eq!(screenshot_file(&game).unwrap().role, "titleshot");
        game.files.retain(|file| file.role != "titleshot");
        assert!(screenshot_file(&game).is_none());
        game.files.retain(|file| file.role != "thumbnail");
        assert_eq!(cover_file(&game).unwrap().role, "boxart");
        game.files.retain(|file| file.role != "boxart");
        game.files.push(make("image"));
        assert!(cover_file(&game).is_none());
        assert_eq!(screenshot_file(&game).unwrap().role, "image");
        game.files.clear();
        assert!(cover_file(&game).is_none());
        assert!(screenshot_file(&game).is_none());
    }

    #[test]
    fn batocera_image_marquee_thumbnail_uses_the_expected_visuals() {
        let mut game = smoke_page().items.remove(0);
        let stem = "Mario & Luigi - Superstar Saga + Bowser's Minions (Europe) (En,Fr,De,Es,It,Nl)";
        game.files = [
            ("image", "image"),
            ("marquee", "marquee"),
            ("thumbnail", "thumb"),
        ]
        .into_iter()
        .map(|(role, suffix)| File {
            id: hex::encode(Sha256::digest(role.as_bytes())),
            role: role.into(),
            destination: format!("images/{stem}-{suffix}.png"),
            ..art_file(b"synthetic media")
        })
        .collect();
        assert_eq!(cover_file(&game).unwrap().role, "thumbnail");
        assert_eq!(screenshot_file(&game).unwrap().role, "image");
        assert!(cover_file(&game)
            .unwrap()
            .destination
            .contains("Mario & Luigi"));
        assert!(game.files.iter().any(|file| file.role == "marquee"));
        game.files.retain(|file| file.role != "thumbnail");
        assert!(cover_file(&game).is_none());
        assert_eq!(screenshot_file(&game).unwrap().role, "image");
        game.files.retain(|file| file.role != "image");
        assert!(
            screenshot_file(&game).is_none(),
            "The marquee must not become a screenshot"
        );
    }

    #[test]
    fn detail_loads_only_the_mix_image_even_when_other_media_are_available() {
        let mut app = app();
        let root = tempfile::tempdir().unwrap();
        app.config.cache_dir = root.path().into();
        let bytes = b"P3\n2 1\n255\n98 212 173  24 36 59\n";
        let image = File {
            role: "image".into(),
            ..art_file(bytes)
        };
        fs::write(root.path().join(format!("{}.image", image.sha256)), bytes).unwrap();
        let mut game = smoke_page().items.remove(0);
        game.files = [
            "thumbnail",
            "boxart",
            "screenshot",
            "titleshot",
            "marquee",
            "fanart",
        ]
        .into_iter()
        .map(|role| File {
            role: role.into(),
            ..image.clone()
        })
        .collect();
        game.files.push(image);
        app.screen = Screen::Detail;
        app.detail = Some(game);
        let (tx, rx) = mpsc::channel();
        app.tx = tx;
        app.preview_requests = ArtworkLoader::new(
            1,
            app.api.clone(),
            app.config.cache_dir.clone(),
            app.tx.clone(),
        );
        app.load_preview();
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Message::Art(generation, art) => {
                assert_eq!(generation, app.generation);
                assert_eq!(art.unwrap().get_pixel(0, 0).0, [98, 212, 173, 255]);
            }
            _ => panic!("Only the mix-image should be loaded on the detail screen"),
        }
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        app.detail
            .as_mut()
            .unwrap()
            .files
            .retain(|file| file.role != "image");
        app.load_preview();
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "Do not replace a missing image with separate covers, screenshots or logos"
        );
    }

    #[test]
    fn selected_system_image_is_loaded_from_verified_cache() {
        let mut app = app();
        let root = tempfile::tempdir().unwrap();
        app.config.cache_dir = root.path().into();
        let bytes = b"P3\n2 1\n255\n98 212 173  24 36 59\n";
        let file = art_file(bytes);
        fs::write(root.path().join(format!("{}.image", file.sha256)), bytes).unwrap();
        app.systems = vec![
            System {
                id: "nes".into(),
                name: "NES".into(),
                game_count: 1,
                image: Some(file),
            },
            System {
                id: "psx".into(),
                name: "PSX".into(),
                game_count: 1,
                image: None,
            },
        ];
        let (tx, rx) = mpsc::channel();
        app.tx = tx;
        app.preview_requests = ArtworkLoader::new(
            1,
            app.api.clone(),
            app.config.cache_dir.clone(),
            app.tx.clone(),
        );
        app.load_system_art();
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Message::Preview(generation, id, art) => {
                assert_eq!(generation, app.preview_generation);
                assert_eq!(id, "a".repeat(64));
                let art = art.unwrap();
                assert!(art.width() <= 640 && art.height() <= 640);
                assert_eq!(art.width(), art.height() * 2);
            }
            _ => panic!("Réponse inattendue"),
        }
        let generation = app.preview_generation;
        app.action(Action::Down).unwrap();
        assert_eq!(app.system_index, 1);
        assert!(app.preview_generation > generation);
        assert!(rx.try_recv().is_err());
    }
}
