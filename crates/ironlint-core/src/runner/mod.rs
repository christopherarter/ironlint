mod engine;
mod path;
mod timeout;
mod tmpfile;
mod types;

pub use engine::{IronLintEngine, IronLintEngineBuilder};
pub use types::{CheckExplain, CheckInput, CheckOptions, CheckReport, ExplainOutcome};

#[cfg(test)]
pub(crate) use tmpfile::{materialize_tmpfile, sweep_stale_tmpfiles};

#[cfg(test)]
#[allow(unused_imports)]
mod tests;
