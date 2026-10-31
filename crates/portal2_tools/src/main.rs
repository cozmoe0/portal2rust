//! p2tools: small probes into the game's files, used while working out how
//! the engine should behave (formats, entity statistics, particle and scene
//! dumps...). Each command reads your Portal 2 installation (found like the
//! game does, or through `PORTAL2_DIR`) and prints what it finds.
//!
//! ```text
//! cargo run --release -p portal2_tools -- <command> [arguments]
//! cargo run --release -p portal2_tools -- help
//! ```

use source_formats::fs::GameFileSystem;

mod cmd {
    pub mod actorscan;
    pub mod brushesat;
    pub mod bspcheck;
    pub mod bspphys;
    pub mod cubecheck;
    pub mod devicemaps;
    pub mod entdump;
    pub mod entnum;
    pub mod entpos;
    pub mod entstats;
    pub mod faceat;
    pub mod fgdcheck;
    pub mod fgdkeys;
    pub mod gamecat;
    pub mod gamels;
    pub mod iochain;
    pub mod iofind;
    pub mod kvkeys;
    pub mod lightscale;
    pub mod lmscale;
    pub mod locfind;
    pub mod luxelat;
    pub mod m8survey;
    pub mod matfaces;
    pub mod mdlattach;
    pub mod mdlevents;
    pub mod mdlinfo;
    pub mod mdlscan;
    pub mod mdlseq;
    pub mod mdluv;
    pub mod modelbounds;
    pub mod modelbrushes;
    pub mod modellights;
    pub mod openspace;
    pub mod overlayfaces;
    pub mod overlays;
    pub mod p2dump;
    pub mod particlesurvey;
    pub mod pcfdump;
    pub mod phyaxes;
    pub mod phybones;
    pub mod phycheck;
    pub mod phyinfo;
    pub mod phypieces;
    pub mod portalable;
    pub mod portalfaces;
    pub mod propcube;
    pub mod propshape;
    pub mod propsnear;
    pub mod scenefind;
    pub mod shaderprops;
    pub mod skyfaces;
    pub mod skyinfo;
    pub mod sndscript;
    pub mod spritemodes;
    pub mod texinfo;
    pub mod vcddump;
    pub mod vfontinfo;
    pub mod vhvcheck;
    pub mod vhvscale;
    pub mod vmtparams;
    pub mod vpkgrep;
    pub mod vtfinfo;
    pub mod wavinfo;
}

/// Every command: name, what it does, entry point.
const COMMANDS: &[(&str, &str, fn())] = &[
    ("actorscan", "Survey for actor animation (names and counts only)", cmd::actorscan::main),
    ("brushesat", "Lists brush entities whose model bounds contain a point, with their class, name and bounds.", cmd::brushesat::main),
    ("bspcheck", "Winding and lightmap brightness diagnostics.", cmd::bspcheck::main),
    ("bspphys", "Lists the physics collision pieces of a brush model from the map's physics lump", cmd::bspphys::main),
    ("cubecheck", "Reports formats and value ranges of a map's baked cube maps.", cmd::cubecheck::main),
    ("devicemaps", "Counts puzzle-device entities per single-player map (counts only).", cmd::devicemaps::main),
    (
        "entdump",
        "Prints every keyvalue of entities whose targetname or classname contains a substring (case-insensitive).",
        cmd::entdump::main,
    ),
    ("entnum", "Prints the numeric keyvalues of one entity (by lump index)", cmd::entnum::main),
    ("entpos", "Prints origins of entities whose classname contains a substring.", cmd::entpos::main),
    ("entstats", "Structural entity statistics across maps (counts only)", cmd::entstats::main),
    ("faceat", "Lists the faces passing through a point with their materials", cmd::faceat::main),
    ("fgdcheck", "Parses the installed entity definitions and reports coverage (counts only).", cmd::fgdcheck::main),
    (
        "fgdkeys",
        "Lists keyvalue names and defaults the installed entity definitions declare for given classes (names and defaults only).",
        cmd::fgdkeys::main,
    ),
    ("gamecat", "Prints a file from the installed game's search paths (VPKs included) to stdout.", cmd::gamecat::main),
    ("gamels", "Lists game files under a directory (recursively), optionally with one extension", cmd::gamels::main),
    ("iochain", "Walks the I/O graph backwards from every entity of a class", cmd::iochain::main),
    (
        "iofind",
        "Lists entity outputs whose target, input or parameter contains a substring (case-insensitive), with the sending entity's name, class and position.",
        cmd::iofind::main,
    ),
    ("kvkeys", "Lists the key structure of a KeyValues file (keys and short values only).", cmd::kvkeys::main),
    (
        "lightscale",
        "Compares lightmap values on floors with the direct light predicted from the map's world lights (PVS-visible point and spot lights only).",
        cmd::lightscale::main,
    ),
    ("lmscale", "Compares floor lightmap luxels with the +Z ambient cube face just above them.", cmd::lmscale::main),
    ("luxelat", "Prints a face's lightmap luxel nearest a world point.", cmd::luxelat::main),
    ("locfind", "Searches the English localization files for tokens whose key or text contains a substring", cmd::locfind::main),
    ("m8survey", "Counts the entities and surfaces visual-parity work cares about, per singleplayer map", cmd::m8survey::main),
    ("matfaces", "Lists the faces whose material name contains a substring, with centre, normal and size", cmd::matfaces::main),
    ("mdlattach", "Lists a model's attachments with their bones", cmd::mdlattach::main),
    ("mdlevents", "Lists the events of a model's sequences", cmd::mdlevents::main),
    ("mdlinfo", "Loads studio models and prints structural stats (no content dump).", cmd::mdlinfo::main),
    ("mdlscan", "Histogram of MDL / VVD / VTX header versions across the install.", cmd::mdlscan::main),
    (
        "mdlseq",
        "Prints a model's pose parameters, bones (with `--bones`) and sequences (flags, blend grid, pose parameter axes).",
        cmd::mdlseq::main,
    ),
    ("mdluv", "Prints the range of a model's texture coordinates and normals.", cmd::mdluv::main),
    ("modelbounds", "Prints a brush model's bounds", cmd::modelbounds::main),
    ("modelbrushes", "Lists a brush model's brushes with their contents and bounds", cmd::modelbrushes::main),
    ("modellights", "Prints the lighting inputs a dynamic model at a point would see", cmd::modellights::main),
    ("openspace", "Distances from a point to solid world geometry along the six axes, for placing cameras", cmd::openspace::main),
    ("overlayfaces", "Prints the faces an overlay covers", cmd::overlayfaces::main),
    ("overlays", "Lists a map's overlays by material, with the material's shader and the number of faces they cover", cmd::overlays::main),
    ("p2dump", "Diagnostic dump of a Portal 2 installation", cmd::p2dump::main),
    (
        "particlesurvey",
        "Which particle systems the singleplayer maps start, and which operators those systems (with their children) need",
        cmd::particlesurvey::main,
    ),
    ("pcfdump", "Lists particle systems from the install's `.pcf` files with their operator function names.", cmd::pcfdump::main),
    (
        "phyaxes",
        "Measures which axis mapping makes collision points line up with model hull bounds, across many installed models.",
        cmd::phyaxes::main,
    ),
    ("phybones", "Tests aligning collision solids with their bone's default pose.", cmd::phybones::main),
    ("phycheck", "Validates collision hulls", cmd::phycheck::main),
    ("phyinfo", "Collision pieces of a model's .phy", cmd::phyinfo::main),
    ("phypieces", "Prints the collision solids of a model's `.phy`", cmd::phypieces::main),
    (
        "portalable",
        "Counts drawn faces whose surface accepts portals (no no-portal flag and no %noportal material).",
        cmd::portalable::main,
    ),
    ("portalfaces", "Lists the largest portal-accepting wall/floor faces near a point (centre, normal, area).", cmd::portalfaces::main),
    (
        "propcube",
        "For static props near a point, prints the lighting origin the engine uses (the lump's when flagged, else the model's illumination point placed with the prop) and the cube map sample nearest to it.",
        cmd::propcube::main,
    ),
    ("propshape", "Compares a map entity class's collision bounds with its model bounds.", cmd::propshape::main),
    ("propsnear", "Lists static props whose origin lies within a radius of a point", cmd::propsnear::main),
    (
        "scenefind",
        "Lists compiled scenes with an actor (and optionally an event kind) matching the arguments, named by the paths SP maps and scripts reference",
        cmd::scenefind::main,
    ),
    (
        "shaderprops",
        "Lists the static props and model entities of maps whose materials (in the skin they use) are drawn with a given shader, or with `env_cubemap` as the shader name, whose materials reflect the nearest cube map",
        cmd::shaderprops::main,
    ),
    ("skyfaces", "Lists the faces that show the sky (`SURF_SKY` / `SURF_SKY2D`) of a map", cmd::skyfaces::main),
    ("skyinfo", "Per singleplayer map", cmd::skyinfo::main),
    ("sndscript", "Prints a sound script entry", cmd::sndscript::main),
    (
        "spritemodes",
        "Counts env_sprite / env_glow render modes, materials and spawn flags over the singleplayer maps",
        cmd::spritemodes::main,
    ),
    ("texinfo", "Prints a texinfo's material and the material's text", cmd::texinfo::main),
    ("vcddump", "Prints a choreographed scene.", cmd::vcddump::main),
    ("vfontinfo", "Decodes a `.vfont` and lists its TrueType tables", cmd::vfontinfo::main),
    ("vhvcheck", "Compares static prop lighting file layout with model mesh vertex counts.", cmd::vhvcheck::main),
    (
        "vhvscale",
        "Compares baked static prop vertex colours with ambient-cube brightness at the prop origin under different decodings.",
        cmd::vhvscale::main,
    ),
    ("vmtparams", "Prints selected numeric parameters of the materials a map uses, filtered by shader.", cmd::vmtparams::main),
    (
        "vpkgrep",
        "Prints lines of game files (under a directory, with an extension) that contain a substring, case-insensitively",
        cmd::vpkgrep::main,
    ),
    ("vtfinfo", "Prints the header of textures from the installed game", cmd::vtfinfo::main),
    ("wavinfo", "Prints format, length, loop points and metadata of sound files.", cmd::wavinfo::main),
];

/// The command line as a command sees it: the program name, then the
/// arguments after the command's name.
pub fn args() -> impl Iterator<Item = String> {
    let mut all = std::env::args();
    let program = all.next().unwrap_or_default();
    std::iter::once(program).chain(all.skip(1))
}

/// The folder of the Portal 2 installation; exits with a hint if there is none.
pub fn install_root() -> std::path::PathBuf {
    GameFileSystem::find_install().unwrap_or_else(|| {
        eprintln!("Portal 2 installation not found. Install it through Steam or set PORTAL2_DIR to the folder with portal2.exe.");
        std::process::exit(1)
    })
}

/// The game's layered filesystem (loose folders and VPKs), as the game mounts it.
pub fn game_fs() -> GameFileSystem {
    let root = install_root();
    GameFileSystem::portal2(&root).unwrap_or_else(|e| {
        eprintln!("could not mount {}: {e}", root.display());
        std::process::exit(1)
    })
}

fn help() {
    println!("usage: p2tools <command> [arguments]\n");
    for (name, summary, _) in COMMANDS {
        println!("  {name:<16} {summary}");
    }
}

fn main() {
    let Some(command) = std::env::args().nth(1) else {
        help();
        return;
    };
    match COMMANDS.iter().find(|(name, _, _)| *name == command) {
        Some((_, _, run)) => run(),
        None if command == "help" || command == "--help" || command == "-h" => help(),
        None => {
            eprintln!("unknown command '{command}'; `p2tools help` lists them");
            std::process::exit(2);
        }
    }
}
