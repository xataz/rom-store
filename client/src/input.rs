use sdl2::{
    controller::{Axis, Button, GameController},
    event::Event,
    keyboard::Keycode,
    GameControllerSubsystem,
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Select,
    DownloadSelection,
    Search,
    Alphabet,
    Menu,
    Back,
    Quit,
}

fn button_action(button: Button) -> Option<Action> {
    match button {
        Button::A => Some(Action::Confirm),
        Button::X => Some(Action::DownloadSelection),
        Button::B => Some(Action::Back),
        Button::Start => Some(Action::Menu),
        Button::LeftStick => Some(Action::Search),
        Button::RightShoulder => Some(Action::Alphabet),
        _ => None,
    }
}

fn key_action(key: Keycode, repeat: bool) -> Option<Action> {
    match key {
        Keycode::Up => Some(Action::Up),
        Keycode::Down => Some(Action::Down),
        Keycode::Left => Some(Action::Left),
        Keycode::Right => Some(Action::Right),
        Keycode::Return if !repeat => Some(Action::Confirm),
        Keycode::Space if !repeat => Some(Action::DownloadSelection),
        Keycode::F if !repeat => Some(Action::Search),
        Keycode::L if !repeat => Some(Action::Alphabet),
        Keycode::Escape if !repeat => Some(Action::Menu),
        Keycode::Backspace if !repeat => Some(Action::Back),
        _ => None,
    }
}

pub struct Input {
    subsystem: GameControllerSubsystem,
    controllers: HashMap<u32, GameController>,
    held: Option<Action>,
    repeat_at: Instant,
    confirm: ConfirmHold,
}

const LONG_PRESS: Duration = Duration::from_millis(650);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmSource {
    Keyboard,
    Controller(u32),
}

#[derive(Default)]
struct ConfirmHold {
    held: Option<(ConfirmSource, Instant, bool)>,
}

impl ConfirmHold {
    fn press(&mut self, source: ConfirmSource, now: Instant) {
        if self.held.is_none() {
            self.held = Some((source, now, false));
        }
    }

    fn poll(&mut self, now: Instant) -> Option<Action> {
        let (_, start, fired) = self.held.as_mut()?;
        if !*fired && now.duration_since(*start) >= LONG_PRESS {
            *fired = true;
            Some(Action::Select)
        } else {
            None
        }
    }

    fn release(&mut self, source: ConfirmSource, now: Instant) -> Option<Action> {
        let (pressed, start, fired) = self.held?;
        if source != pressed {
            return None;
        }
        self.held = None;
        if fired {
            None
        } else if now.duration_since(start) >= LONG_PRESS {
            Some(Action::Select)
        } else {
            Some(Action::Confirm)
        }
    }
}

impl Input {
    pub fn new(subsystem: GameControllerSubsystem) -> Self {
        let mut input = Self {
            subsystem,
            controllers: HashMap::new(),
            held: None,
            repeat_at: Instant::now(),
            confirm: ConfirmHold::default(),
        };
        for index in 0..input.subsystem.num_joysticks().unwrap_or(0) {
            input.open(index);
        }
        input
    }

    fn open(&mut self, index: u32) {
        if self.subsystem.is_game_controller(index) {
            if let Ok(controller) = self.subsystem.open(index) {
                self.controllers
                    .insert(controller.instance_id(), controller);
            }
        }
    }

    pub fn event(&mut self, event: Event) -> Option<Action> {
        if matches!(
            &event,
            Event::Window {
                win_event: sdl2::event::WindowEvent::FocusLost,
                ..
            }
        ) {
            self.confirm.held = None;
        }
        match event {
            Event::Quit { .. } => Some(Action::Quit),
            Event::ControllerDeviceAdded { which, .. } => {
                self.open(which);
                None
            }
            Event::ControllerDeviceRemoved { which, .. } => {
                self.controllers.remove(&which);
                self.held = None;
                self.confirm.held = None;
                None
            }
            Event::ControllerButtonDown { button, .. } => button_action(button),
            Event::KeyDown {
                keycode: Some(key),
                repeat,
                ..
            } => key_action(key, repeat),
            _ => None,
        }
    }

    /// Only the catalogue delays confirmation. Dialogs keep immediate validation.
    /// Some(None) means the event belongs to the hold gesture and must be consumed.
    pub fn catalogue_event(&mut self, event: &Event, enabled: bool) -> Option<Option<Action>> {
        if !enabled {
            self.confirm.held = None;
            return None;
        }
        let now = Instant::now();
        match event {
            Event::KeyDown {
                keycode: Some(Keycode::Return),
                repeat,
                ..
            } => {
                if !repeat {
                    self.confirm.press(ConfirmSource::Keyboard, now);
                }
                Some(None)
            }
            Event::KeyUp {
                keycode: Some(Keycode::Return),
                ..
            } => Some(self.confirm.release(ConfirmSource::Keyboard, now)),
            Event::ControllerButtonDown {
                button: Button::A,
                which,
                ..
            } => {
                self.confirm.press(ConfirmSource::Controller(*which), now);
                Some(None)
            }
            Event::ControllerButtonUp {
                button: Button::A,
                which,
                ..
            } => Some(self.confirm.release(ConfirmSource::Controller(*which), now)),
            _ => {
                if matches!(
                    event,
                    Event::KeyDown { .. }
                        | Event::ControllerButtonDown { .. }
                        | Event::ControllerDeviceRemoved { .. }
                        | Event::Window {
                            win_event: sdl2::event::WindowEvent::FocusLost,
                            ..
                        }
                ) {
                    self.confirm.held = None;
                }
                None
            }
        }
    }

    pub fn poll_confirm(&mut self, enabled: bool) -> Option<Action> {
        if enabled {
            self.confirm.poll(Instant::now())
        } else {
            self.confirm.held = None;
            None
        }
    }

    pub fn poll_direction(&mut self) -> Option<Action> {
        let direction = self.controllers.values().find_map(|controller| {
            let x = controller.axis(Axis::LeftX);
            let y = controller.axis(Axis::LeftY);
            if controller.button(Button::DPadUp) || y < -16000 {
                Some(Action::Up)
            } else if controller.button(Button::DPadDown) || y > 16000 {
                Some(Action::Down)
            } else if controller.button(Button::DPadLeft) || x < -16000 {
                Some(Action::Left)
            } else if controller.button(Button::DPadRight) || x > 16000 {
                Some(Action::Right)
            } else {
                None
            }
        });
        let now = Instant::now();
        if direction.is_some() {
            self.confirm.held = None;
        }
        if direction != self.held {
            self.held = direction;
            self.repeat_at = now + Duration::from_millis(350);
            direction
        } else if direction.is_some() && now >= self.repeat_at {
            self.repeat_at = now + Duration::from_millis(110);
            direction
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_confirm_and_long_selection_are_mutually_exclusive() {
        let now = Instant::now();
        let mut hold = ConfirmHold::default();
        hold.press(ConfirmSource::Keyboard, now);
        assert_eq!(
            hold.release(ConfirmSource::Keyboard, now + Duration::from_millis(100)),
            Some(Action::Confirm)
        );
        hold.press(ConfirmSource::Controller(3), now);
        assert_eq!(hold.poll(now + LONG_PRESS - Duration::from_millis(1)), None);
        assert_eq!(hold.poll(now + LONG_PRESS), Some(Action::Select));
        assert_eq!(hold.poll(now + LONG_PRESS + LONG_PRESS), None);
        assert_eq!(
            hold.release(ConfirmSource::Controller(3), now + LONG_PRESS + LONG_PRESS),
            None
        );
        hold.press(ConfirmSource::Keyboard, now);
        assert_eq!(
            hold.release(ConfirmSource::Keyboard, now + LONG_PRESS),
            Some(Action::Select)
        );
    }

    #[test]
    fn releases_from_another_controller_do_not_validate_or_reset_the_hold() {
        let now = Instant::now();
        let mut hold = ConfirmHold::default();
        hold.press(ConfirmSource::Controller(1), now);
        assert_eq!(hold.release(ConfirmSource::Controller(2), now), None);
        hold.press(ConfirmSource::Controller(1), now + LONG_PRESS);
        assert_eq!(hold.poll(now + LONG_PRESS), Some(Action::Select));
        hold.held = None;
        assert_eq!(
            hold.release(ConfirmSource::Controller(1), now + LONG_PRESS),
            None
        );
    }

    #[test]
    fn only_core_buttons_menu_and_batch_have_direct_bindings() {
        assert_eq!(key_action(Keycode::Escape, false), Some(Action::Menu));
        assert_eq!(button_action(Button::Start), Some(Action::Menu));
        assert_eq!(key_action(Keycode::Backspace, false), Some(Action::Back));
        assert_eq!(button_action(Button::B), Some(Action::Back));
        assert_eq!(button_action(Button::X), Some(Action::DownloadSelection));
        assert_eq!(
            key_action(Keycode::Space, false),
            Some(Action::DownloadSelection)
        );
        for key in [
            Keycode::I,
            Keycode::V,
            Keycode::F5,
            Keycode::Tab,
            Keycode::Delete,
        ] {
            assert_eq!(key_action(key, false), None);
        }
        for button in [Button::Y, Button::RightStick, Button::LeftShoulder] {
            assert_eq!(button_action(button), None);
        }
        assert_eq!(key_action(Keycode::Escape, true), None);
    }
    #[test]
    fn unknown_sdl_events_do_not_panic() {
        // Events observed with Fedora's SDL runtime must be safely ignored,
        // not transmuted into invalid enum values (rust-sdl2 0.37).
        for event_type in [0x206, 0x207, 0x1234] {
            assert!(sdl2::event::EventType::try_from(event_type).is_err());
        }
    }
}
