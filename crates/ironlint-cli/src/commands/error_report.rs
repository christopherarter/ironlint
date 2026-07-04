use crate::cli::OutputFormat;
use serde_json::json;

/// Emit an error in the right format and return `code`. JSON mode writes a
/// `{"status":"error","reason":"..."}` object to STDOUT (not stderr) so a JSON
/// consumer always gets a parseable document; human mode writes `error: ...`
/// to stderr.
pub fn emit_error(format: OutputFormat, reason: &str, code: i32) -> i32 {
    match format {
        OutputFormat::Human => eprintln!("error: {reason}"),
        OutputFormat::Json => {
            let body = json!({ "status": "error", "reason": reason });
            println!("{body}");
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_error_returns_supplied_code() {
        assert_eq!(emit_error(OutputFormat::Human, "boom", 7), 7);
        assert_eq!(emit_error(OutputFormat::Json, "boom", 4), 4);
    }
}
