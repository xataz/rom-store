//! Draft configuration editor. Values only become active after an atomic save.
use crate::{
    input::Action,
    search::{KeyboardMode, SearchDialog, SearchOutcome},
    ui::{self, Painter, ACCENT, MUTED, TEXT},
};
use anyhow::{bail, Context, Result};
use romstore_client::{
    config::{Config, Frontend, ViewMode},
    i18n::Language,
};
use sdl2::{event::Event, rect::Rect, render::Canvas, video::Window};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    SystemsView,
    GamesView,
    ServerUrl,
    Port,
    RomsDir,
    MediaDir,
    GamelistsDir,
    CacheDir,
    FontPath,
    Fullscreen,
    Language,
    Frontend,
    AutoRefresh,
    Mapping,
    Command,
}

const FIELDS: &[Field] = &[
    Field::SystemsView,
    Field::GamesView,
    Field::ServerUrl,
    Field::Port,
    Field::RomsDir,
    Field::MediaDir,
    Field::GamelistsDir,
    Field::CacheDir,
    Field::FontPath,
    Field::Fullscreen,
    Field::Language,
    Field::Frontend,
    Field::AutoRefresh,
    Field::Mapping,
    Field::Command,
];

impl Field {
    fn key(self) -> &'static str {
        match self {
            Self::SystemsView => "systems_view",
            Self::GamesView => "games_view",
            Self::ServerUrl => "server_url",
            Self::Port => "server_port",
            Self::RomsDir => "roms_dir",
            Self::MediaDir => "media_dir",
            Self::GamelistsDir => "gamelists_dir",
            Self::CacheDir => "cache_dir",
            Self::FontPath => "font_path",
            Self::Fullscreen => "fullscreen",
            Self::Language => "language",
            Self::Frontend => "frontend",
            Self::AutoRefresh => "auto_refresh",
            Self::Mapping => "system_mapping",
            Self::Command => "refresh_command",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Main,
    Mapping,
    Command,
}

#[derive(Clone)]
enum Target {
    Field(Field),
    MapKey,
    MapValue(String),
    Argument(usize),
}

pub struct SettingsDialog {
    pub draft: Config,
    path: PathBuf,
    original: Option<Vec<u8>>,
    panel: Panel,
    pub selected: usize,
    editing: Option<(Target, SearchDialog)>,
    pub error: Option<String>,
}

fn contents(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("Lecture de {} impossible", path.display()))
        }
    }
}

impl SettingsDialog {
    pub fn new(config: &Config, path: PathBuf) -> Result<Self> {
        let original = contents(&path)?;
        Ok(Self {
            draft: if original.is_some() {
                Config::load(&path)?
            } else {
                config.clone()
            },
            original,
            path,
            panel: Panel::Main,
            selected: 0,
            editing: None,
            error: None,
        })
    }

    pub fn text_input(&self) -> bool {
        self.editing.is_some()
    }

    pub fn save(&mut self) -> Result<()> {
        self.finish_input()?;
        self.draft.validate()?;
        if contents(&self.path)? != self.original {
            bail!("Le fichier a été modifié ailleurs. Recharger le fichier avant de sauvegarder.");
        }
        self.draft.save(&self.path)?;
        self.original = contents(&self.path)?;
        Ok(())
    }

    pub fn finish_input(&mut self) -> Result<()> {
        if self.editing.is_some() {
            self.edit_outcome(Some(SearchOutcome::Submit));
            if self.editing.is_some() {
                bail!("Terminer ou annuler la saisie du champ avant de fermer");
            }
        }
        Ok(())
    }

    fn count(&self) -> usize {
        match self.panel {
            Panel::Main => FIELDS.len() + 2,
            Panel::Mapping => self.draft.system_mapping.len() * 2 + 2,
            Panel::Command => self.draft.refresh_command.len() * 2 + 2,
        }
    }

    fn raw(&self, field: Field) -> String {
        match field {
            Field::ServerUrl => self.draft.server_url.clone(),
            Field::Port => reqwest::Url::parse(&self.draft.server_url)
                .ok()
                .and_then(|u| u.port())
                .map_or(String::new(), |p| p.to_string()),
            Field::RomsDir => self.draft.roms_dir.display().to_string(),
            Field::MediaDir => self
                .draft
                .media_dir
                .as_ref()
                .map_or(String::new(), |p| p.display().to_string()),
            Field::GamelistsDir => self
                .draft
                .gamelists_dir
                .as_ref()
                .map_or(String::new(), |p| p.display().to_string()),
            Field::CacheDir => self.draft.cache_dir.display().to_string(),
            Field::FontPath => self.draft.font_path.display().to_string(),
            _ => String::new(),
        }
    }

    fn editor(&mut self, target: Target, query: String, title: String, mode: KeyboardMode) {
        self.error = None;
        self.editing = Some((
            target,
            SearchDialog::entry(query, mode, 4096).context(title, "settings_input_hint"),
        ));
    }

    fn assign(&mut self, target: Target, value: &str) -> Result<()> {
        match target {
            Target::Field(field) => match field {
                Field::ServerUrl => self.draft.server_url = value.trim().into(),
                Field::Port => {
                    let mut url = reqwest::Url::parse(&self.draft.server_url)?;
                    let port = if value.is_empty() {
                        None
                    } else {
                        let port: u16 = value
                            .parse()
                            .context("Port attendu : entier entre 1 et 65535")?;
                        if port == 0 {
                            bail!("Port attendu : entier entre 1 et 65535");
                        }
                        Some(port)
                    };
                    url.set_port(port).map_err(|_| {
                        anyhow::anyhow!("Impossible de modifier le port de cette URL")
                    })?;
                    self.draft.server_url = url.to_string().trim_end_matches('/').into();
                }
                Field::RomsDir => self.draft.roms_dir = value.into(),
                Field::MediaDir => self.draft.media_dir = (!value.is_empty()).then(|| value.into()),
                Field::GamelistsDir => {
                    self.draft.gamelists_dir = (!value.is_empty()).then(|| value.into())
                }
                Field::CacheDir => self.draft.cache_dir = value.into(),
                Field::FontPath => self.draft.font_path = value.into(),
                _ => (),
            },
            Target::MapKey => {
                if value.is_empty() || self.draft.system_mapping.contains_key(value) {
                    bail!("Identifiant de plateforme vide ou déjà présent");
                }
                self.editor(
                    Target::MapValue(value.into()),
                    String::new(),
                    format!("system_mapping · {value}"),
                    KeyboardMode::Text,
                );
            }
            Target::MapValue(key) => {
                if value.is_empty() {
                    bail!("Le dossier local ne doit pas être vide");
                }
                self.draft.system_mapping.insert(key, value.into());
            }
            Target::Argument(index) => {
                if index == self.draft.refresh_command.len() {
                    self.draft.refresh_command.push(value.into());
                } else {
                    self.draft.refresh_command[index] = value.into();
                }
            }
        }
        Ok(())
    }

    fn edit_outcome(&mut self, outcome: Option<SearchOutcome>) {
        match outcome {
            Some(SearchOutcome::Cancel) => self.editing = None,
            Some(SearchOutcome::Submit) => {
                let (target, mut edit) = self.editing.take().unwrap();
                self.error = None;
                if let Err(error) = self.assign(target.clone(), &edit.query) {
                    self.error = Some(format!("{error:#}"));
                    edit.error = self.error.clone();
                    self.editing = Some((target, edit));
                }
            }
            None => (),
        }
    }

    pub fn event(&mut self, event: &Event) {
        if let Some((_, edit)) = &mut self.editing {
            let outcome = edit.event(event);
            self.edit_outcome(outcome);
        }
    }

    fn cycle(&mut self, field: Field, forward: bool) {
        fn next<T: Copy + PartialEq>(current: T, values: &[T], forward: bool) -> T {
            let i = values.iter().position(|v| *v == current).unwrap_or(0);
            values[(i + if forward { 1 } else { values.len() - 1 }) % values.len()]
        }
        match field {
            Field::SystemsView => self.draft.systems_view = self.draft.systems_view.toggled(),
            Field::GamesView => self.draft.games_view = self.draft.games_view.toggled(),
            Field::Fullscreen => self.draft.fullscreen = !self.draft.fullscreen,
            Field::AutoRefresh => self.draft.auto_refresh = !self.draft.auto_refresh,
            Field::Language => {
                self.draft.language = next(
                    self.draft.language,
                    &[Language::Fr, Language::En, Language::Es, Language::De],
                    forward,
                )
            }
            Field::Frontend => {
                self.draft.frontend = next(
                    self.draft.frontend,
                    &[
                        Frontend::None,
                        Frontend::Batocera,
                        Frontend::Esde,
                        Frontend::Recalbox,
                        Frontend::Emulationstation,
                        Frontend::Custom,
                    ],
                    forward,
                )
            }
            _ => (),
        }
        self.error = None;
    }

    fn confirm(&mut self) -> Result<bool> {
        match self.panel {
            Panel::Main => {
                if let Some(&field) = FIELDS.get(self.selected) {
                    match field {
                        Field::SystemsView
                        | Field::GamesView
                        | Field::Fullscreen
                        | Field::AutoRefresh
                        | Field::Language
                        | Field::Frontend => self.cycle(field, true),
                        Field::Mapping => {
                            self.panel = Panel::Mapping;
                            self.selected = 0;
                        }
                        Field::Command => {
                            self.panel = Panel::Command;
                            self.selected = 0;
                        }
                        _ => self.editor(
                            Target::Field(field),
                            self.raw(field),
                            field.key().into(),
                            if field == Field::Port {
                                KeyboardMode::Integer
                            } else {
                                KeyboardMode::Text
                            },
                        ),
                    }
                } else if self.selected == FIELDS.len() {
                    return Ok(true);
                } else {
                    self.draft = if self.path.exists() {
                        Config::load(&self.path)?
                    } else {
                        Config::default()
                    };
                    self.original = contents(&self.path)?;
                    self.error = None;
                }
            }
            Panel::Mapping => {
                let keys: Vec<_> = self.draft.system_mapping.keys().cloned().collect();
                if self.selected < keys.len() * 2 {
                    let key = keys[self.selected / 2].clone();
                    if self.selected.is_multiple_of(2) {
                        self.editor(
                            Target::MapValue(key.clone()),
                            self.draft.system_mapping[&key].clone(),
                            format!("system_mapping · {key}"),
                            KeyboardMode::Text,
                        );
                    } else {
                        self.draft.system_mapping.remove(&key);
                    }
                } else if self.selected == keys.len() * 2 {
                    self.editor(
                        Target::MapKey,
                        String::new(),
                        "system_mapping · ID".into(),
                        KeyboardMode::Text,
                    );
                } else {
                    self.panel = Panel::Main;
                    self.selected = FIELDS.iter().position(|f| *f == Field::Mapping).unwrap();
                }
            }
            Panel::Command => {
                let len = self.draft.refresh_command.len();
                if self.selected < len * 2 {
                    let index = self.selected / 2;
                    if self.selected.is_multiple_of(2) {
                        self.editor(
                            Target::Argument(index),
                            self.draft.refresh_command[index].clone(),
                            format!("refresh_command [{index}]"),
                            KeyboardMode::Text,
                        );
                    } else {
                        self.draft.refresh_command.remove(index);
                    }
                } else if self.selected == len * 2 {
                    self.editor(
                        Target::Argument(len),
                        String::new(),
                        format!("refresh_command [{len}]"),
                        KeyboardMode::Text,
                    );
                } else {
                    self.panel = Panel::Main;
                    self.selected = FIELDS.iter().position(|f| *f == Field::Command).unwrap();
                }
            }
        }
        self.selected = self.selected.min(self.count() - 1);
        Ok(false)
    }

    /// True asks App to validate, persist and close the entire submenu.
    pub fn action(&mut self, action: Action) -> bool {
        if let Some((_, edit)) = &mut self.editing {
            let outcome = edit.action(action);
            self.edit_outcome(outcome);
            return false;
        }
        match action {
            Action::Up | Action::Down => {
                self.selected = crate::move_grid_selection(self.selected, self.count(), action, 1)
            }
            Action::Back if self.panel == Panel::Main => return true,
            Action::Back => {
                self.panel = Panel::Main;
                self.selected = 0;
            }
            Action::Left | Action::Right
                if self.panel == Panel::Main && self.selected < FIELDS.len() =>
            {
                self.cycle(FIELDS[self.selected], action == Action::Right)
            }
            Action::Confirm => match self.confirm() {
                Ok(close) => return close,
                Err(error) => self.error = Some(format!("{error:#}")),
            },
            _ => (),
        }
        false
    }

    fn value(&self, field: Field, l: Language) -> String {
        match field {
            Field::SystemsView | Field::GamesView => l
                .text(
                    if (if field == Field::SystemsView {
                        self.draft.systems_view
                    } else {
                        self.draft.games_view
                    }) == ViewMode::Grid
                    {
                        "grid_view"
                    } else {
                        "list_view"
                    },
                )
                .into(),
            Field::Fullscreen | Field::AutoRefresh => l
                .text(
                    if if field == Field::Fullscreen {
                        self.draft.fullscreen
                    } else {
                        self.draft.auto_refresh
                    } {
                        "settings_enabled"
                    } else {
                        "settings_disabled"
                    },
                )
                .into(),
            Field::Language => serde_json::to_value(self.draft.language)
                .unwrap()
                .as_str()
                .unwrap()
                .into(),
            Field::Frontend => serde_json::to_value(self.draft.frontend)
                .unwrap()
                .as_str()
                .unwrap()
                .into(),
            Field::Mapping => format!("{} →", self.draft.system_mapping.len()),
            Field::Command => format!("{} →", self.draft.refresh_command.len()),
            _ => {
                let value = self.raw(field);
                if value.is_empty() {
                    l.text("settings_unset").into()
                } else {
                    value
                }
            }
        }
    }

    fn rows(&self, l: Language) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        match self.panel {
            Panel::Main => {
                rows.extend(FIELDS.iter().map(|&field| {
                    (
                        (match field {
                            Field::SystemsView => l.text("menu_systems_view"),
                            Field::GamesView => l.text("menu_games_view"),
                            _ => field.key(),
                        })
                        .into(),
                        self.value(field, l),
                    )
                }));
                rows.push((l.text("settings_save_close").into(), String::new()));
                rows.push((l.text("settings_reload").into(), String::new()));
            }
            Panel::Mapping => {
                for (key, value) in &self.draft.system_mapping {
                    rows.push((key.clone(), value.clone()));
                    rows.push((
                        format!("{} · {key}", l.text("settings_remove")),
                        String::new(),
                    ));
                }
                rows.push((l.text("settings_add_mapping").into(), String::new()));
                rows.push((l.text("back").into(), String::new()));
            }
            Panel::Command => {
                for (index, value) in self.draft.refresh_command.iter().enumerate() {
                    rows.push((
                        format!(
                            "[{index}] {}",
                            l.text(if index == 0 {
                                "settings_executable"
                            } else {
                                "settings_argument"
                            })
                        ),
                        value.clone(),
                    ));
                    rows.push((
                        format!("{} [{index}]", l.text("settings_remove")),
                        String::new(),
                    ));
                }
                rows.push((l.text("settings_add_argument").into(), String::new()));
                rows.push((l.text("back").into(), String::new()));
            }
        }
        rows
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas<Window>,
        painter: &mut Painter,
        l: Language,
    ) -> Result<()> {
        ui::dim(canvas, 220)?;
        ui::panel(canvas, Rect::new(180, 90, 920, 540), true)?;
        painter.text(canvas, l.text("settings_title"), (210, 108), 30, TEXT, 860)?;
        painter.text(
            canvas,
            &self.path.display().to_string(),
            (212, 151),
            14,
            MUTED,
            856,
        )?;
        painter.text(
            canvas,
            self.error.as_deref().unwrap_or(l.text("settings_help")),
            (212, 181),
            14,
            if self.error.is_some() {
                ui::DANGER
            } else {
                MUTED
            },
            856,
        )?;
        let rows = self.rows(l);
        let start = self.selected.saturating_sub(6);
        for (index, (label, value)) in rows.iter().enumerate().skip(start).take(7) {
            let y = 214 + (index - start) as i32 * 48;
            let selected = index == self.selected;
            ui::rounded(
                canvas,
                Rect::new(210, y, 860, 42),
                8,
                if selected { ui::BORDER } else { ui::SURFACE },
            )?;
            painter.text(
                canvas,
                label,
                (226, y + 9),
                19,
                if selected { ACCENT } else { TEXT },
                if value.is_empty() { 824 } else { 350 },
            )?;
            painter.text(canvas, value, (590, y + 10), 18, MUTED, 460)?;
        }
        painter.text(
            canvas,
            l.text("settings_controls"),
            (212, 580),
            14,
            MUTED,
            856,
        )?;
        if let Some((_, edit)) = &self.editing {
            edit.draw(canvas, painter, l, None)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_and_collection_editors_wrap() {
        let root = tempfile::tempdir().unwrap();
        let mut dialog =
            SettingsDialog::new(&Config::default(), root.path().join("config.yaml")).unwrap();
        for panel in [Panel::Main, Panel::Mapping, Panel::Command] {
            dialog.panel = panel;
            dialog.selected = 0;
            dialog.action(Action::Up);
            assert_eq!(dialog.selected, dialog.count() - 1);
            dialog.action(Action::Down);
            assert_eq!(dialog.selected, 0);
        }
    }

    fn select(dialog: &mut SettingsDialog, field: Field) {
        dialog.panel = Panel::Main;
        dialog.selected = FIELDS.iter().position(|f| *f == field).unwrap();
        dialog.action(Action::Confirm);
    }

    fn type_value(dialog: &mut SettingsDialog, text: &str) {
        dialog.editing.as_mut().unwrap().1.query = text.into();
        dialog.edit_outcome(Some(SearchOutcome::Submit));
    }

    #[test]
    fn menu_covers_every_serialized_config_field() {
        let value = serde_json::to_value(Config::default()).unwrap();
        let actual: std::collections::BTreeSet<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let fields: std::collections::BTreeSet<_> = FIELDS
            .iter()
            .filter(|f| **f != Field::Port)
            .map(|f| f.key())
            .collect();
        assert_eq!(actual, fields);
    }

    #[test]
    fn port_uses_digits_and_rejects_out_of_range_values_without_changing_url() {
        let root = tempfile::tempdir().unwrap();
        let mut dialog =
            SettingsDialog::new(&Config::default(), root.path().join("config.yaml")).unwrap();
        select(&mut dialog, Field::Port);
        dialog.editing.as_mut().unwrap().1.query.clear();
        dialog.event(&Event::TextInput {
            timestamp: 0,
            window_id: 0,
            text: "9abc9999".into(),
        });
        assert_eq!(dialog.editing.as_ref().unwrap().1.query, "99999");
        dialog.edit_outcome(Some(SearchOutcome::Submit));
        assert!(dialog.error.is_some());
        assert_eq!(dialog.draft.server_url, Config::default().server_url);
        type_value(&mut dialog, "6789");
        assert!(dialog.editing.is_none());
        assert_eq!(dialog.draft.server_url, "http://127.0.0.1:6789");
    }

    #[test]
    fn mappings_and_command_arguments_can_be_added_edited_removed_and_persisted() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.yaml");
        let mut dialog = SettingsDialog::new(&Config::default(), path.clone()).unwrap();
        select(&mut dialog, Field::Mapping);
        dialog.action(Action::Confirm);
        type_value(&mut dialog, "gamecube");
        type_value(&mut dialog, "gc");
        assert_eq!(dialog.draft.system_mapping["gamecube"], "gc");
        dialog.selected = 0;
        dialog.action(Action::Confirm);
        type_value(&mut dialog, "nintendogc");
        dialog.action(Action::Back);
        select(&mut dialog, Field::Command);
        dialog.action(Action::Confirm);
        type_value(&mut dialog, "/my refresh script");
        dialog.selected = 2;
        dialog.action(Action::Confirm);
        type_value(&mut dialog, "argument with spaces ; not a shell");
        dialog.selected = 4;
        dialog.action(Action::Confirm);
        type_value(&mut dialog, "to remove");
        dialog.selected = 5;
        dialog.action(Action::Confirm);
        dialog.save().unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.system_mapping["gamecube"], "nintendogc");
        assert_eq!(
            loaded.refresh_command,
            ["/my refresh script", "argument with spaces ; not a shell"]
        );
        dialog.action(Action::Back);
        select(&mut dialog, Field::Mapping);
        dialog.selected = 1;
        dialog.action(Action::Confirm);
        assert!(dialog.draft.system_mapping.is_empty());
    }

    #[test]
    fn invalid_or_conflicting_configuration_keeps_disk_intact_and_can_be_reloaded() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.yaml");
        Config::default().save(&path).unwrap();
        let original = fs::read(&path).unwrap();
        let mut dialog = SettingsDialog::new(&Config::default(), path.clone()).unwrap();
        dialog.draft.frontend = Frontend::Esde;
        assert!(dialog.save().is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        dialog.draft.frontend = Frontend::None;
        let external = Config {
            language: Language::En,
            ..Config::default()
        };
        external.save(&path).unwrap();
        assert!(dialog
            .save()
            .unwrap_err()
            .to_string()
            .contains("modifié ailleurs"));
        dialog.selected = FIELDS.len() + 1;
        dialog.action(Action::Confirm);
        assert_eq!(dialog.draft.language, Language::En);
        dialog.save().unwrap();
        assert_eq!(Config::load(&path).unwrap(), external);
    }
}
