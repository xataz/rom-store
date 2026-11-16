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

const CHARACTERS: &str = "abcdefghijklmnopqrstuvwxyz0123456789 -'&";
const COLUMNS: usize = 10;
const MAX_QUERY: usize = 256;

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
}

impl SearchDialog {
    pub fn new(query: String, scope: Option<String>, platform: Option<String>) -> Self {
        Self {
            query: query.chars().take(MAX_QUERY).collect(),
            scope,
            platform,
            selected: 0,
        }
    }

    fn append(&mut self, text: &str) {
        let remaining = MAX_QUERY.saturating_sub(self.query.chars().count());
        self.query.extend(
            text.chars()
                .filter(|char| !char.is_control())
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
        let characters = CHARACTERS.chars().count();
        let count = characters + 3;
        match action {
            Action::Up if self.selected >= COLUMNS => self.selected -= COLUMNS,
            Action::Down if self.selected / COLUMNS < (count - 1) / COLUMNS => {
                self.selected = (self.selected + COLUMNS).min(count - 1)
            }
            Action::Left => self.selected = self.selected.saturating_sub(1),
            Action::Right => self.selected = (self.selected + 1).min(count - 1),
            Action::Confirm => {
                if let Some(char) = CHARACTERS.chars().nth(self.selected) {
                    self.append(&char.to_string());
                } else if self.selected == characters {
                    self.query.pop();
                } else if self.selected == characters + 1 {
                    self.query.clear();
                } else {
                    return Some(SearchOutcome::Submit);
                }
            }
            Action::Back => return Some(SearchOutcome::Cancel),
            Action::Secondary => self.toggle_scope(),
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
                Keycode::Escape if !repeat => return Some(SearchOutcome::Cancel),
                Keycode::Return if !repeat => return Some(SearchOutcome::Submit),
                Keycode::Backspace => {
                    self.query.pop();
                }
                Keycode::Delete => self.query.clear(),
                Keycode::Tab if !repeat => self.toggle_scope(),
                Keycode::Up => return self.action(Action::Up),
                Keycode::Down => return self.action(Action::Down),
                Keycode::Left => return self.action(Action::Left),
                Keycode::Right => return self.action(Action::Right),
                _ => (),
            },
            Event::ControllerButtonDown { button, .. } => match button {
                Button::A => return self.action(Action::Confirm),
                Button::B | Button::LeftStick => return Some(SearchOutcome::Cancel),
                Button::Start => return Some(SearchOutcome::Submit),
                Button::X => {
                    self.query.pop();
                }
                Button::Y => self.toggle_scope(),
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
    ) -> Result<()> {
        ui::dim(canvas, 205)?;
        ui::panel(canvas, Rect::new(200, 115, 880, 510), true)?;
        painter.text(canvas, l.text("search_title"), (236, 138), 30, TEXT, 800)?;
        let scope = self.scope.as_ref().map_or_else(
            || l.text("all_platforms").to_owned(),
            |id| l.format("search_platform", &[id]),
        );
        painter.text(canvas, &scope, (238, 184), 17, ACCENT, 800)?;
        ui::rounded(canvas, Rect::new(236, 220, 808, 50), 10, SURFACE)?;
        let mut tail = self.query.clone();
        while painter.width(&tail, 24) > 746.0 {
            tail.remove(0);
        }
        painter.text(canvas, &format!("{tail}_"), (252, 232), 24, TEXT, 774)?;
        let characters = CHARACTERS.chars().count();
        for index in 0..characters + 3 {
            let x = 236 + (index % COLUMNS) as i32 * 81;
            let y = 292 + (index / COLUMNS) as i32 * 48;
            let selected = self.selected == index;
            ui::rounded(
                canvas,
                Rect::new(x, y, 72, 39),
                7,
                if selected { ACCENT } else { BORDER },
            )?;
            let label = match CHARACTERS.chars().nth(index) {
                Some(' ') => l.text("space_key").to_owned(),
                Some(char) => char.to_uppercase().collect(),
                None if index == characters => "←".into(),
                None if index == characters + 1 => l.text("clear_search").into(),
                None => "OK".into(),
            };
            painter.text(
                canvas,
                &label,
                (x + 10, y + 9),
                if label.chars().count() > 2 { 13 } else { 17 },
                if selected { ui::BG } else { TEXT },
                54,
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
            if self.platform.is_some() {
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
        dialog.action(Action::Secondary);
        assert_eq!(dialog.scope, None);
        dialog.action(Action::Secondary);
        assert_eq!(dialog.scope.as_deref(), Some("nes"));
        dialog.selected = CHARACTERS.chars().count() + 2;
        assert_eq!(dialog.action(Action::Confirm), Some(SearchOutcome::Submit));
        assert_eq!(dialog.action(Action::Back), Some(SearchOutcome::Cancel));
    }
}
