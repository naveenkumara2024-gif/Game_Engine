//! Engine core. Bottom of the crate graph: depends on no other `ge_*` crate (CLAUDE.md §5).

pub mod cvar;
pub mod handle;
pub mod log;
pub mod name;

pub use cvar::{CVar, CVarFlags, CVarValue};
pub use handle::{Handle, Pool};
pub use name::Name;
