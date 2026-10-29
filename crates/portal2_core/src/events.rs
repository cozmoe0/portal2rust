//! Messages between the engine crates.

use bevy::prelude::*;

/// A value carried by a game event.
#[derive(Clone, Debug, PartialEq)]
pub enum EventValue {
    Int(i32),
    Float(f32),
    Str(String),
    /// Players (`userid`) and entities (entity indices) alike.
    Entity(Option<Entity>),
}

/// A named game event with its fields, as the server and client code fire
/// them; the game instructor listens.
#[derive(Message, Clone, Debug)]
pub struct GameEvent {
    pub name: String,
    pub fields: Vec<(String, EventValue)>,
}

impl GameEvent {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_owned(), fields: Vec::new() }
    }

    pub fn with(mut self, key: &str, value: EventValue) -> Self {
        self.fields.push((key.to_owned(), value));
        self
    }

    pub fn get(&self, key: &str) -> Option<&EventValue> {
        self.fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
    }
}

/// Shows the close caption of a sound entry: sent when a sound with a
/// caption starts (`time` is the game clock then, `duration` the sound's
/// length in seconds).
#[derive(Message, Clone, Debug)]
pub struct CaptionRequest {
    pub token: String,
    pub duration: f32,
    pub from_player: bool,
    pub time: f64,
}
