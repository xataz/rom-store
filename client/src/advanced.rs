//! A controller-operated form, using the existing on-screen keyboard for values.
use crate::{
    input::Action,
    search::{SearchDialog, SearchOutcome},
    ui::{self, Painter, ACCENT, MUTED, TEXT},
};
use anyhow::Result;
use romstore_client::{api::System, i18n::Language};
use sdl2::{event::Event, rect::Rect, render::Canvas, video::Window};
use std::collections::BTreeMap;

pub enum Outcome {
    Submit,
    Cancel,
}

pub struct AdvancedDialog {
    pub scope: Option<String>,
    pub values: BTreeMap<String, String>,
    fields: Vec<String>,
    systems: Vec<(String, String)>,
    pub selected: usize,
    editing: Option<SearchDialog>,
    pub loading: bool,
    pub error: Option<String>,
}

impl AdvancedDialog {
    pub fn new(
        systems: &[System],
        scope: Option<String>,
        query: &str,
        filters: &BTreeMap<String, String>,
    ) -> Self {
        let mut values = filters.clone();
        if !query.is_empty() || !values.contains_key("name") {
            values.insert("name".into(), query.into());
        }
        let mut dialog = Self {
            scope,
            values,
            fields: Vec::new(),
            systems: systems
                .iter()
                .map(|s| (s.id.clone(), s.display_name().into()))
                .collect(),
            selected: 0,
            editing: None,
            loading: true,
            error: None,
        };
        dialog.set_fields(vec![
            "name".into(),
            "year".into(),
            "players".into(),
            "genre".into(),
            "developer".into(),
            "publisher".into(),
            "rating".into(),
        ]);
        dialog.loading = true;
        dialog
    }

    pub fn set_fields(&mut self, fields: Vec<String>) {
        let selected_field = self
            .selected
            .checked_sub(1)
            .and_then(|i| self.fields.get(i))
            .cloned();
        let selected_tail = self.selected.checked_sub(self.fields.len() + 1);
        let mut combined = vec![
            "name".to_owned(),
            "year".into(),
            "players".into(),
            "genre".into(),
            "developer".into(),
            "publisher".into(),
            "rating".into(),
        ];
        for field in fields.into_iter().chain(self.values.keys().cloned()) {
            if !combined.contains(&field) {
                combined.push(field);
            }
        }
        self.fields = combined;
        if let Some(field) = selected_field {
            self.selected = self
                .fields
                .iter()
                .position(|key| key == &field)
                .unwrap_or(0)
                + 1;
        } else if let Some(tail) = selected_tail {
            self.selected = self.fields.len() + 1 + tail.min(1);
        }
        self.loading = false;
    }

    pub fn text_input(&self) -> bool {
        self.editing.is_some()
    }

    fn cycle_platform(&mut self, forward: bool) {
        let current = self
            .scope
            .as_ref()
            .and_then(|id| self.systems.iter().position(|(key, _)| key == id))
            .map_or(0, |i| i + 1);
        let count = self.systems.len() + 1;
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        self.scope = next.checked_sub(1).map(|i| self.systems[i].0.clone());
    }

    fn edit_outcome(&mut self, outcome: Option<SearchOutcome>) {
        match outcome {
            Some(SearchOutcome::Submit) => {
                let edit = self.editing.take().unwrap();
                self.values.insert(
                    self.fields[self.selected - 1].clone(),
                    edit.query.trim().into(),
                );
            }
            Some(SearchOutcome::Cancel) => self.editing = None,
            None => (),
        }
    }

    pub fn event(&mut self, event: &Event) {
        if let Some(edit) = &mut self.editing {
            let outcome = edit.event(event);
            self.edit_outcome(outcome);
        }
    }

    pub fn action(&mut self, action: Action) -> Option<Outcome> {
        if let Some(edit) = &mut self.editing {
            let outcome = edit.action(action);
            self.edit_outcome(outcome);
            return None;
        }
        let count = self.fields.len() + 3;
        match action {
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Down => self.selected = (self.selected + 1).min(count - 1),
            Action::Left | Action::Right if self.selected == 0 => {
                self.cycle_platform(action == Action::Right)
            }
            Action::Back => return Some(Outcome::Cancel),
            Action::Confirm if self.selected == 0 => self.cycle_platform(true),
            Action::Confirm if self.selected <= self.fields.len() => {
                let field = &self.fields[self.selected - 1];
                self.editing = Some(SearchDialog::text_entry(
                    self.values.get(field).cloned().unwrap_or_default(),
                ));
            }
            Action::Confirm if self.selected == self.fields.len() + 1 => {
                self.values.clear();
                self.scope = None;
            }
            Action::Confirm => return Some(Outcome::Submit),
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
        ui::dim(canvas, 215)?;
        ui::panel(canvas, Rect::new(180, 90, 920, 540), true)?;
        painter.text(canvas, l.text("advanced_title"), (210, 110), 30, TEXT, 860)?;
        painter.text(
            canvas,
            self.error.as_deref().unwrap_or(l.text(if self.loading {
                "advanced_loading"
            } else {
                "advanced_help"
            })),
            (212, 155),
            14,
            MUTED,
            856,
        )?;
        let start = self.selected.saturating_sub(6);
        for index in (start..self.fields.len() + 3).take(7) {
            let y = 190 + (index - start) as i32 * 49;
            let selected = index == self.selected;
            ui::rounded(
                canvas,
                Rect::new(210, y, 860, 43),
                8,
                if selected { ui::BORDER } else { ui::SURFACE },
            )?;
            let (label, value) = if index == 0 {
                (
                    l.text("filter_platform"),
                    self.scope
                        .as_ref()
                        .and_then(|id| self.systems.iter().find(|(key, _)| key == id))
                        .map_or(l.text("all_platforms"), |(_, name)| name.as_str()),
                )
            } else if let Some(field) = self.fields.get(index - 1) {
                (
                    match field.as_str() {
                        "name" => l.text("filter_name"),
                        "year" => l.text("filter_year"),
                        "players" => l.text("players"),
                        "genre" => l.text("filter_genre"),
                        "developer" => l.text("developer"),
                        "publisher" => l.text("filter_publisher"),
                        "rating" => l.text("filter_rating"),
                        _ => field.as_str(),
                    },
                    self.values
                        .get(field)
                        .filter(|v| !v.is_empty())
                        .map_or(l.text("filter_any"), String::as_str),
                )
            } else {
                (
                    l.text(if index == self.fields.len() + 1 {
                        "advanced_clear"
                    } else {
                        "advanced_apply"
                    }),
                    "",
                )
            };
            painter.text(
                canvas,
                label,
                (226, y + 9),
                20,
                if selected { ACCENT } else { TEXT },
                330,
            )?;
            painter.text(canvas, value, (580, y + 11), 18, MUTED, 470)?;
        }
        painter.text(
            canvas,
            l.text("advanced_controls"),
            (212, 575),
            15,
            MUTED,
            856,
        )?;
        if let Some(edit) = &self.editing {
            edit.draw(canvas, painter, l, None)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_edits_custom_fields_with_the_controller_and_clears_all_criteria() {
        let systems = vec![System {
            id: "switch".into(),
            name: "switch".into(),
            game_count: 1,
            image: None,
        }];
        let mut form = AdvancedDialog::new(
            &systems,
            None,
            "",
            &BTreeMap::from([("custom".into(), "old".into())]),
        );
        form.action(Action::Right);
        assert_eq!(form.scope.as_deref(), Some("switch"));
        form.selected = form.fields.iter().position(|key| key == "custom").unwrap() + 1;
        form.action(Action::Confirm);
        assert!(form.text_input());
        form.set_fields(vec!["lang".into(), "custom".into()]);
        form.editing.as_mut().unwrap().query = "=demo".into();
        form.edit_outcome(Some(SearchOutcome::Submit));
        assert_eq!(form.values["custom"], "=demo");
        assert!(!form.text_input());
        form.selected = form.fields.len() + 1;
        form.action(Action::Confirm);
        assert!(form.values.is_empty());
        assert!(form.scope.is_none());
        form.action(Action::Down);
        assert!(matches!(
            form.action(Action::Confirm),
            Some(Outcome::Submit)
        ));
    }
}
