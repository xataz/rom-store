//! Keyboard- and controller-operated initial selector.
use crate::{
    input::Action,
    ui::{self, Painter, ACCENT, BORDER, MUTED, TEXT},
};
use anyhow::Result;
use romstore_client::i18n::Language;
use sdl2::{event::Event, keyboard::Keycode, rect::Rect, render::Canvas, video::Window};

const LETTERS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const COLUMNS: usize = 10;

pub enum Outcome {
    Cancel,
    Select(char),
}

pub struct AlphabetDialog {
    selected: usize,
}

impl AlphabetDialog {
    pub fn new(name: &str) -> Self {
        let initial = name.chars().next().unwrap_or('A').to_ascii_uppercase();
        Self {
            selected: LETTERS.find(initial).unwrap_or(10),
        }
    }

    pub fn action(&mut self, action: Action) -> Option<Outcome> {
        match action {
            Action::Up if self.selected >= COLUMNS => self.selected -= COLUMNS,
            Action::Down if self.selected / COLUMNS < (LETTERS.len() - 1) / COLUMNS => {
                self.selected = (self.selected + COLUMNS).min(LETTERS.len() - 1)
            }
            Action::Left => self.selected = self.selected.saturating_sub(1),
            Action::Right => self.selected = (self.selected + 1).min(LETTERS.len() - 1),
            Action::Confirm => {
                return Some(Outcome::Select(LETTERS.as_bytes()[self.selected] as char))
            }
            Action::Back | Action::Alphabet => return Some(Outcome::Cancel),
            _ => (),
        }
        None
    }

    pub fn event(&mut self, event: &Event) -> Option<Outcome> {
        if let Event::KeyDown {
            keycode: Some(key),
            repeat: false,
            ..
        } = event
        {
            if *key == Keycode::Escape || *key == Keycode::Backspace {
                return Some(Outcome::Cancel);
            }
            let name = key.name().to_ascii_uppercase();
            if name.len() == 1 && LETTERS.contains(&name) {
                return name.chars().next().map(Outcome::Select);
            }
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
        ui::panel(canvas, Rect::new(200, 155, 880, 410), true)?;
        painter.text(canvas, l.text("alphabet_title"), (236, 180), 30, TEXT, 808)?;
        for (index, letter) in LETTERS.chars().enumerate() {
            let x = 236 + (index % COLUMNS) as i32 * 81;
            let y = 250 + (index / COLUMNS) as i32 * 56;
            let selected = index == self.selected;
            ui::rounded(
                canvas,
                Rect::new(x, y, 72, 44),
                7,
                if selected { ACCENT } else { BORDER },
            )?;
            painter.text(
                canvas,
                &letter.to_string(),
                (x + 25, y + 9),
                22,
                if selected { ui::BG } else { TEXT },
                40,
            )?;
        }
        painter.text(
            canvas,
            l.text("alphabet_controls"),
            (236, 505),
            15,
            MUTED,
            808,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_selection_is_bounded_and_can_cancel() {
        let mut dialog = AlphabetDialog::new("Nintendo Switch");
        assert!(matches!(
            dialog.action(Action::Confirm),
            Some(Outcome::Select('N'))
        ));
        for _ in 0..50 {
            dialog.action(Action::Right);
        }
        assert!(matches!(
            dialog.action(Action::Confirm),
            Some(Outcome::Select('Z'))
        ));
        for _ in 0..50 {
            dialog.action(Action::Up);
        }
        for _ in 0..50 {
            dialog.action(Action::Left);
        }
        assert!(matches!(
            dialog.action(Action::Confirm),
            Some(Outcome::Select('0'))
        ));
        assert!(matches!(dialog.action(Action::Back), Some(Outcome::Cancel)));
    }

    #[test]
    fn keyboard_accepts_letters_reserved_by_global_shortcuts_and_ignores_repeats() {
        let mut dialog = AlphabetDialog::new("");
        for (key, letter) in [
            (Keycode::F, 'F'),
            (Keycode::V, 'V'),
            (Keycode::I, 'I'),
            (Keycode::L, 'L'),
            (Keycode::Num3, '3'),
        ] {
            let event = Event::KeyDown {
                timestamp: 0,
                window_id: 0,
                keycode: Some(key),
                scancode: None,
                keymod: sdl2::keyboard::Mod::NOMOD,
                repeat: false,
            };
            assert!(
                matches!(dialog.event(&event), Some(Outcome::Select(selected)) if selected == letter)
            );
        }
        let event = Event::KeyDown {
            timestamp: 0,
            window_id: 0,
            keycode: Some(Keycode::L),
            scancode: None,
            keymod: sdl2::keyboard::Mod::NOMOD,
            repeat: true,
        };
        assert!(dialog.event(&event).is_none());
    }
}
