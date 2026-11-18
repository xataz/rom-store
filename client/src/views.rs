//! Presentation only: navigation, networking and installation stay in App.
use crate::{
    grid_start, media_file, screenshot_file,
    ui::{self, Painter, ACCENT, BORDER, DANGER, GOLD, MUTED, SURFACE, TEXT},
    App, Job, JobKind, Screen, ViewMode, GRID_COLUMNS, GRID_ROWS,
};
use anyhow::Result;
use romstore_client::i18n::Language;
use sdl2::{pixels::Color, rect::Rect, render::Canvas, video::Window};

type Scene = Canvas<Window>;

fn active_download(jobs: &[Job]) -> Option<(usize, usize, &Job)> {
    let total = jobs
        .iter()
        .filter(|job| job.kind == JobKind::Install)
        .count();
    jobs.iter()
        .filter(|job| job.kind == JobKind::Install)
        .enumerate()
        .find(|(_, job)| job.started.is_some() && !job.done)
        .map(|(index, job)| (index + 1, total, job))
}

fn job_fraction(job: &Job) -> f64 {
    if job.succeeded {
        return 1.0;
    }
    job.progress.as_ref().map_or(0.0, |p| {
        if p.total == 0 {
            0.0
        } else {
            (p.completed as f64 / p.total as f64).clamp(0.0, 1.0)
        }
    })
}

fn job_label(job: &Job, l: Language) -> &'static str {
    if job.succeeded {
        l.text(if job.kind == JobKind::Uninstall {
            "uninstalled"
        } else {
            "installed"
        })
    } else if job.done {
        if job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            l.text("cancelled")
        } else {
            l.text("failed")
        }
    } else if job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
        l.text("cancelling")
    } else if job.started.is_some() {
        l.text(if job.kind == JobKind::Uninstall {
            "uninstalling"
        } else {
            "in_progress"
        })
    } else {
        l.text("waiting")
    }
}

fn job_color(job: &Job) -> Color {
    if job.succeeded {
        ACCENT
    } else if job.done {
        DANGER
    } else if job.started.is_some() {
        GOLD
    } else {
        MUTED
    }
}

fn empty(canvas: &mut Scene, painter: &mut Painter, title: &str, description: &str) -> Result<()> {
    ui::panel(canvas, Rect::new(40, 184, 1200, 436), false)?;
    painter.placeholder(canvas, Rect::new(550, 232, 180, 122), "ROM STORE")?;
    painter.text(canvas, title, (240, 386), 30, TEXT, 800)?;
    painter.paragraph(canvas, description, (240, 438), 800, 3)
}

impl App {
    fn system_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.systems
            .iter()
            .find(|system| system.id == id)
            .map_or_else(
                || romstore_client::systems::display_name(id, id),
                |system| system.display_name(),
            )
    }

    pub(super) fn draw(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        painter.background(canvas)?;
        self.draw_header(canvas, painter)?;
        match self.screen {
            Screen::Systems => self.draw_systems(canvas, painter)?,
            Screen::Games => self.draw_games(canvas, painter)?,
            Screen::Detail => self.draw_detail(canvas, painter)?,
            Screen::Queue => self.draw_queue(canvas, painter)?,
        }
        self.draw_footer(canvas, painter)?;
        if let Some(menu) = &self.menu {
            menu.draw(
                canvas,
                painter,
                &self.config,
                self.reloading,
                self.frontend_refreshing,
            )?;
        }
        if let Some(settings) = &self.settings {
            settings.draw(canvas, painter, self.config.language)?;
        }
        if let Some(advanced) = &self.advanced {
            advanced.draw(canvas, painter, self.config.language)?;
        }
        if let Some(search) = &self.search {
            search.draw(
                canvas,
                painter,
                self.config.language,
                search.scope.as_deref().map(|id| self.system_name(id)),
            )?;
        }
        if let Some(alphabet) = &self.alphabet {
            alphabet.draw(canvas, painter, self.config.language)?;
        }
        if self.uninstall_confirmation.is_some() {
            self.draw_uninstall_confirmation(canvas, painter)?;
        }
        if self.quit_confirmation.is_some() {
            self.draw_quit_confirmation(canvas, painter)?;
        }
        Ok(())
    }

    fn draw_quit_confirmation(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        let Some(confirmation) = &self.quit_confirmation else {
            return Ok(());
        };
        let l = self.config.language;
        let pending = self.jobs.iter().filter(|job| !job.done).count();
        ui::dim(canvas, 220)?;
        ui::panel(canvas, Rect::new(200, 155, 880, 420), true)?;
        painter.text(
            canvas,
            l.text("quit_confirm_title"),
            (236, 182),
            30,
            TEXT,
            808,
        )?;
        painter.paragraph(
            canvas,
            &if pending == 0 && self.frontend_refreshing {
                l.text("quit_confirm_refresh").to_owned()
            } else {
                l.format("quit_confirm_body", &[&pending])
            },
            (236, 253),
            808,
            4,
        )?;
        for (x, selected, label, color) in [
            (
                236,
                !confirmation.confirmed,
                l.text("quit_continue"),
                ACCENT,
            ),
            (654, confirmation.confirmed, l.text("quit_stop"), DANGER),
        ] {
            ui::rounded(
                canvas,
                Rect::new(x, 445, 390, 54),
                10,
                if selected { color } else { SURFACE },
            )?;
            painter.text(
                canvas,
                label,
                (x + 20, 458),
                21,
                if selected { ui::BG } else { color },
                350,
            )?;
        }
        painter.text(
            canvas,
            l.text("uninstall_confirm_controls"),
            (236, 529),
            15,
            MUTED,
            808,
        )
    }

    fn draw_uninstall_confirmation(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        let Some(confirmation) = &self.uninstall_confirmation else {
            return Ok(());
        };
        let l = self.config.language;
        ui::dim(canvas, 215)?;
        ui::panel(canvas, Rect::new(200, 155, 880, 420), true)?;
        painter.text(
            canvas,
            l.text("uninstall_confirm_title"),
            (236, 180),
            30,
            DANGER,
            808,
        )?;
        painter.text(canvas, &confirmation.game.name, (236, 228), 26, TEXT, 808)?;
        painter.paragraph(canvas, l.text("uninstall_confirm_body"), (236, 282), 808, 3)?;
        let target = self
            .config
            .roms_dir
            .join(&confirmation.game.system)
            .join(&confirmation.game.path);
        painter.text(
            canvas,
            &target.display().to_string(),
            (236, 400),
            14,
            MUTED,
            808,
        )?;
        for (x, selected, label, color) in [
            (236, !confirmation.confirmed, l.text("cancel"), ACCENT),
            (654, confirmation.confirmed, l.text("uninstall"), DANGER),
        ] {
            ui::rounded(
                canvas,
                Rect::new(x, 445, 390, 54),
                10,
                if selected { color } else { SURFACE },
            )?;
            painter.text(
                canvas,
                label,
                (x + 20, 458),
                23,
                if selected { ui::BG } else { color },
                350,
            )?;
        }
        painter.text(
            canvas,
            l.text("uninstall_confirm_controls"),
            (236, 529),
            15,
            MUTED,
            808,
        )?;
        Ok(())
    }

    fn draw_header(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        let l = self.config.language;
        let download = active_download(&self.jobs);
        for (x, y, color) in [
            (40, 25, ACCENT),
            (57, 25, ACCENT),
            (40, 42, ACCENT),
            (57, 42, GOLD),
        ] {
            ui::rounded(canvas, Rect::new(x, y, 13, 13), 3, color)?;
        }
        painter.text(canvas, "ROM STORE", (88, 23), 27, TEXT, 300)?;
        painter.pill(
            canvas,
            concat!("v", env!("CARGO_PKG_VERSION")),
            (280, 28),
            MUTED,
        )?;
        painter.text(
            canvas,
            l.text(if self.reloading {
                "reloading_catalog"
            } else {
                "tagline"
            }),
            (90, 58),
            10,
            if self.reloading { ACCENT } else { MUTED },
            if download.is_some() { 290 } else { 430 },
        )?;
        painter.pill(canvas, l.text("menu_shortcut"), (1000, 30), ACCENT)?;
        if let Some((position, total, job)) = download {
            // Centered in the 1280-pixel logical viewport, between branding and menu.
            ui::panel(canvas, Rect::new(400, 8, 480, 72), true)?;
            let fraction = job_fraction(job);
            painter.text(
                canvas,
                &l.format("download_notification", &[&position, &total]),
                (418, 15),
                14,
                ACCENT,
                366,
            )?;
            painter.text(
                canvas,
                &format!("{:.0}%", fraction * 100.0),
                (802, 15),
                14,
                ACCENT,
                60,
            )?;
            painter.text(canvas, &job.name, (418, 36), 18, TEXT, 444)?;
            ui::progress(canvas, Rect::new(418, 65, 444, 6), fraction, ACCENT)?;
        }
        ui::rounded(canvas, Rect::new(40, 86, 1200, 1), 0, BORDER)
    }

    fn heading(
        &self,
        canvas: &mut Scene,
        painter: &mut Painter,
        eyebrow: &str,
        title: &str,
    ) -> Result<()> {
        painter.text(canvas, eyebrow, (40, 105), 12, ACCENT, 700)?;
        painter.text(canvas, title, (40, 126), 34, TEXT, 930)
    }

    fn draw_systems(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        let l = self.config.language;
        self.heading(
            canvas,
            painter,
            l.text("systems_eyebrow"),
            l.text("systems_title"),
        )?;
        painter.pill(
            canvas,
            l.text(if self.config.systems_view == ViewMode::Grid {
                "grid_view"
            } else {
                "list_view"
            }),
            (780, 103),
            ACCENT,
        )?;
        painter.pill(
            canvas,
            &l.format("platform_count", &[&self.systems.len()]),
            (1060, 133),
            MUTED,
        )?;
        if self.systems.is_empty() {
            return empty(
                canvas,
                painter,
                if self.loading {
                    l.text("connecting_library")
                } else {
                    l.text("collection_starts")
                },
                l.text("systems_empty_help"),
            );
        }
        if self.config.systems_view == ViewMode::Grid {
            return self.draw_system_grid(canvas, painter);
        }
        let start = self.system_index.saturating_sub(5);
        for (index, system) in self.systems.iter().enumerate().skip(start).take(6) {
            let y = 184 + (index - start) as i32 * 73;
            let selected = index == self.system_index;
            ui::panel(canvas, Rect::new(40, y, 352, 64), selected)?;
            painter.text(
                canvas,
                system.display_name(),
                (56, y + 18),
                23,
                if selected { ACCENT } else { TEXT },
                320,
            )?;
        }
        if self.systems.len() > 6 {
            ui::progress(
                canvas,
                Rect::new(40, 628, 352, 3),
                (self.system_index + 1) as f64 / self.systems.len() as f64,
                ACCENT,
            )?;
        }
        if let Some(system) = self.systems.get(self.system_index) {
            ui::panel(canvas, Rect::new(416, 184, 824, 436), false)?;
            painter.pill(canvas, l.text("your_turn"), (442, 204), ACCENT)?;
            painter.text(
                canvas,
                &format!("{:02} / {:02}", self.system_index + 1, self.systems.len()),
                (1114, 212),
                13,
                MUTED,
                100,
            )?;
            painter.preview(
                canvas,
                system.image.as_ref().map_or("", |file| file.id.as_str()),
                Rect::new(520, 248, 616, 234),
                l.text("platform_placeholder"),
            )?;
            painter.text(canvas, system.display_name(), (446, 499), 32, TEXT, 742)?;
            painter.text(
                canvas,
                &l.format("games_available", &[&system.game_count]),
                (448, 546),
                17,
                MUTED,
                742,
            )?;
            painter.text(
                canvas,
                l.text("explore_collection"),
                (448, 581),
                18,
                ACCENT,
                700,
            )?;
        }
        Ok(())
    }

    fn draw_system_grid(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        let l = self.config.language;
        let start = grid_start(self.system_index);
        for (index, system) in self
            .systems
            .iter()
            .enumerate()
            .skip(start)
            .take(GRID_COLUMNS * GRID_ROWS)
        {
            let position = index - start;
            let x = 40 + (position % GRID_COLUMNS) as i32 * 242;
            let y = 184 + (position / GRID_COLUMNS) as i32 * 196;
            let selected = index == self.system_index;
            painter.cover_tile(
                canvas,
                &format!("system:{}", system.id),
                Rect::new(x + 10, y + 10, 212, 124),
                selected,
            )?;
            if selected {
                ui::rounded(canvas, Rect::new(x, y + 140, 232, 44), 9, SURFACE)?;
            }
            painter.text(
                canvas,
                system.display_name(),
                (x + 13, y + 143),
                18,
                if selected { ACCENT } else { TEXT },
                206,
            )?;
            painter.text(
                canvas,
                &l.format("games_discover", &[&system.game_count]),
                (x + 14, y + 166),
                11,
                MUTED,
                204,
            )?;
        }
        if let Some(system) = self.systems.get(self.system_index) {
            ui::rounded(canvas, Rect::new(40, 582, 1200, 40), 10, SURFACE)?;
            painter.text(canvas, system.display_name(), (56, 589), 21, TEXT, 860)?;
            painter.text(
                canvas,
                &format!("{} / {}", self.system_index + 1, self.systems.len()),
                (1110, 595),
                14,
                MUTED,
                110,
            )?;
        }
        Ok(())
    }

    fn draw_games(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        let l = self.config.language;
        let system = self
            .catalog_system
            .as_ref()
            .map_or(l.text("all_platforms"), |id| self.system_name(id));
        let eyebrow = if !self.filters.is_empty() {
            l.format("advanced_active", &[&self.filters.len()])
        } else if self.search_query.is_empty() {
            l.text("collection_eyebrow").to_owned()
        } else {
            l.format("search_results", &[&self.search_query])
        };
        self.heading(canvas, painter, &eyebrow, system)?;
        painter.pill(
            canvas,
            l.text(if self.config.games_view == ViewMode::Grid {
                "grid_view"
            } else {
                "list_view"
            }),
            (780, 103),
            ACCENT,
        )?;
        if self.loading {
            // Initial loads and alphabet jumps use placeholders; scrolling does not.
            for index in 0..GRID_COLUMNS * GRID_ROWS {
                let x = 40 + (index % GRID_COLUMNS) as i32 * 242;
                let y = 184 + (index / GRID_COLUMNS) as i32 * 196;
                ui::panel(canvas, Rect::new(x, y, 232, 184), false)?;
                ui::rounded(canvas, Rect::new(x + 12, y + 12, 208, 122), 8, BORDER)?;
                ui::rounded(canvas, Rect::new(x + 14, y + 148, 150, 8), 3, BORDER)?;
            }
            return Ok(());
        }
        let Some(page) = &self.page else {
            return empty(
                canvas,
                painter,
                l.text("catalog_unavailable"),
                l.text("catalog_retry"),
            );
        };
        if page.items.is_empty() {
            if !self.search_query.is_empty() || !self.filters.is_empty() {
                return empty(
                    canvas,
                    painter,
                    &if self.search_query.is_empty() {
                        l.text("advanced_no_results").to_owned()
                    } else {
                        l.format("search_no_results", &[&self.search_query])
                    },
                    l.text("search_empty_help"),
                );
            }
            return empty(canvas, painter, l.text("no_games"), l.text("no_games_help"));
        }
        painter.pill(
            canvas,
            &l.format(
                "catalog_position",
                &[&(page.offset + self.game_index + 1), &page.total],
            ),
            (1010, 133),
            MUTED,
        )?;
        if self.selection.enabled {
            painter.pill(
                canvas,
                &l.format("selection_count", &[&self.selection.games.len()]),
                (780, 139),
                GOLD,
            )?;
        } else if self.catalog_pending.is_some() {
            painter.text(canvas, l.text("loading_more"), (780, 145), 13, ACCENT, 210)?;
        }
        if self.config.games_view == ViewMode::List {
            return self.draw_game_list(canvas, painter);
        }
        let start = grid_start(self.game_index);
        for (index, game) in page
            .items
            .iter()
            .enumerate()
            .skip(start)
            .take(GRID_COLUMNS * GRID_ROWS)
        {
            let position = index - start;
            let x = 40 + (position % GRID_COLUMNS) as i32 * 242;
            let y = 184 + (position / GRID_COLUMNS) as i32 * 196;
            let selected = index == self.game_index;
            painter.cover_tile(
                canvas,
                &game.id,
                Rect::new(x + 10, y + 10, 212, 124),
                selected,
            )?;
            if selected {
                ui::rounded(canvas, Rect::new(x, y + 140, 232, 44), 9, SURFACE)?;
            }
            if game.installed {
                painter.pill(canvas, "✓", (x + 188, y + 12), ACCENT)?;
            }
            if self.selection.enabled {
                painter.pill(
                    canvas,
                    if self.selection.contains(&game.id) {
                        "☑"
                    } else {
                        "□"
                    },
                    (x + 12, y + 12),
                    GOLD,
                )?;
            }
            painter.text(
                canvas,
                &game.name,
                (x + 13, y + 143),
                18,
                if selected { ACCENT } else { TEXT },
                206,
            )?;
            painter.text(
                canvas,
                &if game.installed {
                    if self.catalog_system.is_none() {
                        format!(
                            "{} · ✓ {}",
                            self.system_name(&game.system),
                            l.text("installed")
                        )
                    } else {
                        format!("✓ {}", l.text("installed"))
                    }
                } else if self.catalog_system.is_none() {
                    format!(
                        "{} · {}",
                        self.system_name(&game.system),
                        if game.installable {
                            l.text("rom_media")
                        } else {
                            l.text("unsupported_badge")
                        }
                    )
                } else if game.installable {
                    l.text("rom_media").into()
                } else {
                    l.text("unsupported_badge").into()
                },
                (x + 14, y + 167),
                10,
                if game.installed {
                    ACCENT
                } else if game.installable {
                    MUTED
                } else {
                    GOLD
                },
                204,
            )?;
        }
        if let Some(game) = page.items.get(self.game_index) {
            ui::rounded(canvas, Rect::new(40, 582, 1200, 40), 10, SURFACE)?;
            painter.text(canvas, &game.name, (56, 589), 21, TEXT, 850)?;
            painter.text(
                canvas,
                l.text(if self.selection.enabled {
                    "selection_batch_hint"
                } else {
                    "quick_actions"
                }),
                (930, 595),
                14,
                MUTED,
                294,
            )?;
        }
        Ok(())
    }

    fn draw_game_list(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        let l = self.config.language;
        let Some(page) = &self.page else {
            return Ok(());
        };
        let start = self.game_index.saturating_sub(8);
        for (index, game) in page.items.iter().enumerate().skip(start).take(9) {
            let y = 184 + (index - start) as i32 * 48;
            let selected = index == self.game_index;
            ui::panel(canvas, Rect::new(40, y, 456, 43), selected)?;
            if self.selection.enabled {
                painter.text(
                    canvas,
                    if self.selection.contains(&game.id) {
                        "☑"
                    } else {
                        "□"
                    },
                    (56, y + 9),
                    21,
                    GOLD,
                    26,
                )?;
            }
            painter.text(
                canvas,
                &game.name,
                (if self.selection.enabled { 88 } else { 56 }, y + 10),
                18,
                if selected { ACCENT } else { TEXT },
                (if game.installed { 382 } else { 424 })
                    - if self.selection.enabled { 32 } else { 0 },
            )?;
            if game.installed {
                painter.text(canvas, "✓", (458, y + 10), 20, ACCENT, 26)?;
            }
        }
        if let Some(game) = page.items.get(self.game_index) {
            ui::panel(canvas, Rect::new(520, 184, 720, 436), false)?;
            let system_name = self.system_name(&game.system);
            let badge_width = (painter.width(system_name, 14).ceil() as u32 + 24).min(300);
            let title_width = if self.catalog_system.is_none() {
                674 - badge_width - 16
            } else {
                674
            };
            let logo = media_file(game, "marquee")
                .map(|file| file.id.as_str())
                .unwrap_or("");
            if !painter.marquee(canvas, logo, Rect::new(542, 194, title_width, 42))? {
                painter.text(canvas, &game.name, (542, 201), 26, TEXT, title_width)?;
            }
            if self.catalog_system.is_none() {
                painter.pill_bounded(
                    canvas,
                    system_name,
                    (1216 - badge_width as i32, 201),
                    ACCENT,
                    badge_width,
                )?;
            }
            painter.thumbnail(canvas, &game.id, Rect::new(542, 250, 184, 214))?;
            if game.installed {
                painter.pill(
                    canvas,
                    &format!("✓ {}", l.text("installed")),
                    (1030, 587),
                    ACCENT,
                )?;
            }
            if let Some(file) = screenshot_file(game) {
                painter.preview(
                    canvas,
                    &file.id,
                    Rect::new(746, 250, 470, 214),
                    l.text("screenshot_missing"),
                )?;
            } else {
                painter.placeholder(
                    canvas,
                    Rect::new(746, 250, 470, 214),
                    l.text("screenshot_missing"),
                )?;
            }
            painter.paragraph(
                canvas,
                game.metadata
                    .get("desc")
                    .map_or(l.text("no_description"), String::as_str),
                (544, 514),
                670,
                2,
            )?;
            painter.text(
                canvas,
                &l.format(
                    "game_facts",
                    &[
                        &game
                            .metadata
                            .get("developer")
                            .map_or(l.text("unknown"), String::as_str),
                        &game
                            .metadata
                            .get("players")
                            .map_or(l.text("unknown"), String::as_str),
                    ],
                ),
                (544, 480),
                14,
                MUTED,
                670,
            )?;
            painter.text(
                canvas,
                l.text(if self.selection.enabled {
                    "selection_batch_hint"
                } else {
                    "quick_actions"
                }),
                (544, 593),
                15,
                if game.installable { ACCENT } else { GOLD },
                668,
            )?;
        }
        Ok(())
    }

    fn draw_detail(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        let l = self.config.language;
        let Some(game) = &self.detail else {
            return Ok(());
        };
        self.heading(
            canvas,
            painter,
            &l.format("detail_eyebrow", &[&self.system_name(&game.system)]),
            &game.name,
        )?;
        // The mix-image already includes the cover, logo and screenshot.
        ui::panel(canvas, Rect::new(40, 184, 660, 436), false)?;
        painter.art(canvas, Rect::new(58, 202, 624, 400))?;
        let badge_width = painter.pill_bounded(
            canvas,
            self.system_name(&game.system),
            (724, 188),
            ACCENT,
            320,
        )?;
        if game.installed {
            painter.pill(
                canvas,
                &format!("✓ {}", l.text("installed")),
                (1010, 133),
                ACCENT,
            )?;
        }
        if let Some(genre) = game.metadata.get("genre").filter(|text| !text.is_empty()) {
            painter.text(
                canvas,
                genre,
                (740 + badge_width as i32, 194),
                17,
                MUTED,
                480_u32.saturating_sub(badge_width),
            )?;
        }
        painter.text(canvas, l.text("story_title"), (724, 240), 25, TEXT, 516)?;
        painter.paragraph(
            canvas,
            game.metadata
                .get("desc")
                .map_or(l.text("no_description"), String::as_str),
            (724, 282),
            516,
            5,
        )?;
        for (index, (key, label)) in [
            ("developer", l.text("developer")),
            ("players", l.text("players")),
        ]
        .iter()
        .enumerate()
        {
            let x = 724 + index as i32 * 310;
            let width = if index == 0 { 286 } else { 206 };
            painter.text(canvas, label, (x, 454), 11, MUTED, width)?;
            painter.text(
                canvas,
                game.metadata
                    .get(*key)
                    .filter(|text| !text.is_empty())
                    .map_or(l.text("unknown"), String::as_str),
                (x, 476),
                19,
                TEXT,
                width,
            )?;
        }
        let size = game
            .files
            .iter()
            .fold(0_u64, |total, file| total.saturating_add(file.size));
        let summary = if game.installed {
            l.text("uninstall_preserved").into()
        } else if game.installable {
            l.format(
                "files_summary",
                &[
                    &game.files.len(),
                    &format!("{:.2}", size as f64 / 1048576.0),
                ],
            )
        } else {
            l.unavailable_reason(game.unavailable_reason.as_deref())
                .into()
        };
        painter.text(
            canvas,
            &summary,
            (724, 521),
            16,
            if game.installable { MUTED } else { GOLD },
            516,
        )?;
        let busy = self
            .jobs
            .iter()
            .any(|job| job.game_id == game.id && !job.done);
        let actionable = (game.installable || game.installed) && !busy;
        let action_color = if game.installed { DANGER } else { ACCENT };
        ui::rounded(
            canvas,
            Rect::new(724, 560, 516, 60),
            12,
            if actionable { action_color } else { SURFACE },
        )?;
        painter.text(
            canvas,
            if busy {
                l.text("operation_pending")
            } else if game.installed {
                l.text("uninstall_game")
            } else if game.installable {
                l.text("install_game")
            } else {
                l.text("install_unavailable")
            },
            (744, 576),
            23,
            if actionable { ui::BG } else { GOLD },
            328,
        )?;
        if actionable {
            painter.text(canvas, l.text("install_key"), (1092, 582), 15, ui::BG, 128)?;
        }
        Ok(())
    }

    fn draw_queue(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        let l = self.config.language;
        self.heading(
            canvas,
            painter,
            l.text("transfers_eyebrow"),
            l.text("transfers_title"),
        )?;
        painter.pill(
            canvas,
            &l.format(
                "completed_count",
                &[
                    &self.jobs.iter().filter(|job| job.done).count(),
                    &self.jobs.len(),
                ],
            ),
            (1040, 133),
            MUTED,
        )?;
        if self.jobs.is_empty() {
            return empty(
                canvas,
                painter,
                l.text("queue_empty"),
                l.text("queue_empty_help"),
            );
        }
        let start = self.queue_index.saturating_sub(3);
        for (index, job) in self.jobs.iter().enumerate().skip(start).take(4) {
            let y = 184 + (index - start) as i32 * 110;
            ui::panel(
                canvas,
                Rect::new(40, y, 764, 102),
                index == self.queue_index,
            )?;
            painter.text(
                canvas,
                &format!("{:02}", index + 1),
                (60, y + 21),
                19,
                ACCENT,
                36,
            )?;
            painter.text(canvas, &job.name, (112, y + 14), 23, TEXT, 660)?;
            painter.text(
                canvas,
                job_label(job, l),
                (114, y + 47),
                11,
                job_color(job),
                440,
            )?;
            painter.text(
                canvas,
                &format!("{:.0}%", job_fraction(job) * 100.0),
                (718, y + 46),
                13,
                MUTED,
                62,
            )?;
            ui::progress(
                canvas,
                Rect::new(114, y + 77, 660, 5),
                job_fraction(job),
                job_color(job),
            )?;
        }
        if let Some(job) = self.jobs.get(self.queue_index) {
            ui::panel(canvas, Rect::new(828, 184, 412, 436), false)?;
            painter.pill(canvas, job_label(job, l), (852, 206), job_color(job))?;
            painter.text(
                canvas,
                &format!("{:.0}%", job_fraction(job) * 100.0),
                (852, 252),
                60,
                TEXT,
                360,
            )?;
            painter.text(canvas, &job.name, (854, 336), 23, TEXT, 356)?;
            painter.paragraph(canvas, &job.state, (854, 378), 354, 4)?;
            if job.kind == JobKind::Uninstall {
                painter.paragraph(canvas, l.text("uninstall_preserved"), (854, 520), 354, 2)?;
            } else if let Some(p) = &job.progress {
                painter.text(
                    canvas,
                    &l.format(
                        "transfer_amount",
                        &[
                            &format!("{:.1}", p.completed as f64 / 1048576.0),
                            &format!("{:.1}", p.total as f64 / 1048576.0),
                        ],
                    ),
                    (854, 520),
                    17,
                    MUTED,
                    354,
                )?;
                let seconds = job
                    .started
                    .map_or(1.0, |start| start.elapsed().as_secs_f64().max(0.1));
                let transfer = if job.succeeded {
                    l.text("transfer_done").to_owned()
                } else if job.done {
                    l.text("transfer_interrupted").to_owned()
                } else if job.started.is_none() {
                    l.text("transfer_pending").to_owned()
                } else {
                    l.format(
                        "transfer_speed",
                        &[&format!("{:.1}", p.completed as f64 / 1048576.0 / seconds)],
                    )
                };
                painter.text(canvas, &transfer, (854, 554), 16, job_color(job), 354)?;
            }
            painter.text(
                canvas,
                l.text("collection_accessible"),
                (854, 590),
                13,
                MUTED,
                354,
            )?;
        }
        Ok(())
    }

    fn draw_footer(&self, canvas: &mut Scene, painter: &mut Painter) -> Result<()> {
        let l = self.config.language;
        let status_color = if self.quit_requested { GOLD } else { MUTED };
        // The small status rail remains visible for network errors and installation feedback.
        ui::rounded(canvas, Rect::new(40, 647, 6, 6), 3, status_color)?;
        painter.text(
            canvas,
            if self.reloading {
                l.text("reloading_catalog")
            } else {
                &self.status
            },
            (56, 640),
            15,
            status_color,
            1184,
        )?;
        ui::rounded(canvas, Rect::new(40, 665, 1200, 1), 0, BORDER)?;
        let label = match self.screen {
            Screen::Games if self.selection.enabled => l.text("selection_toggle"),
            Screen::Games => l.text("details"),
            Screen::Queue => l.text("cancel"),
            Screen::Detail if self.detail.as_ref().is_some_and(|game| game.installed) => {
                l.text("uninstall")
            }
            Screen::Detail => l.text("install"),
            Screen::Systems => l.text("choose"),
        };
        let x = painter.hint(canvas, l.text("enter_key"), label, 40)?;
        let selecting = self.screen == Screen::Games && self.selection.enabled;
        let mut x = painter.hint(
            canvas,
            l.text("back_key"),
            l.text(if selecting { "cancel" } else { "back" }),
            x,
        )?;
        if self.screen == Screen::Queue {
            painter.hint(
                canvas,
                l.text("selection_download_key"),
                l.text("queue_clean"),
                x,
            )?;
        }
        if selecting {
            x = painter.hint(
                canvas,
                l.text("selection_download_key"),
                l.text("selection_download"),
                x,
            )?;
            painter.hint(canvas, "L / R1", l.text("alphabet_jump"), x)?;
            return Ok(());
        }
        if matches!(self.screen, Screen::Systems | Screen::Games) {
            x = painter.hint(canvas, "F / L3", l.text("search"), x)?;
            if self.screen == Screen::Games {
                x = painter.hint(
                    canvas,
                    l.text("selection_hold_key"),
                    l.text("multi_select"),
                    x,
                )?;
            }
            painter.hint(canvas, "L / R1", l.text("alphabet_jump"), x)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_only_counts_downloads_and_never_promotes_waiting_jobs() {
        let job = |kind, done, started| Job {
            id: 0,
            kind,
            done,
            started,
            game_id: String::new(),
            name: "Download".into(),
            cancel: Default::default(),
            state: String::new(),
            succeeded: false,
            progress: None,
        };
        let mut jobs = vec![
            job(JobKind::Uninstall, false, Some(std::time::Instant::now())),
            job(JobKind::Install, true, Some(std::time::Instant::now())),
            job(JobKind::Install, false, Some(std::time::Instant::now())),
            job(JobKind::Install, false, None),
        ];
        assert_eq!(
            active_download(&jobs).map(|(position, total, _)| (position, total)),
            Some((2, 3))
        );
        jobs[2].done = true;
        assert!(active_download(&jobs).is_none());
    }

    #[test]
    fn progress_is_bounded_even_for_inconsistent_server_totals() {
        let mut job = Job {
            id: 0,
            kind: JobKind::Install,
            game_id: String::new(),
            name: String::new(),
            cancel: Default::default(),
            state: String::new(),
            done: false,
            succeeded: false,
            started: None,
            progress: None,
        };
        assert_eq!(job_fraction(&job), 0.0);
        job.progress = Some(crate::Progress {
            completed: 10,
            total: 0,
            message: String::new(),
        });
        assert_eq!(job_fraction(&job), 0.0);
        job.progress.as_mut().unwrap().total = 5;
        assert_eq!(job_fraction(&job), 1.0);
        job.done = true;
        assert_eq!(job_label(&job, Language::Fr), "ÉCHEC");
        assert_eq!(job_color(&job), DANGER);
        job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(job_label(&job, Language::Fr), "ANNULÉ");
        job.succeeded = true;
        job.progress = None;
        assert_eq!(job_fraction(&job), 1.0);
        assert_eq!(job_label(&job, Language::Fr), "INSTALLÉ");
        assert_eq!(job_color(&job), ACCENT);
    }
}
