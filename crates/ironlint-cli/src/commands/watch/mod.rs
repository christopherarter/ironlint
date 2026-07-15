mod render;
mod runtime;
mod state;

// Public facade paths for external callers; stream/explorer are not used by
// the binary runtime itself, so retain this narrow, documented lint allowance.
#[allow(unused_imports)]
pub use render::{explorer_lines, stream_lines, ui, StreamRow};
pub use runtime::run;
pub use state::{handle_key, Loop, View, ViewState};

#[cfg(test)]
mod tests;
