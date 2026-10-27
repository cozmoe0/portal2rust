//! Particle system definitions (`.pcf`): binary DMX files whose root lists
//! `DmeParticleSystemDefinition` elements. A definition has base properties,
//! six ordered stacks of `DmeParticleOperator`s (each naming an engine
//! operator through `functionName`) and `DmeParticleChild` links to other
//! definitions.
//!
//! Newer files store only attributes that differ from the defaults, so the
//! typed fields fall back to the game's own defaults (see `docs/re/pcf.md`).

use std::collections::HashMap;

use glam::Vec3;

use crate::dmx::{Attribute, Dmx, Element, Value};
use crate::text::normalize_path;
use crate::{Error, Result, invalid};

/// Global manifest naming the `.pcf` files the game loads.
pub const MANIFEST_PATH: &str = "particles/particles_manifest.txt";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Normalised path, e.g. `particles/fire_01.pcf`.
    pub path: String,
    /// Listed with a `!` prefix (precached when the manifest is read).
    pub precache: bool,
}

/// The `file` entries of a particle manifest, in order.
pub fn parse_manifest(text: &str) -> Vec<ManifestEntry> {
    let Some(root) = crate::kv::parse(text) else { return Vec::new() };
    root.get_all("file")
        .filter_map(|kv| kv.as_str())
        .map(|file| {
            let (precache, path) = match file.strip_prefix('!') {
                Some(path) => (true, path),
                None => (false, file),
            };
            ManifestEntry { path: normalize_path(path), precache }
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct Pcf {
    pub format_version: u32,
    /// Definitions in the order the root lists them, followed by any that are
    /// only reachable as children.
    pub systems: Vec<ParticleSystemDefinition>,
}

/// The six operator stacks of a definition, in the order files store them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OperatorKind {
    Renderer,
    Operator,
    Initializer,
    Emitter,
    Force,
    Constraint,
}

impl OperatorKind {
    pub const ALL: [OperatorKind; 6] =
        [Self::Renderer, Self::Operator, Self::Initializer, Self::Emitter, Self::Force, Self::Constraint];

    /// Element-array attribute of the definition holding this stack.
    pub fn attribute(self) -> &'static str {
        match self {
            Self::Renderer => "renderers",
            Self::Operator => "operators",
            Self::Initializer => "initializers",
            Self::Emitter => "emitters",
            Self::Force => "forces",
            Self::Constraint => "constraints",
        }
    }
}

/// Attributes of an element as stored, in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attributes(pub Vec<Attribute>);

impl Attributes {
    /// Value by exact attribute name.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.iter().find(|a| a.name == name).map(|a| &a.value)
    }

    pub fn get_i32(&self, name: &str) -> Option<i32> {
        self.get(name)?.as_i32()
    }

    pub fn get_f32(&self, name: &str) -> Option<f32> {
        self.get(name)?.as_f32()
    }

    pub fn get_bool(&self, name: &str) -> Option<bool> {
        self.get(name)?.as_bool()
    }

    pub fn get_str(&self, name: &str) -> Option<&str> {
        self.get(name)?.as_str()
    }

    pub fn get_vec3(&self, name: &str) -> Option<Vec3> {
        self.get(name)?.as_vec3()
    }

    pub fn get_color(&self, name: &str) -> Option<[u8; 4]> {
        self.get(name)?.as_color()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Attribute> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParticleOperator {
    /// Engine operator this entry instantiates, e.g. `Movement Basic`. The
    /// game matches it ignoring case.
    pub function_name: String,
    /// Label given in the editor; nearly always equal to `function_name`.
    pub name: String,
    /// Every attribute except `functionName`. Absent ones take the
    /// operator's own defaults.
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParticleChild {
    /// Name of the child definition.
    pub name: String,
    /// Index of the child definition in [`Pcf::systems`].
    pub index: usize,
    /// Seconds after the parent starts.
    pub delay: f32,
    /// `end cap effect`: started when the parent ends rather than with it.
    pub end_cap_effect: bool,
}

/// One particle system. Field comments give the DMX attribute name where it
/// differs from the field name.
#[derive(Clone, Debug, PartialEq)]
pub struct ParticleSystemDefinition {
    pub name: String,
    pub max_particles: i32,
    pub initial_particles: i32,
    /// Path relative to `materials/`, as stored (usually with backslashes
    /// and `.vmt`).
    pub material: String,
    pub bounding_box_min: Vec3,
    pub bounding_box_max: Vec3,
    pub cull_radius: f32,
    pub cull_cost: f32,
    pub cull_control_point: i32,
    /// `cull_replacement_definition`.
    pub cull_replacement: String,
    /// `fallback replacement definition`.
    pub fallback_replacement: String,
    /// `fallback max count`.
    pub fallback_max_count: i32,
    pub radius: f32,
    /// RGBA.
    pub color: [u8; 4],
    pub rotation: f32,
    pub rotation_speed: f32,
    pub normal: Vec3,
    pub sequence_number: i32,
    /// `sequence_number 1`.
    pub sequence_number_1: i32,
    /// `group id`.
    pub group_id: i32,
    /// `maximum time step`.
    pub max_time_step: f32,
    /// `maximum sim tick rate`.
    pub max_sim_tick_rate: f32,
    /// `minimum sim tick rate`.
    pub min_sim_tick_rate: f32,
    /// `minimum rendered frames`.
    pub min_rendered_frames: i32,
    /// `control point to disable rendering if it is the camera`; -1 for none.
    pub camera_disable_control_point: i32,
    /// `control point to only enable rendering if it is the camera`; -1 for none.
    pub camera_only_control_point: i32,
    /// `maximum draw distance`.
    pub max_draw_distance: f32,
    /// `time to sleep when not drawn`.
    pub sleep_when_not_drawn: f32,
    /// `Sort particles`.
    pub sort_particles: bool,
    /// `batch particle systems`.
    pub batch_particle_systems: bool,
    /// `view model effect`.
    pub view_model_effect: bool,
    /// `screen space effect`.
    pub screen_space_effect: bool,
    /// `draw through leafsystem`.
    pub draw_through_leafsystem: bool,
    /// `maximum portal recursion depth`.
    pub max_portal_recursion_depth: i32,
    /// `aggregation radius`.
    pub aggregation_radius: f32,
    /// `minimum free particles to aggregate`.
    pub min_free_particles_to_aggregate: i32,
    /// `minimum simulation time step`.
    pub min_simulation_time_step: f32,
    /// `minimum CPU level`.
    pub min_cpu_level: i32,
    /// `minimum GPU level`.
    pub min_gpu_level: i32,
    /// `freeze simulation after time`.
    pub freeze_simulation_after_time: f32,
    /// `preventNameBasedLookup`.
    pub prevent_name_based_lookup: bool,
    pub renderers: Vec<ParticleOperator>,
    pub operators: Vec<ParticleOperator>,
    pub initializers: Vec<ParticleOperator>,
    pub emitters: Vec<ParticleOperator>,
    pub forces: Vec<ParticleOperator>,
    pub constraints: Vec<ParticleOperator>,
    pub children: Vec<ParticleChild>,
    /// Every attribute except the operator stacks and `children`, as stored:
    /// both the ones the typed fields mirror and ones the game no longer
    /// reads (`bounding_box_control_point`, `fallback_dx80`).
    pub attributes: Attributes,
}

impl Default for ParticleSystemDefinition {
    /// The game's values for attributes a file leaves out.
    fn default() -> Self {
        Self {
            name: String::new(),
            max_particles: 1000,
            initial_particles: 0,
            material: "vgui/white".to_owned(),
            bounding_box_min: Vec3::splat(-10.0),
            bounding_box_max: Vec3::splat(10.0),
            cull_radius: 0.0,
            cull_cost: 1.0,
            cull_control_point: 0,
            cull_replacement: String::new(),
            fallback_replacement: String::new(),
            fallback_max_count: -1,
            radius: 5.0,
            color: [255; 4],
            rotation: 0.0,
            rotation_speed: 0.0,
            normal: Vec3::Z,
            sequence_number: 0,
            sequence_number_1: 0,
            group_id: 0,
            max_time_step: 0.1,
            max_sim_tick_rate: 0.0,
            min_sim_tick_rate: 0.0,
            min_rendered_frames: 0,
            camera_disable_control_point: -1,
            camera_only_control_point: -1,
            max_draw_distance: 100_000.0,
            sleep_when_not_drawn: 8.0,
            sort_particles: true,
            batch_particle_systems: false,
            view_model_effect: false,
            screen_space_effect: false,
            draw_through_leafsystem: true,
            max_portal_recursion_depth: 8,
            aggregation_radius: 0.0,
            min_free_particles_to_aggregate: 0,
            min_simulation_time_step: 0.0,
            min_cpu_level: 0,
            min_gpu_level: 0,
            freeze_simulation_after_time: 1.0e9,
            prevent_name_based_lookup: false,
            renderers: Vec::new(),
            operators: Vec::new(),
            initializers: Vec::new(),
            emitters: Vec::new(),
            forces: Vec::new(),
            constraints: Vec::new(),
            children: Vec::new(),
            attributes: Attributes::default(),
        }
    }
}

impl Pcf {
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::from_dmx(&Dmx::parse(data)?)
    }

    pub fn from_dmx(dmx: &Dmx) -> Result<Self> {
        let root = dmx.root().ok_or_else(|| Error::Invalid("empty pcf".into()))?;
        let Some(listed) = root.get("particleSystemDefinitions").and_then(Value::as_elements) else {
            return invalid("pcf root has no particleSystemDefinitions");
        };
        // Element index of each system, listed ones first, then the closure
        // over child links.
        let mut order: Vec<usize> = Vec::new();
        let mut index_of: HashMap<usize, usize> = HashMap::new();
        let mut add = |e: usize, order: &mut Vec<usize>| {
            index_of.entry(e).or_insert_with(|| {
                order.push(e);
                order.len() - 1
            });
        };
        for &e in listed.iter().flatten() {
            add(e, &mut order);
        }
        let mut next = 0;
        while next < order.len() {
            for child in linked(dmx, element_at(dmx, order[next])?, "children") {
                if let Some(e) = child.get("child").and_then(Value::as_element) {
                    add(e, &mut order);
                }
            }
            next += 1;
        }

        let mut systems = Vec::with_capacity(order.len());
        for &e in &order {
            let element = element_at(dmx, e)?;
            if element.type_name != "DmeParticleSystemDefinition" {
                return invalid(format!("{} {:?} used as a particle system", element.type_name, element.name));
            }
            systems.push(ParticleSystemDefinition::from_element(dmx, element, &index_of)?);
        }
        Ok(Self { format_version: dmx.header.format_version, systems })
    }

    /// Definition by name, ignoring case.
    pub fn find(&self, name: &str) -> Option<&ParticleSystemDefinition> {
        self.systems.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }
}

fn element_at(dmx: &Dmx, index: usize) -> Result<&Element> {
    dmx.elements.get(index).ok_or_else(|| Error::Invalid(format!("pcf element {index} out of range")))
}

/// Non-null elements referenced by an element-array attribute.
fn linked<'a>(dmx: &'a Dmx, element: &'a Element, attribute: &'static str) -> impl Iterator<Item = &'a Element> {
    let refs = element.get(attribute).and_then(Value::as_elements).unwrap_or_default();
    refs.iter().flatten().filter_map(|&i| dmx.elements.get(i))
}

impl ParticleSystemDefinition {
    fn from_element(dmx: &Dmx, element: &Element, index_of: &HashMap<usize, usize>) -> Result<Self> {
        let stack = |kind: OperatorKind| -> Vec<ParticleOperator> {
            linked(dmx, element, kind.attribute()).map(ParticleOperator::from_element).collect()
        };
        let mut children = Vec::new();
        for child in linked(dmx, element, "children") {
            let Some(target) = child.get("child").and_then(Value::as_element) else { continue };
            let index = *index_of.get(&target).ok_or_else(|| Error::Invalid("unresolved pcf child".into()))?;
            children.push(ParticleChild {
                name: element_at(dmx, target)?.name.clone(),
                index,
                delay: child.get("delay").and_then(Value::as_f32).unwrap_or(0.0),
                end_cap_effect: child.get("end cap effect").and_then(Value::as_bool).unwrap_or(false),
            });
        }
        let structural = |name: &str| name == "children" || OperatorKind::ALL.iter().any(|k| k.attribute() == name);
        let a = Attributes(element.attributes.iter().filter(|a| !structural(&a.name)).cloned().collect());
        let d = Self::default();
        let string = |name: &str, default: String| a.get_str(name).map_or(default, str::to_owned);
        Ok(Self {
            name: element.name.clone(),
            max_particles: a.get_i32("max_particles").unwrap_or(d.max_particles),
            initial_particles: a.get_i32("initial_particles").unwrap_or(d.initial_particles),
            material: string("material", d.material),
            bounding_box_min: a.get_vec3("bounding_box_min").unwrap_or(d.bounding_box_min),
            bounding_box_max: a.get_vec3("bounding_box_max").unwrap_or(d.bounding_box_max),
            cull_radius: a.get_f32("cull_radius").unwrap_or(d.cull_radius),
            cull_cost: a.get_f32("cull_cost").unwrap_or(d.cull_cost),
            cull_control_point: a.get_i32("cull_control_point").unwrap_or(d.cull_control_point),
            cull_replacement: string("cull_replacement_definition", d.cull_replacement),
            fallback_replacement: string("fallback replacement definition", d.fallback_replacement),
            fallback_max_count: a.get_i32("fallback max count").unwrap_or(d.fallback_max_count),
            radius: a.get_f32("radius").unwrap_or(d.radius),
            color: a.get_color("color").unwrap_or(d.color),
            rotation: a.get_f32("rotation").unwrap_or(d.rotation),
            rotation_speed: a.get_f32("rotation_speed").unwrap_or(d.rotation_speed),
            normal: a.get_vec3("normal").unwrap_or(d.normal),
            sequence_number: a.get_i32("sequence_number").unwrap_or(d.sequence_number),
            sequence_number_1: a.get_i32("sequence_number 1").unwrap_or(d.sequence_number_1),
            group_id: a.get_i32("group id").unwrap_or(d.group_id),
            max_time_step: a.get_f32("maximum time step").unwrap_or(d.max_time_step),
            max_sim_tick_rate: a.get_f32("maximum sim tick rate").unwrap_or(d.max_sim_tick_rate),
            min_sim_tick_rate: a.get_f32("minimum sim tick rate").unwrap_or(d.min_sim_tick_rate),
            min_rendered_frames: a.get_i32("minimum rendered frames").unwrap_or(d.min_rendered_frames),
            camera_disable_control_point: a
                .get_i32("control point to disable rendering if it is the camera")
                .unwrap_or(d.camera_disable_control_point),
            camera_only_control_point: a
                .get_i32("control point to only enable rendering if it is the camera")
                .unwrap_or(d.camera_only_control_point),
            max_draw_distance: a.get_f32("maximum draw distance").unwrap_or(d.max_draw_distance),
            sleep_when_not_drawn: a.get_f32("time to sleep when not drawn").unwrap_or(d.sleep_when_not_drawn),
            sort_particles: a.get_bool("Sort particles").unwrap_or(d.sort_particles),
            batch_particle_systems: a.get_bool("batch particle systems").unwrap_or(d.batch_particle_systems),
            view_model_effect: a.get_bool("view model effect").unwrap_or(d.view_model_effect),
            screen_space_effect: a.get_bool("screen space effect").unwrap_or(d.screen_space_effect),
            draw_through_leafsystem: a.get_bool("draw through leafsystem").unwrap_or(d.draw_through_leafsystem),
            max_portal_recursion_depth: a
                .get_i32("maximum portal recursion depth")
                .unwrap_or(d.max_portal_recursion_depth),
            aggregation_radius: a.get_f32("aggregation radius").unwrap_or(d.aggregation_radius),
            min_free_particles_to_aggregate: a
                .get_i32("minimum free particles to aggregate")
                .unwrap_or(d.min_free_particles_to_aggregate),
            min_simulation_time_step: a.get_f32("minimum simulation time step").unwrap_or(d.min_simulation_time_step),
            min_cpu_level: a.get_i32("minimum CPU level").unwrap_or(d.min_cpu_level),
            min_gpu_level: a.get_i32("minimum GPU level").unwrap_or(d.min_gpu_level),
            freeze_simulation_after_time: a
                .get_f32("freeze simulation after time")
                .unwrap_or(d.freeze_simulation_after_time),
            prevent_name_based_lookup: a.get_bool("preventNameBasedLookup").unwrap_or(d.prevent_name_based_lookup),
            renderers: stack(OperatorKind::Renderer),
            operators: stack(OperatorKind::Operator),
            initializers: stack(OperatorKind::Initializer),
            emitters: stack(OperatorKind::Emitter),
            forces: stack(OperatorKind::Force),
            constraints: stack(OperatorKind::Constraint),
            children,
            attributes: a,
        })
    }

    pub fn stack(&self, kind: OperatorKind) -> &[ParticleOperator] {
        match kind {
            OperatorKind::Renderer => &self.renderers,
            OperatorKind::Operator => &self.operators,
            OperatorKind::Initializer => &self.initializers,
            OperatorKind::Emitter => &self.emitters,
            OperatorKind::Force => &self.forces,
            OperatorKind::Constraint => &self.constraints,
        }
    }
}

impl ParticleOperator {
    fn from_element(element: &Element) -> Self {
        let function_name = element.get("functionName").and_then(Value::as_str).unwrap_or(&element.name);
        Self {
            function_name: function_name.to_owned(),
            name: element.name.clone(),
            attributes: Attributes(element.attributes.iter().filter(|a| a.name != "functionName").cloned().collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dmx::Header;
    use crate::fs::GameFileSystem;

    fn element(type_name: &str, name: &str, attributes: Vec<(&str, Value)>) -> Element {
        Element {
            type_name: type_name.to_owned(),
            name: name.to_owned(),
            id: [0; 16],
            attributes: attributes.into_iter().map(|(n, value)| Attribute { name: n.to_owned(), value }).collect(),
        }
    }

    #[test]
    fn definitions() {
        let refs = |v: &[usize]| Value::ElementArray(v.iter().map(|&i| Some(i)).collect());
        let text = |s: &str| Value::String(s.to_owned());
        let dmx = Dmx {
            header: Header {
                encoding: "binary".to_owned(),
                encoding_version: 5,
                format: "pcf".to_owned(),
                format_version: 2,
            },
            elements: vec![
                element("DmElement", "untitled", vec![("particleSystemDefinitions", refs(&[1]))]),
                element(
                    "DmeParticleSystemDefinition",
                    "parent",
                    vec![
                        ("renderers", refs(&[2])),
                        ("operators", refs(&[])),
                        ("emitters", refs(&[3])),
                        ("children", refs(&[4])),
                        ("max_particles", Value::Int(16)),
                        ("material", text("effects\\a.vmt")),
                        ("bounding_box_control_point", Value::Int(0)),
                    ],
                ),
                element(
                    "DmeParticleOperator",
                    "render_animated_sprites",
                    vec![("functionName", text("render_animated_sprites")), ("animation rate", Value::Float(0.5))],
                ),
                element(
                    "DmeParticleOperator",
                    "burst",
                    vec![("functionName", text("emit_instantaneously")), ("num_to_emit", Value::Int(8))],
                ),
                element(
                    "DmeParticleChild",
                    "child",
                    vec![("child", Value::Element(Some(5))), ("delay", Value::Float(0.25))],
                ),
                element("DmeParticleSystemDefinition", "unlisted", vec![("radius", Value::Float(2.0))]),
            ],
        };
        let pcf = Pcf::from_dmx(&dmx).unwrap();
        assert_eq!(pcf.systems.len(), 2);
        let parent = &pcf.systems[0];
        assert_eq!((parent.max_particles, parent.initial_particles, parent.radius), (16, 0, 5.0));
        assert_eq!(parent.material, "effects\\a.vmt");
        assert!(parent.sort_particles && parent.draw_through_leafsystem);
        assert_eq!(parent.renderers[0].function_name, "render_animated_sprites");
        assert_eq!(parent.renderers[0].attributes.get_f32("animation rate"), Some(0.5));
        assert!(parent.renderers[0].attributes.get("functionName").is_none());
        assert_eq!(
            (parent.emitters[0].name.as_str(), parent.emitters[0].function_name.as_str()),
            ("burst", "emit_instantaneously")
        );
        assert!(parent.operators.is_empty() && parent.forces.is_empty());
        assert_eq!(
            parent.children,
            vec![ParticleChild { name: "unlisted".to_owned(), index: 1, delay: 0.25, end_cap_effect: false }]
        );
        assert_eq!(parent.attributes.get_i32("bounding_box_control_point"), Some(0));
        assert!(parent.attributes.get("renderers").is_none());
        assert_eq!(pcf.systems[1].radius, 2.0);
        assert_eq!(pcf.find("PARENT").map(|s| s.name.as_str()), Some("parent"));
    }

    #[test]
    fn manifest() {
        let text = r#"particles_manifest
        {
            "file" "!particles/debug.pcf"
            "file" "Particles/Fire_01.pcf" // comment
            //"file" "particles/unused.pcf"
        }"#;
        let entries = parse_manifest(text);
        assert_eq!(
            entries,
            vec![
                ManifestEntry { path: "particles/debug.pcf".to_owned(), precache: true },
                ManifestEntry { path: "particles/fire_01.pcf".to_owned(), precache: false },
            ]
        );
    }

    /// Parses every manifest-listed and every packed `.pcf` of a local install.
    #[test]
    fn install_pcfs() {
        let Some(root) = GameFileSystem::find_install() else {
            eprintln!("Portal 2 install not found; skipping");
            return;
        };
        let fs = GameFileSystem::portal2(&root).unwrap();
        let manifest = parse_manifest(&fs.read_string(MANIFEST_PATH).expect("particle manifest"));
        assert!(!manifest.is_empty());
        let mut paths: Vec<String> = manifest.iter().map(|e| e.path.clone()).collect();
        paths.extend(fs.list("particles", Some("pcf"), true));
        paths.sort();
        paths.dedup();
        let mut systems = 0;
        for path in &paths {
            let data = fs.read(path).unwrap_or_else(|| panic!("{path} is listed but missing"));
            let pcf = Pcf::parse(&data).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(!pcf.systems.is_empty(), "{path} has no systems");
            for def in &pcf.systems {
                for kind in OperatorKind::ALL {
                    assert!(def.stack(kind).iter().all(|op| !op.function_name.is_empty()), "{path} {}", def.name);
                }
                for child in &def.children {
                    assert_eq!(pcf.systems[child.index].name, child.name);
                }
            }
            systems += pcf.systems.len();
        }
        eprintln!("{} pcf files, {systems} systems", paths.len());
    }
}
