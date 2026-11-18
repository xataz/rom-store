//! Unicode text entry plus a controller-operated keyboard; no requests until submit.
use crate::{
    input::Action,
    ui::{self, Painter, ACCENT, BORDER, GOLD, MUTED, SURFACE, TEXT},
};
use anyhow::Result;
use romstore_client::i18n::Language;
use sdl2::{
    controller::Button, event::Event, keyboard::Keycode, rect::Rect, render::Canvas, video::Window,
};

const CHARACTERS: &str = "abcdefghijklmnopqrstuvwxyz0123456789 -'&.=<>+_/:@\\~\"()[]{}!,;?%$";
const MAX_QUERY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardMode {
    Text,
    Integer,
    IntegerFilter,
    DecimalFilter,
}

impl KeyboardMode {
    fn characters(self) -> &'static str {
        match self {
            Self::Text => CHARACTERS,
            Self::Integer => "1234567890",
            Self::IntegerFilter => "1234567890=<>-+",
            Self::DecimalFilter => "1234567890.=<>",
        }
    }

    pub fn for_filter(field: &str) -> Self {
        match field {
            "year" | "players" | "playcount" | "gametime" | "timesplayed" => Self::IntegerFilter,
            "rating" => Self::DecimalFilter,
            _ => Self::Text,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum SearchOutcome {
    Submit,
    Cancel,
}

pub(super) struct SearchDialog {
    pub query: String,
    pub scope: Option<String>,
    platform: Option<String>,
    selected: usize,
    text_only: bool,
    mode: KeyboardMode,
    uppercase: bool,
    limit: usize,
    title: Option<String>,
    hint: &'static str,
    pub error: Option<String>,
}

impl SearchDialog {
    pub fn new(query: String, scope: Option<String>, platform: Option<String>) -> Self {
        Self {
            query: query.chars().take(MAX_QUERY).collect(),
            scope,
            platform,
            selected: 0,
            text_only: false,
            mode: KeyboardMode::Text,
            uppercase: false,
            limit: MAX_QUERY,
            title: None,
            hint: "advanced_help",
            error: None,
        }
    }

    pub fn entry(query: String, mode: KeyboardMode, limit: usize) -> Self {
        let mut entry = Self {
            text_only: true,
            mode,
            limit,
            ..Self::new(String::new(), None, None)
        };
        entry.append(&query);
        entry
    }

    pub fn context(mut self, title: String, hint: &'static str) -> Self {
        self.title = Some(title);
        self.hint = hint;
        self
    }

    fn columns(&self) -> usize {
        match self.mode {
            KeyboardMode::Text => 12,
            KeyboardMode::Integer => 3,
            _ => 4,
        }
    }

    fn characters(&self) -> &'static str {
        self.mode.characters()
    }

    fn has_scope(&self) -> bool {
        self.platform.is_some() && !self.text_only
    }

    fn key_count(&self) -> usize {
        self.characters().chars().count()
            + 3
            + usize::from(self.has_scope())
            + usize::from(self.mode == KeyboardMode::Text)
    }

    fn append(&mut self, text: &str) {
        let remaining = self.limit.saturating_sub(self.query.chars().count());
        self.query.extend(
            text.chars()
                .filter(|char| {
                    !char.is_control()
                        && (self.mode == KeyboardMode::Text
                            || self.mode.characters().contains(*char))
                })
                .take(remaining),
        );
    }

    fn toggle_scope(&mut self) {
        if self.platform.is_some() {
            self.scope = if self.scope.is_some() {
                None
            } else {
                self.platform.clone()
            };
        }
    }

    pub fn action(&mut self, action: Action) -> Option<SearchOutcome> {
        let characters = self.characters().chars().count();
        let count = self.key_count();
        let columns = self.columns();
        match action {
            Action::Up | Action::Down | Action::Left | Action::Right => {
                self.selected = crate::move_grid_selection(self.selected, count, action, columns);
            }
            Action::Confirm => {
                if let Some(char) = self.characters().chars().nth(self.selected) {
                    self.append(&if self.uppercase {
                        char.to_uppercase().collect()
                    } else {
                        char.to_string()
                    });
                } else if self.selected == characters {
                    self.query.pop();
                } else if self.selected == characters + 1 {
                    self.query.clear();
                } else if self.selected == characters + 2 {
                    return Some(SearchOutcome::Submit);
                } else if self.has_scope() && self.selected == characters + 3 {
                    self.toggle_scope();
                } else {
                    self.uppercase = !self.uppercase;
                }
            }
            Action::Back => return Some(SearchOutcome::Cancel),
            _ => (),
        }
        None
    }

    pub fn event(&mut self, event: &Event) -> Option<SearchOutcome> {
        match event {
            Event::TextInput { text, .. } => self.append(text),
            Event::KeyDown {
                keycode: Some(key),
                repeat,
                ..
            } => match *key {
                Keycode::Return if !repeat => return Some(SearchOutcome::Submit),
                Keycode::Backspace => {
                    self.query.pop();
                }
                Keycode::Delete => self.query.clear(),
                Keycode::Up => return self.action(Action::Up),
                Keycode::Down => return self.action(Action::Down),
                Keycode::Left => return self.action(Action::Left),
                Keycode::Right => return self.action(Action::Right),
                _ => (),
            },
            Event::ControllerButtonDown { button, .. } => match button {
                Button::A => return self.action(Action::Confirm),
                Button::B | Button::LeftStick => return Some(SearchOutcome::Cancel),
                _ => (),
            },
            _ => (),
        }
        None
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas<Window>,
        painter: &mut Painter,
        l: Language,
        scope_name: Option<&str>,
    ) -> Result<()> {
        ui::dim(canvas, 205)?;
        ui::panel(canvas, Rect::new(200, 115, 880, 510), true)?;
        painter.text(
            canvas,
            self.title.as_deref().unwrap_or(l.text(if self.text_only {
                "filter_value"
            } else {
                "search_title"
            })),
            (236, 138),
            30,
            TEXT,
            800,
        )?;
        let scope = self.scope.as_ref().map_or_else(
            || l.text("all_platforms").to_owned(),
            |id| l.format("search_platform", &[&scope_name.unwrap_or(id)]),
        );
        painter.text(
            canvas,
            if self.text_only {
                l.text(self.hint)
            } else {
                &scope
            },
            (238, 184),
            17,
            ACCENT,
            800,
        )?;
        ui::rounded(canvas, Rect::new(236, 220, 808, 50), 10, SURFACE)?;
        // Settings may contain long paths/arguments: bound the per-frame work.
        let mut visible: Vec<_> = self.query.chars().rev().take(256).collect();
        visible.reverse();
        let mut tail: String = visible.into_iter().collect();
        while painter.width(&tail, 24) > 746.0 {
            tail.remove(0);
        }
        painter.text(canvas, &format!("{tail}_"), (252, 232), 24, TEXT, 774)?;
        let characters = self.characters().chars().count();
        let columns = self.columns();
        let step = if self.mode == KeyboardMode::Text {
            67
        } else {
            120
        };
        let origin = if self.mode == KeyboardMode::Text {
            236
        } else {
            (1280 - columns as i32 * step) / 2
        };
        for index in 0..self.key_count() {
            let x = origin + (index % columns) as i32 * step;
            let y = 292 + (index / columns) as i32 * 40;
            let selected = self.selected == index;
            ui::rounded(
                canvas,
                Rect::new(x, y, (step - 8) as u32, 39),
                7,
                if selected { ACCENT } else { BORDER },
            )?;
            let label = match self.characters().chars().nth(index) {
                Some(' ') => l.text("space_key").to_owned(),
                Some(char) => {
                    if self.uppercase {
                        char.to_uppercase().collect()
                    } else {
                        char.to_string()
                    }
                }
                None if index == characters => "←".into(),
                None if index == characters + 1 => l.text("clear_search").into(),
                None if index == characters + 2 => "OK".into(),
                None if self.has_scope() && index == characters + 3 => {
                    l.text("filter_platform").into()
                }
                None => if self.uppercase { "ABC" } else { "abc" }.into(),
            };
            painter.text(
                canvas,
                &label,
                (x + 10, y + 9),
                if label.chars().count() > 2 { 13 } else { 17 },
                if selected { ui::BG } else { TEXT },
                (step - 24) as u32,
            )?;
        }
        painter.text(
            canvas,
            l.text("search_keyboard_hint"),
            (238, 552),
            14,
            MUTED,
            800,
        )?;
        painter.text(
            canvas,
            if let Some(error) = &self.error {
                error
            } else if self.text_only {
                l.text(if self.mode == KeyboardMode::Text {
                    self.hint
                } else {
                    "numeric_keyboard_hint"
                })
            } else if self.platform.is_some() {
                l.text("search_scope_hint")
            } else {
                l.text("search_global_hint")
            },
            (238, 584),
            14,
            GOLD,
            800,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_keyboard_wraps_at_the_edges() {
        let mut dialog = SearchDialog::entry(String::new(), KeyboardMode::Text, 256);
        dialog.action(Action::Left);
        assert_eq!(dialog.selected, dialog.key_count() - 1);
        dialog.action(Action::Right);
        assert_eq!(dialog.selected, 0);
        dialog.action(Action::Up);
        assert!(dialog.selected >= (dialog.key_count() - 1) / dialog.columns() * dialog.columns());
        dialog.action(Action::Down);
        assert_eq!(dialog.selected, 0);
    }

    #[test]
    fn numeric_keypads_reject_letters_from_both_typing_and_paste() {
        for (mode, input, expected) in [
            (KeyboardMode::Integer, "port: 8000abc", "8000"),
            (KeyboardMode::IntegerFilter, ">=1990abc", ">=1990"),
            (KeyboardMode::IntegerFilter, "1-4 joueurs", "1-4"),
            (KeyboardMode::DecimalFilter, ">=0.8stars", ">=0.8"),
        ] {
            let mut dialog = SearchDialog::entry(String::new(), mode, 256);
            dialog.event(&Event::TextInput {
                timestamp: 0,
                window_id: 0,
                text: input.into(),
            });
            assert_eq!(dialog.query, expected);
            assert!(!dialog.characters().chars().any(|c| c.is_alphabetic()));
            assert!(dialog.key_count().div_ceil(dialog.columns()) <= 6);
        }
        assert_eq!(
            KeyboardMode::for_filter("year"),
            KeyboardMode::IntegerFilter
        );
        assert_eq!(
            KeyboardMode::for_filter("players"),
            KeyboardMode::IntegerFilter
        );
        assert_eq!(KeyboardMode::for_filter("name"), KeyboardMode::Text);
    }

    #[test]
    fn text_keyboard_can_type_uppercase_and_preserves_long_existing_values() {
        let value = format!("/Games/{}", "A".repeat(500));
        let dialog = SearchDialog::entry(value.clone(), KeyboardMode::Text, 4096);
        assert_eq!(dialog.query, value);
        assert!(dialog.key_count().div_ceil(dialog.columns()) <= 6);
        let mut dialog = SearchDialog::new(String::new(), None, None);
        dialog.selected = dialog.key_count() - 1;
        dialog.action(Action::Confirm);
        dialog.selected = 0;
        dialog.action(Action::Confirm);
        assert_eq!(dialog.query, "A");
    }

    #[test]
    fn unicode_input_is_bounded_and_backspace_removes_a_character() {
        let mut dialog = SearchDialog::new(String::new(), None, None);
        dialog.append("Éclair & Mario\n");
        assert_eq!(dialog.query, "Éclair & Mario");
        dialog.append(&"é".repeat(300));
        assert_eq!(dialog.query.chars().count(), 256);
        dialog.event(&Event::KeyDown {
            timestamp: 0,
            window_id: 0,
            keycode: Some(Keycode::Backspace),
            scancode: None,
            keymod: sdl2::keyboard::Mod::NOMOD,
            repeat: false,
        });
        assert_eq!(dialog.query.chars().count(), 255);
    }

    #[test]
    fn controller_keyboard_and_scope_work_without_a_physical_keyboard() {
        let mut dialog = SearchDialog::new(String::new(), Some("nes".into()), Some("nes".into()));
        dialog.action(Action::Confirm);
        assert_eq!(dialog.query, "a");
        dialog.toggle_scope();
        assert_eq!(dialog.scope, None);
        dialog.toggle_scope();
        assert_eq!(dialog.scope.as_deref(), Some("nes"));
        dialog.selected = CHARACTERS.chars().count() + 2;
        assert_eq!(dialog.action(Action::Confirm), Some(SearchOutcome::Submit));
        assert_eq!(dialog.action(Action::Back), Some(SearchOutcome::Cancel));
    }
}
