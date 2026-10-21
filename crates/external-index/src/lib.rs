//! Opt-in go-to-definition providers backed by indexes Forge does not build:
//! a user-installed Universal Ctags run on demand, and a prebuilt SCIP file at
//! the checkout root. Neither runs unless the daemon constructs it.

mod ctags;
mod process;
mod scip_index;
mod text;

pub use ctags::{CtagsProvider, ProbeError, DEFAULT_PROBE_TIMEOUT};
pub use scip_index::{ScipProvider, DEFAULT_INDEX_NAME, DEFAULT_MAX_INDEX_BYTES};
