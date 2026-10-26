//! CodeCurrency core engine.
//!
//! Phase 1 proves one claim: a single command can build, test, measure, and
//! explain a solution, and produce a result anyone else can replay to the same
//! answer. Everything here serves that claim.

pub mod capsule;
pub mod engine;
pub mod explain;
pub mod host;
pub mod measure;
pub mod problem;
pub mod sandbox;
pub mod toolchain;

pub use capsule::*;
