//! Key bindings and `+`/`-` buttons.
//!
//! Keys use the original engine's names (`w`, `SPACE`, `MOUSE1`, ...), so the
//! player's own `config.cfg` applies unchanged. Pressing a key runs its bound
//! command; releasing a key bound to a `+command` runs the matching
//! `-command`. Buttons remember which keys hold them, like the original
//! input code, so two keys bound to `+forward` behave as expected.

use std::collections::{HashMap, HashSet};

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;

use super::Console;

/// Source key name and the physical key it maps to.
const KEYS: &[(&str, KeyCode)] = &[
    ("a", KeyCode::KeyA),
    ("b", KeyCode::KeyB),
    ("c", KeyCode::KeyC),
    ("d", KeyCode::KeyD),
    ("e", KeyCode::KeyE),
    ("f", KeyCode::KeyF),
    ("g", KeyCode::KeyG),
    ("h", KeyCode::KeyH),
    ("i", KeyCode::KeyI),
    ("j", KeyCode::KeyJ),
    ("k", KeyCode::KeyK),
    ("l", KeyCode::KeyL),
    ("m", KeyCode::KeyM),
    ("n", KeyCode::KeyN),
    ("o", KeyCode::KeyO),
    ("p", KeyCode::KeyP),
    ("q", KeyCode::KeyQ),
    ("r", KeyCode::KeyR),
    ("s", KeyCode::KeyS),
    ("t", KeyCode::KeyT),
    ("u", KeyCode::KeyU),
    ("v", KeyCode::KeyV),
    ("w", KeyCode::KeyW),
    ("x", KeyCode::KeyX),
    ("y", KeyCode::KeyY),
    ("z", KeyCode::KeyZ),
    ("0", KeyCode::Digit0),
    ("1", KeyCode::Digit1),
    ("2", KeyCode::Digit2),
    ("3", KeyCode::Digit3),
    ("4", KeyCode::Digit4),
    ("5", KeyCode::Digit5),
    ("6", KeyCode::Digit6),
    ("7", KeyCode::Digit7),
    ("8", KeyCode::Digit8),
    ("9", KeyCode::Digit9),
    ("kp_ins", KeyCode::Numpad0),
    ("kp_end", KeyCode::Numpad1),
    ("kp_downarrow", KeyCode::Numpad2),
    ("kp_pgdn", KeyCode::Numpad3),
    ("kp_leftarrow", KeyCode::Numpad4),
    ("kp_5", KeyCode::Numpad5),
    ("kp_rightarrow", KeyCode::Numpad6),
    ("kp_home", KeyCode::Numpad7),
    ("kp_uparrow", KeyCode::Numpad8),
    ("kp_pgup", KeyCode::Numpad9),
    ("kp_slash", KeyCode::NumpadDivide),
    ("kp_multiply", KeyCode::NumpadMultiply),
    ("kp_minus", KeyCode::NumpadSubtract),
    ("kp_plus", KeyCode::NumpadAdd),
    ("kp_enter", KeyCode::NumpadEnter),
    ("kp_del", KeyCode::NumpadDecimal),
    ("[", KeyCode::BracketLeft),
    ("]", KeyCode::BracketRight),
    ("semicolon", KeyCode::Semicolon),
    ("'", KeyCode::Quote),
    ("`", KeyCode::Backquote),
    (",", KeyCode::Comma),
    (".", KeyCode::Period),
    ("/", KeyCode::Slash),
    ("\\", KeyCode::Backslash),
    ("-", KeyCode::Minus),
    ("=", KeyCode::Equal),
    ("enter", KeyCode::Enter),
    ("space", KeyCode::Space),
    ("backspace", KeyCode::Backspace),
    ("tab", KeyCode::Tab),
    ("capslock", KeyCode::CapsLock),
    ("numlock", KeyCode::NumLock),
    ("escape", KeyCode::Escape),
    ("scrolllock", KeyCode::ScrollLock),
    ("ins", KeyCode::Insert),
    ("del", KeyCode::Delete),
    ("home", KeyCode::Home),
    ("end", KeyCode::End),
    ("pgup", KeyCode::PageUp),
    ("pgdn", KeyCode::PageDown),
    ("pause", KeyCode::Pause),
    ("shift", KeyCode::ShiftLeft),
    ("rshift", KeyCode::ShiftRight),
    ("alt", KeyCode::AltLeft),
    ("ralt", KeyCode::AltRight),
    ("ctrl", KeyCode::ControlLeft),
    ("rctrl", KeyCode::ControlRight),
    ("lwin", KeyCode::SuperLeft),
    ("rwin", KeyCode::SuperRight),
    ("app", KeyCode::ContextMenu),
    ("uparrow", KeyCode::ArrowUp),
    ("leftarrow", KeyCode::ArrowLeft),
    ("downarrow", KeyCode::ArrowDown),
    ("rightarrow", KeyCode::ArrowRight),
    ("f1", KeyCode::F1),
    ("f2", KeyCode::F2),
    ("f3", KeyCode::F3),
    ("f4", KeyCode::F4),
    ("f5", KeyCode::F5),
    ("f6", KeyCode::F6),
    ("f7", KeyCode::F7),
    ("f8", KeyCode::F8),
    ("f9", KeyCode::F9),
    ("f10", KeyCode::F10),
    ("f11", KeyCode::F11),
    ("f12", KeyCode::F12),
];

const MOUSE: &[(&str, MouseButton)] = &[
    ("mouse1", MouseButton::Left),
    ("mouse2", MouseButton::Right),
    ("mouse3", MouseButton::Middle),
    ("mouse4", MouseButton::Back),
    ("mouse5", MouseButton::Forward),
];

/// Controller button names; binds to them are kept (controllers are not
/// read yet).
const PAD: &[&str] = &[
    "a_button", "b_button", "x_button", "y_button", "l_shoulder", "r_shoulder", "back", "start", "stick1", "stick2",
    "up", "right", "down", "left", "l_trigger", "r_trigger", "s1_up", "s1_down", "s1_left", "s1_right", "s2_up",
    "s2_down", "s2_left", "s2_right",
];

/// Whether `name` is a key the engine knows (bind accepts only these).
pub fn is_key_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    KEYS.iter().any(|(k, _)| *k == n)
        || MOUSE.iter().any(|(k, _)| *k == n)
        || PAD.contains(&n.as_str())
        || n == "mwheelup"
        || n == "mwheeldown"
        || n.strip_prefix("joy").is_some_and(|d| d.parse::<u32>().is_ok())
}

/// State of the `+`/`-` buttons (`forward`, `jump`, `attack`, ...).
#[derive(Resource, Default)]
pub struct Buttons {
    /// Button name -> keys holding it ("" for the console).
    held: HashMap<String, HashSet<String>>,
    pressed: HashSet<String>,
    released: HashSet<String>,
}

impl Buttons {
    pub fn held(&self, name: &str) -> bool {
        self.held.get(name).is_some_and(|h| !h.is_empty())
    }

    /// Went down this frame.
    pub fn just_pressed(&self, name: &str) -> bool {
        self.pressed.contains(name)
    }

    pub fn just_released(&self, name: &str) -> bool {
        self.released.contains(name)
    }

    pub fn press(&mut self, name: &str, key: &str) {
        let holders = self.held.entry(name.to_owned()).or_default();
        if holders.is_empty() {
            self.pressed.insert(name.to_owned());
        }
        holders.insert(key.to_owned());
    }

    pub fn release(&mut self, name: &str, key: &str) {
        let Some(holders) = self.held.get_mut(name) else { return };
        if key.is_empty() {
            holders.clear();
        } else {
            holders.remove(key);
        }
        if holders.is_empty() {
            self.released.insert(name.to_owned());
        }
    }

    /// Lets go of everything (map changes, focus loss).
    pub fn clear(&mut self) {
        for (name, holders) in self.held.iter_mut() {
            if !holders.is_empty() {
                self.released.insert(name.clone());
            }
            holders.clear();
        }
    }

    fn begin_frame(&mut self) {
        self.pressed.clear();
        self.released.clear();
    }
}

/// Runs bound commands for this frame's key presses and releases.
pub fn process_binds(world: &mut World) {
    world.resource_mut::<Buttons>().begin_frame();
    let mut events: Vec<(&'static str, bool)> = Vec::new();
    {
        let keys = world.resource::<ButtonInput<KeyCode>>();
        for (name, code) in KEYS {
            if keys.just_pressed(*code) {
                events.push((name, true));
            }
            if keys.just_released(*code) {
                events.push((name, false));
            }
        }
        let mouse = world.resource::<ButtonInput<MouseButton>>();
        for (name, button) in MOUSE {
            if mouse.just_pressed(*button) {
                events.push((name, true));
            }
            if mouse.just_released(*button) {
                events.push((name, false));
            }
        }
        // Wheel steps press and release at once.
        let scroll = world.resource::<AccumulatedMouseScroll>().delta.y;
        if scroll != 0.0 {
            let name = if scroll > 0.0 { "mwheelup" } else { "mwheeldown" };
            events.push((name, true));
            events.push((name, false));
        }
    }
    for (key, down) in events {
        let Some(binding) = world.resource::<Console>().binds.get(key).cloned() else { continue };
        let binding = binding.trim().to_owned();
        if down {
            // `+commands` learn which key holds them.
            let text = if binding.starts_with('+') && !binding.contains(';') { format!("{binding} {key}") } else { binding };
            super::execute_now(world, &text);
        } else if let Some(rest) = binding.strip_prefix('+')
            && !binding.contains(';')
        {
            let name = rest.split_whitespace().next().unwrap_or(rest);
            super::execute_now(world, &format!("-{name} {key}"));
        }
    }
}
