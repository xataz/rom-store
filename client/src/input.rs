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
    Search,
    Alphabet,
    Menu,
    Back,
    Quit,
}

fn button_action(button: Button) -> Option<Action> {
    match button {
        Button::A => Some(Action::Confirm),
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
}

impl Input {
    pub fn new(subsystem: GameControllerSubsystem) -> Self {
        let mut input = Self {
            subsystem,
            controllers: HashMap::new(),
            held: None,
            repeat_at: Instant::now(),
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
        match event {
            Event::Quit { .. } => Some(Action::Quit),
            Event::ControllerDeviceAdded { which, .. } => {
                self.open(which);
                None
            }
            Event::ControllerDeviceRemoved { which, .. } => {
                self.controllers.remove(&which);
                self.held = None;
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
    fn only_core_buttons_and_menu_have_direct_bindings() {
        assert_eq!(key_action(Keycode::Escape, false), Some(Action::Menu));
        assert_eq!(button_action(Button::Start), Some(Action::Menu));
        assert_eq!(key_action(Keycode::Backspace, false), Some(Action::Back));
        assert_eq!(button_action(Button::B), Some(Action::Back));
        for key in [
            Keycode::I,
            Keycode::V,
            Keycode::F5,
            Keycode::Tab,
            Keycode::Delete,
        ] {
            assert_eq!(key_action(key, false), None);
        }
        for button in [
            Button::X,
            Button::Y,
            Button::RightStick,
            Button::LeftShoulder,
        ] {
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
