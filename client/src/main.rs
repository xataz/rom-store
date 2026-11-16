mod artwork;
mod fonts;
mod input;
mod search;
mod smoke;
mod ui;
mod views;

use anyhow::{anyhow, bail, Context, Result};
use artwork::{ArtRequest, ArtTarget, ArtworkLoader};
use image::RgbaImage;
use input::{Action, Input};
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
    collections::{HashSet, VecDeque},
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
    ReloadStarted(Result<ReloadStatus>),
    ReloadState(Result<ReloadStatus>),
    Reloaded(Result<(ReloadStatus, Vec<System>)>),
    Systems(u64, Result<Vec<System>>),
    Games(u64, usize, Result<Page>),
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
type InstallRequest = (usize, Operation, Arc<AtomicBool>);

struct UninstallConfirmation {
    game: Game,
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
    if len == 0 {
        return 0;
    }
    let index = index.min(len - 1);
    match (action, view) {
        (Action::Up, ViewMode::Grid) if index >= GRID_COLUMNS => index - GRID_COLUMNS,
        (Action::Down, ViewMode::Grid) if index / GRID_COLUMNS < (len - 1) / GRID_COLUMNS => {
            (index + GRID_COLUMNS).min(len - 1)
        }
        (Action::Left, ViewMode::Grid) | (Action::Up, ViewMode::List) => index.saturating_sub(1),
        (Action::Right, ViewMode::Grid) | (Action::Down, ViewMode::List) => {
            (index + 1).min(len - 1)
        }
        _ => index,
    }
}

struct App {
    config: Config,
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
    jobs: Vec<Job>,
    status: String,
    quit_requested: bool,
    catalog_system: Option<String>,
    search_query: String,
    search: Option<SearchDialog>,
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
        let (api, tx) = (self.api.clone(), self.tx.clone());
        thread::spawn(move || {
            let _ = tx.send(Message::ReloadStarted(api.start_reload()));
        });
    }

    fn poll_reload(&mut self) {
        if self.reload_poll_pending || Instant::now() < self.next_reload_poll {
            return;
        }
        self.reload_poll_pending = true;
        let (api, tx) = (self.api.clone(), self.tx.clone());
        thread::spawn(move || {
            let _ = tx.send(Message::ReloadState(api.reload_status()));
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
                    let (api, tx) = (self.api.clone(), self.tx.clone());
                    thread::spawn(move || {
                        let result = api.systems().map(|systems| (state, systems));
                        let _ = tx.send(Message::Reloaded(result));
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
        thread::spawn(move || {
            let result = api
                .games(system.as_deref(), offset, &query)
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
            if loaded < page.total && self.game_index.saturating_add(20) >= loaded {
                self.fetch_games(loaded);
            }
        }
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
                    if self
                        .jobs
                        .iter()
                        .any(|job| job.game_id == game.id && job.done)
                    {
                        game.installed = self.config.rom_exists(&game.system, &game.path);
                    }
                }
                // An empty batch ends scrolling even if the server total is stale.
                if batch.items.is_empty() {
                    batch.total = offset;
                }
                if offset == 0 {
                    self.page = Some(batch);
                } else if let Some(page) = &mut self.page {
                    if page.items.len() != offset {
                        return false;
                    }
                    page.total = batch.total;
                    page.items.extend(batch.items);
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
        if self.jobs.iter().any(|j| j.game_id == game.id && !j.done) {
            self.status = self.config.language.text("already_queued").into();
            return Ok(());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let id = self.jobs.len();
        self.jobs.push(Job {
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
        });
        let operation = match kind {
            JobKind::Install => Operation::Install(game.id),
            JobKind::Uninstall => Operation::Uninstall(Box::new(game)),
        };
        self.installer.send((id, operation, cancel))?;
        self.status = self.config.language.text("queued_hint").into();
        Ok(())
    }

    fn action(&mut self, action: Action) -> Result<bool> {
        if self.uninstall_confirmation.is_some() && action != Action::Quit {
            match action {
                Action::Left | Action::Up => {
                    self.uninstall_confirmation.as_mut().unwrap().confirmed = false
                }
                Action::Right | Action::Down => {
                    self.uninstall_confirmation.as_mut().unwrap().confirmed = true
                }
                Action::Back | Action::Cancel | Action::Secondary | Action::Info => {
                    self.uninstall_confirmation = None
                }
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
        let action = if action == Action::Secondary {
            if self.screen == Screen::Queue {
                Action::Cancel
            } else {
                Action::Info
            }
        } else {
            action
        };
        if self.quit_requested {
            return Ok(self.jobs.iter().any(|job| !job.done));
        }
        if action == Action::Quit || (action == Action::Back && self.screen == Screen::Systems) {
            self.search = None;
            self.uninstall_confirmation = None;
            if self.jobs.iter().all(|job| job.done) {
                return Ok(false);
            }
            self.quit_requested = true;
            for job in &self.jobs {
                job.cancel.store(true, Ordering::Relaxed);
            }
            self.screen = Screen::Queue;
            self.status = self.config.language.text("closing").into();
            return Ok(true);
        }
        match action {
            Action::Reload if matches!(self.screen, Screen::Systems | Screen::Games) => {
                self.reload_catalog()
            }
            Action::Search if matches!(self.screen, Screen::Systems | Screen::Games) => {
                self.open_search()
            }
            Action::Queue => {
                if self.screen == Screen::Queue {
                    self.screen = self.before_queue;
                } else {
                    self.before_queue = self.screen;
                    self.screen = Screen::Queue;
                }
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
                    self.catalog_system = None;
                    self.gallery_cancel.store(true, Ordering::Relaxed);
                    self.gallery_generation += 1;
                    self.systems();
                }
                Screen::Systems => (),
            },
            Action::Cancel if self.screen == Screen::Queue => {
                if let Some(job) = self.jobs.get_mut(self.queue_index) {
                    if !job.done {
                        job.cancel.store(true, Ordering::Relaxed);
                        job.state = self.config.language.text("cancel_requested").into();
                    }
                }
            }
            Action::Up | Action::Down | Action::Left | Action::Right
                if matches!(self.screen, Screen::Systems | Screen::Games) && !self.loading =>
            {
                if self.screen == Screen::Games {
                    let len = self.page.as_ref().map_or(0, |page| page.items.len());
                    let previous = self.game_index;
                    self.game_index = move_selection(previous, len, action, self.config.games_view);
                    if previous != self.game_index {
                        self.load_thumbnails();
                        self.load_preview();
                    }
                    self.load_more();
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
                if len > 0 {
                    *index = if action == Action::Up {
                        index.saturating_sub(1)
                    } else {
                        (*index + 1).min(len - 1)
                    };
                }
            }
            Action::Confirm => match self.screen {
                Screen::Systems if !self.loading => {
                    if !self.systems.is_empty() {
                        self.catalog_system = self
                            .systems
                            .get(self.system_index)
                            .map(|system| system.id.clone());
                        self.search_query.clear();
                        self.screen = Screen::Games;
                        self.page = None;
                        self.game_index = 0;
                        self.games();
                    } else {
                        self.systems();
                    }
                }
                Screen::Games if !self.loading => self.open_detail(),
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
            Action::Info if self.screen == Screen::Games && !self.loading => self.open_detail(),
            Action::ToggleView
                if matches!(self.screen, Screen::Games | Screen::Systems) && !self.loading =>
            {
                if self.screen == Screen::Games {
                    self.config.games_view = self.config.games_view.toggled();
                    self.load_thumbnails();
                } else {
                    self.config.systems_view = self.config.systems_view.toggled();
                    self.load_system_thumbnails();
                }
                self.load_preview();
            }
            _ => (),
        }
        Ok(true)
    }

    fn message(&mut self, message: Message, painter: &mut Painter) -> Result<()> {
        match message {
            Message::ReloadStarted(Err(error)) => self.finish_reload(Err(error)),
            Message::ReloadStarted(Ok(state)) => self.receive_reload_status(Ok(state)),
            Message::ReloadState(result) => self.receive_reload_status(result),
            Message::Reloaded(result) => self.finish_reload(result),
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
                if let Some(job) = self.jobs.get_mut(id) {
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
                if let Some(job) = self.jobs.get_mut(id) {
                    job.progress = Some(progress);
                }
            }
            Message::Finished(id, result) => self.finish_job(id, result),
            _ => (),
        }
        Ok(())
    }

    fn finish_job(&mut self, id: usize, result: Result<()>) {
        if let Some(job) = self.jobs.get_mut(id) {
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
                        self.config.language.format(
                            if job.kind == JobKind::Uninstall {
                                "uninstall_failed"
                            } else {
                                "install_failed"
                            },
                            &[&format!("{error:#}")],
                        )
                    }
                }
            };
            self.status = job.state.clone();
        }
        if let Some(game_id) = self.jobs.get(id).map(|job| job.game_id.clone()) {
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

    fn search_outcome(&mut self, outcome: Option<SearchOutcome>) {
        match outcome {
            Some(SearchOutcome::Cancel) => self.search = None,
            Some(SearchOutcome::Submit) => {
                let search = self.search.take().unwrap();
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
        let language = if let Some(path) = &config_path {
            Config::load(path)?.language
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
    let config = if let Some(path) = config_path {
        Config::load(&path)?
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
    let (install_api, install_config, install_tx) = (api.clone(), config.clone(), tx.clone());
    thread::spawn(move || {
        for (id, operation, cancel) in requests {
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
        jobs: Vec::new(),
        status: String::new(),
        quit_requested: false,
        catalog_system: None,
        search_query: String::new(),
        search: None,
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
        if app.search.is_some() != text_input_active {
            text_input_active = app.search.is_some();
            if text_input_active {
                video.text_input().start();
            } else {
                video.text_input().stop();
            }
        }
        for event in events.poll_iter() {
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
                if matches!(action, Action::Confirm | Action::Info | Action::Secondary)
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
        for message in rx.try_iter().take(100) {
            app.message(message, &mut painter)?;
        }
        if app.quit_requested && app.jobs.iter().all(|job| job.done) {
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
            jobs: Vec::new(),
            status: String::new(),
            quit_requested: false,
            catalog_system: None,
            search_query: String::new(),
            search: None,
            uninstall_confirmation: None,
        }
    }

    #[test]
    fn queue_returns_to_previous_screen() {
        let mut app = app();
        app.screen = Screen::Detail;
        app.action(Action::Queue).unwrap();
        assert_eq!(app.screen, Screen::Queue);
        app.action(Action::Back).unwrap();
        assert_eq!(app.screen, Screen::Detail);
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
        app.action(Action::Reload).unwrap();
        assert!(app.reloading);
        app.action(Action::Reload).unwrap();
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
        assert!(app.quit_requested);
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(app.screen, Screen::Queue);
        app.jobs[0].done = true;
        assert!(!app.action(Action::Quit).unwrap());
    }

    #[test]
    fn empty_navigation_is_safe() {
        let mut app = app();
        assert!(app.action(Action::Up).unwrap());
        assert!(app.action(Action::Down).unwrap());
        assert_eq!(app.system_index, 0);
        assert!(!app.action(Action::Back).unwrap());
    }

    #[test]
    fn grid_navigation_moves_by_column_and_scrolls_by_row() {
        let mut app = app();
        app.screen = Screen::Games;
        app.page = Some(smoke_page());
        app.action(Action::Right).unwrap();
        assert_eq!(app.game_index, 1);
        app.action(Action::Up).unwrap();
        assert_eq!(app.game_index, 1);
        app.action(Action::Down).unwrap();
        assert_eq!(app.game_index, 6);
        assert_eq!(grid_start(app.game_index), 0);
        app.action(Action::Down).unwrap();
        assert_eq!(app.game_index, 11);
        assert_eq!(grid_start(app.game_index), 5);
        app.action(Action::Right).unwrap();
        app.action(Action::Down).unwrap();
        assert_eq!(app.game_index, 11);
        app.action(Action::Up).unwrap();
        assert_eq!(app.game_index, 6);
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
            app.action(Action::Info).unwrap();
            assert_eq!(app.screen, Screen::Detail);
            assert_eq!(app.detail.as_ref().unwrap().id, expected);
            assert_eq!(app.jobs.len(), 1);
            app.action(Action::Back).unwrap();
            assert_eq!(app.game_index, 1);
            assert_eq!(app.config.games_view, view);
            app.action(Action::Queue).unwrap();
            app.action(Action::Secondary).unwrap();
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
        app.action(Action::Queue).unwrap(); // Modal must not change its target/screen.
        assert_eq!(app.screen, Screen::Detail);
        app.action(Action::Confirm).unwrap();
        let (_, operation, cancel) = requests.try_recv().unwrap();
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
        app.action(Action::Info).unwrap();
        assert!(app.jobs.is_empty());
        assert_eq!(app.screen, Screen::Games);
        app.loading = false;
        app.game_index = 2;
        app.action(Action::Confirm).unwrap();
        assert!(app.jobs.is_empty());
        app.action(Action::Secondary).unwrap();
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
        app.action(Action::ToggleView).unwrap();
        assert_eq!(app.config.games_view, ViewMode::List);
        assert_eq!(app.game_index, 6);
        app.action(Action::Down).unwrap();
        assert_eq!(app.game_index, 7);
        app.action(Action::Right).unwrap();
        assert_eq!(app.game_index, 7);
        app.action(Action::ToggleView).unwrap();
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
        app.action(Action::ToggleView).unwrap();
        assert_eq!(app.system_index, 6);
        assert_eq!(app.config.systems_view, ViewMode::Grid);
        app.action(Action::Down).unwrap();
        assert_eq!(app.system_index, 11);
        app.action(Action::Up).unwrap();
        assert_eq!(app.system_index, 6);
        app.action(Action::ToggleView).unwrap();
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
