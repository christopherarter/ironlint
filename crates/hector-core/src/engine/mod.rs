//! Engine module: the single gate-execution model.

pub mod gate;

pub use gate::{run_gate, GateEnv, GateOutcome, InternalReason};
