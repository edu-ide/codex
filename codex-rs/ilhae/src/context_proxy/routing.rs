//! Team Routing — barrel re-export module.
//!
//! Re-exports team utility functions for use in prompt.rs and other modules.

// Re-export all public items from submodules so existing callers are unaffected.
pub use super::team_utils::*;
