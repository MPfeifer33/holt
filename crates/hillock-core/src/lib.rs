//! hillock-core — the memory system library.
//!
//! 5 tools, one pipeline, clear data flow.
//! The MCP layer (hillock-server) is a thin wrapper around this.

pub mod embed;
pub mod engine;
pub mod index;
pub mod model;
pub mod ops;
pub mod rotation;
pub mod store;

pub use engine::Engine;
pub use model::*;
