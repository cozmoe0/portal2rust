//! Portal 2's gameplay on the portal2rust engine: map entities and their
//! I/O ([`entities`]), the player ([`player`]), physics and carrying
//! ([`physics`]), portals and the portal gun ([`portals`]), test devices
//! such as lasers, bridges, funnels, faith plates and turrets
//! ([`devices`]), gels ([`paint`]), the VScript host for map scripts
//! ([`script`]), saved games ([`save`]), and the entities that drive
//! sounds, particle effects, visuals and screen messages.

pub mod commands;
pub mod devices;
pub mod effects;
pub mod entities;
pub mod paint;
pub mod physics;
pub mod player;
pub mod portals;
pub mod save;
pub mod screen;
pub mod script;
pub mod sound;
pub mod visuals;
