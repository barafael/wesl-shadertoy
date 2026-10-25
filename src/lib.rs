//! The engine-free half of wesl-shadertoy: the WESL → WGSL [`compile`]r, the
//! live-[`tweak`] rewrite it applies, and the bundled [`examples`]. The app
//! (`src/main.rs`) re-exports these modules under their usual `crate::`
//! paths; they are a library so that tooling which has no use for Bevy, such
//! as the `backend-diff` test crate, can compose shaders exactly like the app.

pub mod compile;
pub mod examples;
pub mod heat;
pub mod pragmas;
pub mod tweak;
