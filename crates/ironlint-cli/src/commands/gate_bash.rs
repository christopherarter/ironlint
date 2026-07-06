//! The `ironlint gate-bash` built-in: read a command on stdin, classify it
//! via `ironlint_bash_gate::decide`, and emit the binary exit contract
//! (0 = allow, 2 = block with the reason on stdout). The adapters' Bash
//! branches shell out to this; it is the single source of the deny decision.
//!
//! Not a `check`: no config load, no trust gate, no per-check spawn. The
//! bash-gate must run even when `.ironlint.yml` is missing or untrusted —
//! that is exactly when the agent is most motivated to run `ironlint trust`.

use anyhow::Result;
use std::io::Read;

pub fn run() -> Result<i32> {
    // Read the whole command from stdin. The adapters pipe it in; the
    // pre-filter guarantees the bytes contained a UTF8-decodable `ironlint`
    // or `.ironlint` substring, so lossy decoding is safe for the matcher.
    // A genuinely malformed (non-UTF8) stdin is unreachable in practice but
    // defended: allow + log, never crash.
    let mut buf = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut buf) {
        eprintln!("ironlint gate-bash: could not read stdin: {e}");
        return Ok(0);
    }
    let command = if let Ok(s) = std::str::from_utf8(&buf) {
        s
    } else {
        eprintln!("ironlint gate-bash: non-UTF8 stdin — allowing (unreachable via pre-filter)");
        return Ok(0);
    };

    match ironlint_bash_gate::decide(command) {
        ironlint_bash_gate::Decision::Allow => Ok(0),
        ironlint_bash_gate::Decision::Block(reason) => {
            print!("{reason}");
            Ok(2)
        }
    }
}
