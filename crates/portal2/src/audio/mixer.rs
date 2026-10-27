//! Mix groups, sound mixers, mix layers and layer triggers from
//! `scripts/soundmixers.txt` (behaviour notes in `docs/re/audio_runtime.md`).
//!
//! Every sound belongs to up to eight mix groups, chosen by rules on its wave
//! path, emitting class, channel and sound level. The active mixer gives each
//! group a volume, level and DSP factor plus solo and mute amounts. Mix
//! layers modify those values; a layer fades in while a sound of one of its
//! triggering groups plays (this is how dialogue ducks the rest of the mix).

use std::collections::HashMap;

use source_formats::fs::GameFileSystem;

const MAX_GROUPS_PER_SOUND: usize = 8;
/// Host seconds between layer trigger updates.
const TRIGGER_PERIOD: f64 = 0.1;

#[derive(Clone, Copy, Debug)]
pub struct GroupValues {
    /// Negative: the group is not listed (the mixer leaves it alone).
    pub vol: f32,
    pub level: f32,
    pub dsp: f32,
    pub solo: f32,
    pub mute: f32,
}

impl Default for GroupValues {
    fn default() -> Self {
        Self { vol: -1.0, level: 1.0, dsp: 1.0, solo: 0.0, mute: 0.0 }
    }
}

struct Rule {
    group: usize,
    dir: String,
    class: String,
    channel: i32,
    level_min: i32,
    level_max: i32,
}

struct Mix {
    name: String,
    values: Vec<GroupValues>,
}

struct Layer {
    name: String,
    amount: f32,
    values: Vec<GroupValues>,
}

struct Trigger {
    layer: usize,
    group: usize,
    threshold: f32,
    factor: f32,
    attack: f32,
    release: f32,
}

#[derive(Default)]
pub struct MixData {
    groups: Vec<String>,
    rules: Vec<Rule>,
    mixes: Vec<Mix>,
    layers: Vec<Layer>,
    triggers: Vec<Trigger>,
    current: Option<usize>,
    // Combined layer values per group, rebuilt by `update_layers`.
    layer_vol: Vec<f32>,
    layer_level: Vec<f32>,
    layer_dsp: Vec<f32>,
    layer_solo: Vec<f32>,
    layer_mute: Vec<f32>,
    total_solo: f32,
    /// Summed `mixlayer_trigger` outputs of the sounds in each group.
    trigger_total: Vec<f32>,
    last_trigger_update: f64,
}

fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '/' {
            chars.next();
            if chars.peek() == Some(&'/') {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
        } else if c == '"' {
            chars.next();
            let mut s = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                s.push(c);
            }
            out.push(s);
        } else if c == '{' || c == '}' {
            chars.next();
            out.push(c.to_string());
        } else {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '"' || c == '{' || c == '}' {
                    break;
                }
                s.push(c);
                chars.next();
            }
            out.push(s);
        }
    }
    out
}

fn num(s: &str) -> f32 {
    source_formats::kv::parse_f32_lenient(s)
}

/// `atoi`, or `missing` for an empty field.
fn int_or(s: &str, missing: i32) -> i32 {
    if s.is_empty() { missing } else { source_formats::kv::parse_i32_lenient(s) }
}

impl MixData {
    pub fn load(fs: &GameFileSystem) -> Self {
        let mut data = Self::default();
        if let Some(text) = fs.read_string("scripts/soundmixers.txt") {
            data.parse(&text);
        } else {
            bevy::log::warn!("audio: scripts/soundmixers.txt missing");
        }
        data.select("Default_Mix");
        data
    }

    fn group_index(&mut self, name: &str) -> usize {
        if let Some(i) = self.groups.iter().position(|g| g.eq_ignore_ascii_case(name)) {
            return i;
        }
        self.groups.push(name.to_owned());
        for list in self.mixes.iter_mut().map(|m| &mut m.values).chain(self.layers.iter_mut().map(|l| &mut l.values)) {
            list.push(GroupValues::default());
        }
        self.groups.len() - 1
    }

    pub fn group(&self, name: &str) -> Option<usize> {
        self.groups.iter().position(|g| g.eq_ignore_ascii_case(name))
    }

    pub fn layer(&self, name: &str) -> Option<usize> {
        self.layers.iter().position(|l| l.name.eq_ignore_ascii_case(name))
    }

    fn parse(&mut self, text: &str) {
        let t = tokens(text);
        let mut i = 0;
        let section_end = |t: &[String], mut i: usize| {
            // `i` points after "{"; returns the index after the matching "}".
            let mut depth = 1;
            while i < t.len() && depth > 0 {
                if t[i] == "{" {
                    depth += 1;
                } else if t[i] == "}" {
                    depth -= 1;
                }
                i += 1;
            }
            i
        };
        while i < t.len() {
            let name = t[i].clone();
            if t.get(i + 1).map(String::as_str) != Some("{") {
                i += 1;
                continue;
            }
            let body = i + 2;
            let end = section_end(&t, body);
            let inner = &t[body..end.saturating_sub(1)];
            match name.to_ascii_lowercase().as_str() {
                "mixgroups" => {
                    for row in inner.chunks(11) {
                        if row.len() < 11 {
                            break;
                        }
                        let group = self.group_index(&row[0]);
                        let channel = match row[3].to_ascii_uppercase().as_str() {
                            "" => -1,
                            "CHAN_STATIC" => 6,
                            "CHAN_WEAPON" => 1,
                            "CHAN_VOICE" => 2,
                            "CHAN_BODY" => 4,
                            "CHAN_ITEM" => 3,
                            _ => 0,
                        };
                        self.rules.push(Rule {
                            group,
                            dir: row[1].to_ascii_lowercase(),
                            class: row[2].to_ascii_lowercase(),
                            channel,
                            level_min: int_or(&row[4], -1),
                            level_max: int_or(&row[5], -1),
                        });
                    }
                }
                "soundmixers" | "mixlayers" => {
                    let layers = name.eq_ignore_ascii_case("mixlayers");
                    let mut j = 0;
                    while j + 1 < inner.len() {
                        let block = inner[j].clone();
                        if inner[j + 1] != "{" {
                            j += 1;
                            continue;
                        }
                        let close = section_end(inner, j + 2);
                        let rows: Vec<String> = inner[j + 2..close - 1].to_vec();
                        let mut values = vec![GroupValues::default(); self.groups.len()];
                        for row in rows.chunks(6) {
                            if row.len() < 6 {
                                break;
                            }
                            let g = self.group_index(&row[0]);
                            values.resize(self.groups.len(), GroupValues::default());
                            values[g] = GroupValues {
                                vol: num(&row[1]),
                                level: num(&row[2]),
                                dsp: num(&row[3]),
                                solo: num(&row[4]),
                                mute: num(&row[5]),
                            };
                        }
                        values.resize(self.groups.len(), GroupValues::default());
                        if layers {
                            self.layers.push(Layer { name: block, amount: 0.0, values });
                        } else {
                            self.mixes.push(Mix { name: block, values });
                        }
                        j = close;
                    }
                }
                "layertriggers" => {
                    for row in inner.chunks(6) {
                        if row.len() < 6 {
                            break;
                        }
                        let (Some(layer), Some(group)) = (self.layer(&row[0]), self.group(&row[1])) else { continue };
                        self.triggers.push(Trigger {
                            layer,
                            group,
                            threshold: num(&row[2]),
                            factor: num(&row[3]),
                            attack: num(&row[4]),
                            release: num(&row[5]),
                        });
                    }
                }
                _ => {}
            }
            i = end;
        }
        let n = self.groups.len();
        for v in [&mut self.layer_vol, &mut self.layer_level, &mut self.layer_dsp] {
            *v = vec![1.0; n];
        }
        for v in [&mut self.layer_solo, &mut self.layer_mute, &mut self.trigger_total] {
            *v = vec![0.0; n];
        }
        bevy::log::debug!(
            "audio: {} mix groups, {} rules, {} mixers, {} layers, {} triggers",
            n,
            self.rules.len(),
            self.mixes.len(),
            self.layers.len(),
            self.triggers.len()
        );
    }

    /// Makes the named mixer current (`snd_soundmixer`, soundscapes).
    pub fn select(&mut self, name: &str) {
        if let Some(i) = self.mixes.iter().position(|m| m.name.eq_ignore_ascii_case(name)) {
            self.current = Some(i);
        }
    }

    /// The groups a new sound joins, in rule order.
    pub fn groups_for(&self, wave: &str, class: Option<&str>, local_player: bool, channel: i32, level: f32) -> Vec<usize> {
        let wave = wave.to_ascii_lowercase().replace('\\', "/");
        let class = class.map(str::to_ascii_lowercase);
        let mut out = Vec::new();
        for r in &self.rules {
            if !r.dir.is_empty() && !wave.contains(&r.dir) {
                continue;
            }
            if !r.class.is_empty() {
                let by_class = class.as_deref().is_some_and(|c| c.contains(&r.class));
                if !by_class && !(local_player && r.class == "localplayer") {
                    continue;
                }
            }
            if r.channel >= 0 && r.channel != channel {
                continue;
            }
            if r.level_min >= 0 && (r.level_min as f32) > level {
                continue;
            }
            if r.level_max >= 0 && level > r.level_max as f32 {
                continue;
            }
            out.push(r.group);
            if out.len() >= MAX_GROUPS_PER_SOUND {
                break;
            }
        }
        out
    }

    /// (volume, level, dsp) of a group under the current mixer and layers.
    pub fn values(&self, g: usize, master_level: f32, master_dsp: f32) -> (f32, f32, f32) {
        let Some(m) = self.current.map(|c| &self.mixes[c]) else { return (1.0, 1.0, 1.0) };
        let Some(v) = m.values.get(g) else { return (1.0, 1.0, 1.0) };
        if v.vol < 0.0 {
            return (1.0, 1.0, 1.0);
        }
        let level = v.level * self.layer_level[g] * master_level;
        let dsp = v.dsp * self.layer_dsp[g] * master_dsp;
        let mut vol = self.layer_vol[g] * v.vol;
        vol *= 1.0 - v.mute.max(self.layer_mute[g]);
        if self.total_solo > 0.0 {
            let solo = v.solo.max(self.layer_solo[g]);
            vol *= 1.0 - (1.0 - solo / self.total_solo) * self.total_solo;
        }
        (vol, level, dsp)
    }

    /// `sys_mixlayer`: overrides one field of a group inside a layer.
    pub fn set_layer_field(&mut self, layer: usize, group: usize, field: &str, value: f32) {
        let Some(v) = self.layers.get_mut(layer).and_then(|l| l.values.get_mut(group)) else { return };
        match field.to_ascii_lowercase().as_str() {
            "volume" => v.vol = value,
            "level" => v.level = value,
            "dsp" => v.dsp = value,
            "solo" => v.solo = value,
            "mute" => v.mute = value,
            _ => {}
        }
    }

    /// Combines the active layers into per-group factors.
    pub fn update_layers(&mut self, solo_disabled: bool) {
        let n = self.groups.len();
        let mut weight_sum = vec![0.0f32; n];
        for l in &self.layers {
            for (g, v) in l.values.iter().enumerate() {
                if v.vol >= 0.0 {
                    weight_sum[g] += l.amount;
                }
            }
        }
        self.layer_vol.iter_mut().for_each(|x| *x = 1.0);
        self.layer_level.iter_mut().for_each(|x| *x = 1.0);
        self.layer_dsp.iter_mut().for_each(|x| *x = 1.0);
        self.layer_solo.iter_mut().for_each(|x| *x = 0.0);
        self.layer_mute.iter_mut().for_each(|x| *x = 0.0);
        for l in &self.layers {
            for (g, v) in l.values.iter().enumerate() {
                if weight_sum[g] <= 0.0 || v.vol < 0.0 {
                    continue;
                }
                let w = l.amount / weight_sum[g] * l.amount;
                self.layer_vol[g] = (self.layer_vol[g] + (v.vol - 1.0) * w).clamp(0.0, 1.0);
                self.layer_level[g] = (self.layer_level[g] + (v.level - 1.0) * w).clamp(0.0, 1.0);
                self.layer_dsp[g] = (self.layer_dsp[g] + (v.dsp - 1.0) * w).clamp(0.0, 1.0);
                self.layer_solo[g] = (self.layer_solo[g] + v.solo * w).clamp(0.0, 1.0);
                self.layer_mute[g] = (self.layer_mute[g] + v.mute * w).clamp(0.0, 1.0);
            }
        }
        self.total_solo = 0.0;
        if !solo_disabled
            && let Some(m) = self.current.map(|c| &self.mixes[c])
        {
            for g in 0..n {
                let s = m.values.get(g).map_or(0.0, |v| v.solo);
                self.total_solo = self.total_solo.max(s).max(self.layer_solo[g]);
            }
        }
    }

    /// Ramps triggered layers towards their targets. `sounds` lists each
    /// playing sound's groups and `mixlayer_trigger` value.
    pub fn update_triggers<'a>(&mut self, now: f64, sounds: impl Iterator<Item = (&'a [usize], f32)>) {
        if (now - self.last_trigger_update).abs() < TRIGGER_PERIOD {
            return;
        }
        self.last_trigger_update = now;
        self.trigger_total.iter_mut().for_each(|t| *t = 0.0);
        for (groups, value) in sounds {
            if value > 0.0 {
                for &g in groups {
                    self.trigger_total[g] += value;
                }
            }
        }
        let mut per_layer: HashMap<usize, (f32, f32, f32, f32)> = HashMap::new();
        for t in &self.triggers {
            let e = per_layer.entry(t.layer).or_insert((0.0, 0.0, 0.0, 0.0));
            e.0 = e.0.max(t.factor);
            if self.trigger_total[t.group] > t.threshold {
                e.1 = e.1.max(t.factor);
            }
            e.2 = e.2.max(t.attack);
            e.3 = e.3.max(t.release);
        }
        for (layer, (max_factor, target, attack, release)) in per_layer {
            let l = &mut self.layers[layer];
            let current = l.amount;
            if target == current {
                continue;
            }
            let time = if target > current { attack } else { release };
            l.amount = if time > 0.0 {
                let step = (TRIGGER_PERIOD as f32 / time * max_factor).copysign(target - current);
                let next = current + step;
                if (step < 0.0 && next < target) || (step > 0.0 && next > target) { target } else { next }
            } else {
                target
            };
        }
    }
}
