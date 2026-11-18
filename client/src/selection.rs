//! Stable game identities survive infinite scroll and alphabet jumps.
use crate::Game;

#[derive(Default)]
pub struct Selection {
    pub enabled: bool,
    pub games: Vec<Game>,
}

impl Selection {
    pub fn contains(&self, id: &str) -> bool {
        self.games.iter().any(|game| game.id == id)
    }

    pub fn toggle(&mut self, game: Game) {
        self.enabled = true;
        if let Some(index) = self
            .games
            .iter()
            .position(|selected| selected.id == game.id)
        {
            self.games.remove(index);
        } else {
            self.games.push(game);
        }
    }

    pub fn clear(&mut self) {
        self.games.clear();
        self.enabled = false;
    }
}
