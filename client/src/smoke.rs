//! Deterministic, network-free scenes for the SDL rendering smoke test.
use crate::{
    smoke_page, App, File, Job, JobKind, Painter, Progress, Screen, System, UninstallConfirmation,
    ViewMode,
};
use anyhow::Result;
use image::{Rgba, RgbaImage};
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

pub const SCENES: usize = 26;

fn media(id: char, role: &str) -> File {
    File {
        id: id.to_string().repeat(64),
        role: role.into(),
        destination: format!("images/{role}.png"),
        size: 0,
        sha256: "0".repeat(64),
    }
}

fn artwork(index: usize) -> RgbaImage {
    let palettes = [
        [83, 106, 208],
        [55, 163, 151],
        [182, 86, 124],
        [203, 143, 67],
    ];
    let color = palettes[index % palettes.len()];
    RgbaImage::from_fn(320, 220, |x, y| {
        // Original synthetic pixel landscapes, not commercial game artwork.
        let x = x as i32 / 4;
        let y = y as i32 / 4;
        let moon = (x - 57).pow(2) + (y - 14).pow(2) < 70;
        let mountain = y > 45 - ((x + index as i32 * 7) % 38 - 19).abs();
        let front = y > 54 - ((x + 23) % 54 - 27).abs() / 2;
        let pixel = if moon {
            [248, 220, 162]
        } else if front {
            [17, 33, 54]
        } else if mountain {
            [color[0] / 2, color[1] / 2, color[2] / 2]
        } else {
            [color[0], color[1], color[2]]
        };
        Rgba([pixel[0], pixel[1], pixel[2], 255])
    })
}

fn cover(index: usize) -> RgbaImage {
    let art = artwork(index);
    match index % 3 {
        0 => image::imageops::rotate90(&art),
        1 => art,
        _ => image::imageops::crop_imm(&art, 50, 0, 220, 220).to_image(),
    }
}

fn logo() -> RgbaImage {
    // Original 5×7 pixel lettering, with a transparent background.
    let glyphs = [
        [30, 17, 17, 30, 20, 18, 17],
        [14, 17, 17, 17, 17, 17, 14],
        [17, 27, 21, 17, 17, 17, 17],
        [0; 7],
        [15, 16, 16, 14, 1, 1, 30],
        [31, 4, 4, 4, 4, 4, 4],
        [14, 17, 17, 17, 17, 17, 14],
        [30, 17, 17, 30, 20, 18, 17],
        [31, 16, 16, 30, 16, 16, 31],
    ];
    RgbaImage::from_fn(432, 56, |x, y| {
        let column = (x / 8 % 6) as usize;
        if column < 5 && glyphs[(x / 48) as usize][(y / 8) as usize] & (1 << (4 - column)) != 0 {
            Rgba([105, 235, 196, 255])
        } else {
            Rgba([0, 0, 0, 0])
        }
    })
}

fn mix_image(index: usize) -> RgbaImage {
    let mut image = RgbaImage::from_pixel(800, 600, Rgba([0, 0, 0, 0]));
    let screenshot = image::imageops::resize(
        &artwork(index),
        740,
        480,
        image::imageops::FilterType::Triangle,
    );
    let cover = image::imageops::resize(
        &cover(index),
        170,
        240,
        image::imageops::FilterType::Triangle,
    );
    image::imageops::overlay(&mut image, &screenshot, 60, 0);
    image::imageops::overlay(&mut image, &cover, 24, 330);
    image::imageops::overlay(&mut image, &logo(), 300, 530);
    image
}

pub fn prepare(app: &mut App, painter: &mut Painter, frame: usize) -> Result<&'static str> {
    let l = app.config.language;
    app.loading = false;
    app.reloading = false;
    app.uninstall_confirmation = None;
    match frame {
        0 => {
            app.catalog_system = Some("nes".into());
            app.config.games_view = ViewMode::Grid;
            app.config.systems_view = ViewMode::List;
            let mut page = smoke_page();
            let names = [
                "Les voyageurs de l'aube",
                "Le dernier archipel",
                "Circuit des étoiles",
                "La cité oubliée",
                "Au-delà des nuages",
                "L'odyssée des lucioles",
                "Les gardiens du phare",
                "Voyage en pixel",
                "Les sentiers du vent",
                "La vallée des comètes",
                "Le secret de la forêt",
                "Une aventure au nom particulièrement long pour vérifier la troncature",
            ];
            for (index, game) in page.items.iter_mut().enumerate() {
                game.installed = index == 0 || index == 4;
                game.name = names[index].into();
                game.metadata.insert("desc".into(), "Partez à la découverte d'un monde oublié, entre montagnes lumineuses et cités suspendues. Explorez des chemins secrets, relevez de nouveaux défis et retrouvez le plaisir d'une aventure à votre rythme. Une création de démonstration pour imaginer votre prochaine partie.".into());
                game.metadata
                    .insert("developer".into(), "Studio Démonstration".into());
                game.metadata
                    .insert("genre".into(), "Aventure / Exploration".into());
                game.metadata.insert("players".into(), "1–2".into());
                game.files.push(File {
                    id: format!("{:064x}", 0x1000 + index),
                    ..media('a', "thumbnail")
                });
                if index != 3 {
                    painter.set_thumbnail(game.id.clone(), cover(index))?;
                }
            }
            page.items[0].files.extend([
                media('b', "image"),
                media('d', "marquee"),
                media('e', "fanart"),
            ]);
            app.systems = [
                ("nes", "NES"),
                ("snes", "Super Nintendo"),
                ("megadrive", "Mega Drive"),
                ("psx", "PlayStation"),
                ("gba", "Game Boy Advance"),
                ("pcengine", "PC Engine"),
                ("neogeo", "Neo Geo"),
                (
                    "demo-unknown",
                    "Nintendo Entertainment System — nom très long",
                ),
            ]
            .iter()
            .enumerate()
            .map(|(index, (id, name))| System {
                id: (*id).into(),
                name: (*name).into(),
                game_count: 1280 + index as u64 * 82,
                image: Some(media('c', "image")),
            })
            .collect();
            app.page = Some(page);
            app.screen = Screen::Games;
            app.status = l.text("navigation_hint").into();
        }
        1 => {
            app.game_index = 11;
        }
        2 => {
            app.screen = Screen::Detail;
            app.detail = app
                .page
                .as_ref()
                .and_then(|page| page.items.first())
                .cloned();
            painter.set_art(Some(mix_image(0)))?;
            painter.set_preview("b".repeat(64), artwork(2))?;
            painter.set_marquee("d".repeat(64), logo())?;
            app.status = l.text("add_to_queue_hint").into();
        }
        3 => {
            app.screen = Screen::Systems;
            painter.set_preview("c".repeat(64), artwork(1))?;
            app.status = l.text("choose_system").into();
        }
        4 => {
            app.system_index = 7;
            app.systems[7].image = None;
        }
        5 => {
            app.screen = Screen::Queue;
            app.status = l.text("smoke_queue_hint").into();
        }
        6 => {
            for (index, (name, state, completed)) in [
                ("Les voyageurs de l'aube", l.text("installing"), 42),
                ("Les gardiens du phare", l.text("waiting"), 0),
                ("La vallée des comètes", l.text("installed_hint"), 100),
                ("Le dernier archipel", l.text("transfer_interrupted"), 15),
                ("Voyage en pixel", l.text("waiting"), 0),
            ]
            .iter()
            .enumerate()
            {
                app.jobs.push(Job {
                    kind: JobKind::Install,
                    game_id: format!("{index:064x}"),
                    name: (*name).into(),
                    cancel: Arc::new(AtomicBool::new(false)),
                    state: (*state).into(),
                    done: index == 2 || index == 3,
                    succeeded: index == 2,
                    started: (index != 1 && index != 4)
                        .then(|| Instant::now() - Duration::from_secs(8)),
                    progress: Some(Progress {
                        completed: completed * 1024 * 1024,
                        total: 100 * 1024 * 1024,
                        message: (*state).into(),
                    }),
                });
            }
        }
        7 => {
            app.queue_index = 3;
        }
        8 => {
            app.screen = Screen::Games;
            app.loading = true;
            app.status = l.text("loading_catalog").into();
        }
        9 => {
            app.page.as_mut().unwrap().items.clear();
            app.status = l.text("no_games").into();
        }
        10 => {
            app.screen = Screen::Detail;
            let game = app.detail.as_mut().unwrap();
            game.files
                .retain(|file| file.role != "fanart" && file.role != "marquee");
            game.installable = false;
            game.installed = false;
            game.unavailable_reason = Some(l.text("unsupported_format").into());
            game.metadata.insert("desc".into(), "Une description volontairement longue pour vérifier que les textes restent dans leur panneau et se terminent par des points de suspension. ".repeat(12));
            painter.set_art(None)?;
            app.status = l.text("catalog_retry").into();
        }
        11 => {
            app.screen = Screen::Games;
            app.config.games_view = ViewMode::List;
            let mut page = smoke_page();
            page.items[0] = app.detail.as_ref().unwrap().clone();
            page.items[0].installable = true;
            page.items[0].files.push(media('d', "marquee"));
            page.items[0]
                .metadata
                .insert("desc".into(), l.text("no_description").into());
            app.page = Some(page);
            app.game_index = 0;
            painter.set_preview("b".repeat(64), artwork(2))?;
            app.status = l.text("navigation_hint").into();
        }
        12 => {
            app.game_index = 11;
        }
        13 => {
            app.screen = Screen::Systems;
            app.system_index = 0;
            app.config.systems_view = ViewMode::Grid;
            for (index, system) in app.systems.iter().enumerate() {
                if index != 7 {
                    painter.set_thumbnail(format!("system:{}", system.id), artwork(index))?;
                }
            }
            app.status = l.text("choose_system").into();
        }
        14 => {
            app.system_index = 7;
        }
        15 => {
            app.open_search();
            app.search.as_mut().unwrap().query = "Mario & Luigi".into();
        }
        16 => {
            app.search = None;
            app.screen = Screen::Games;
            app.catalog_system = Some("nes".into());
            app.open_search();
            app.search.as_mut().unwrap().query = "Éclair & Mario".into();
        }
        17 => {
            app.search = None;
            app.catalog_system = None;
            app.search_query = "voyageurs".into();
            app.config.games_view = ViewMode::Grid;
            let mut page = smoke_page();
            page.items.truncate(2);
            page.items[0].name = "Les voyageurs de l'aube".into();
            page.items[1].name = "Les voyageurs d'ailleurs".into();
            page.items[1].system = "snes".into();
            page.total = 2;
            app.page = Some(page);
            app.game_index = 0;
            app.status = l.text("navigation_hint").into();
        }
        18 => {
            let page = app.page.as_mut().unwrap();
            page.items.clear();
            page.total = 0;
            app.search_query = "aucun résultat".into();
        }
        19 => {
            app.search_query.clear();
            app.screen = Screen::Games;
            app.config.games_view = ViewMode::Grid;
            let mut page = smoke_page();
            page.total = 200;
            page.items[0].installed = true;
            app.page = Some(page);
            app.catalog_pending = Some(12);
            app.game_index = 0;
        }
        20 => {
            app.catalog_pending = None;
            app.config.games_view = ViewMode::List;
        }
        21 => {
            app.reloading = true;
        }
        22 => {
            app.screen = Screen::Detail;
            let mut game = app.page.as_ref().unwrap().items[0].clone();
            game.name = "Les voyageurs de l'aube".into();
            game.id = "f".repeat(64);
            game.installed = true;
            app.detail = Some(game);
            app.status = l.text("uninstall_hint").into();
            painter.set_art(Some(mix_image(0)))?;
        }
        23 | 24 => {
            app.uninstall_confirmation = Some(UninstallConfirmation {
                game: app.detail.as_ref().unwrap().clone(),
                confirmed: frame == 24,
            });
        }
        25 => {
            app.screen = Screen::Queue;
            app.queue_index = app.jobs.len();
            app.jobs.push(Job {
                kind: JobKind::Uninstall,
                game_id: "0".repeat(64),
                name: l.format("uninstall_job", &[&"Les voyageurs de l'aube"]),
                cancel: Arc::new(AtomicBool::new(false)),
                state: l.text("uninstalled_hint").into(),
                done: true,
                succeeded: true,
                started: Some(Instant::now()),
                progress: Some(Progress {
                    completed: 1,
                    total: 1,
                    message: String::new(),
                }),
            });
            app.status = l.text("uninstalled_hint").into();
        }
        _ => unreachable!(),
    }
    Ok([
        "collection",
        "collection-scroll",
        "detail",
        "platforms",
        "platforms-no-image",
        "queue-empty",
        "queue",
        "queue-error",
        "loading",
        "collection-empty",
        "detail-unavailable",
        "collection-list",
        "collection-list-scroll-no-screenshot",
        "platforms-grid",
        "platforms-grid-selection",
        "search-global-keyboard",
        "search-platform-keyboard",
        "search-global-results",
        "search-no-results",
        "infinite-scroll-loading",
        "installed-list",
        "catalogue-reloading",
        "detail-uninstall",
        "uninstall-confirm-cancel",
        "uninstall-confirm-remove",
        "uninstall-completed",
    ][frame])
}
