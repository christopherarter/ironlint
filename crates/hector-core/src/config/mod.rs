pub mod extends;
pub mod parser;
pub mod scope;
pub mod skip;
pub mod types;

pub use parser::{is_legacy, parse_file, parse_str, SUPPORTED_SCHEMAS};
pub use types::*;

use anyhow::Result;
use std::path::Path;

pub fn parse_file_with_extends(path: &Path) -> Result<Config> {
    extends::resolve(path)
}
