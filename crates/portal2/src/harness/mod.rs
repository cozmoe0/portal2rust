//! Batch modes for testing the engine against the original game: views
//! captured for side-by-side comparison (`--shots`, `--screenshot`), frame
//! timing (`--bench`) and frame recording (`--record`), whole-campaign runs
//! (`--smoke`, `--chain`), saved-game round trips (`--save-test`) and the
//! view-ray probe (`--probe`).

pub mod chain;
pub mod probe;
pub mod record;
pub mod roundtrip;
pub mod screenshot;
pub mod shots;
pub mod smoke;
