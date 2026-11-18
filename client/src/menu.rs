//! The only entry point for settings and secondary application actions.
use crate::{
    ui::{self, Painter, ACCENT, MUTED, TEXT},
    ViewMode,
};
use anyhow::Result;
use romstore_client::{config::Config, i18n::Language};
use sdl2::{rect::Rect, render::Canvas, video::Window};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    SystemsView,
    GamesView,
    Downloads,
    AdvancedSearch,
    Reload,
    Refresh,
    Quit,
}

#[derive(Default)]
pub struct Menu {
    pub selected: usize,
}

impl Menu {
    pub fn items(refresh: bool) -> Vec<Item> {
        let mut items = vec![
            Item::SystemsView,
            Item::GamesView,
            Item::Downloads,
            Item::AdvancedSearch,
            Item::Reload,
        ];
        if refresh {
            items.push(Item::Refresh);
        }
        items.push(Item::Quit);
        items
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas<Window>,
        painter: &mut Painter,
        config: &Config,
        reloading: bool,
        refreshing: bool,
    ) -> Result<()> {
        let l: Language = config.language;
        ui::dim(canvas, 215)?;
        ui::panel(canvas, Rect::new(240, 90, 800, 530), true)?;
        painter.text(canvas, l.text("menu_title"), (276, 111), 30, TEXT, 728)?;
        for (index, item) in Self::items(config.auto_refresh).iter().enumerate() {
            let y = 172 + index as i32 * 52;
            let selected = self.selected == index;
            let label = l.text(match item {
                Item::SystemsView => "menu_systems_view",
                Item::GamesView => "menu_games_view",
                Item::Downloads => "queue",
                Item::AdvancedSearch => "advanced_title",
                Item::Reload => {
                    if reloading {
                        "reloading_catalog"
                    } else {
                        "menu_reload"
                    }
                }
                Item::Refresh => {
                    if refreshing {
                        "refresh_running"
                    } else {
                        "menu_refresh"
                    }
                }
                Item::Quit => "quit",
            });
            ui::rounded(
                canvas,
                Rect::new(270, y, 740, 44),
                8,
                if selected { ui::BORDER } else { ui::SURFACE },
            )?;
            painter.text(
                canvas,
                label,
                (286, y + 9),
                21,
                if selected { ACCENT } else { TEXT },
                540,
            )?;
            let view = match item {
                Item::SystemsView => Some(config.systems_view),
                Item::GamesView => Some(config.games_view),
                _ => None,
            };
            if let Some(view) = view {
                painter.text(
                    canvas,
                    l.text(if view == ViewMode::Grid {
                        "grid_view"
                    } else {
                        "list_view"
                    }),
                    (816, y + 13),
                    14,
                    ACCENT,
                    176,
                )?;
            }
        }
        painter.text(canvas, l.text("menu_controls"), (276, 580), 15, MUTED, 728)
    }
}
