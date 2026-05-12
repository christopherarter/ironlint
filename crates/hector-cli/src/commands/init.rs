use anyhow::{anyhow, Result};
use std::path::Path;

pub fn run(dir: &Path) -> Result<i32> {
    let cfg_path = dir.join(".hector.yml");
    if cfg_path.exists() {
        return Err(anyhow!(
            "{} already exists; refusing to overwrite",
            cfg_path.display()
        ));
    }
    let stack = detect_stack(dir);
    let body = match stack {
        Stack::Rust => RUST_TEMPLATE,
        Stack::Node => NODE_TEMPLATE,
        Stack::Python => PYTHON_TEMPLATE,
        Stack::Unknown => GENERIC_TEMPLATE,
    };
    std::fs::write(&cfg_path, body)?;
    println!("scaffolded: {}", cfg_path.display());
    println!("review the config, then run: hector trust");
    Ok(0)
}

#[derive(Debug)]
enum Stack {
    Rust,
    Node,
    Python,
    Unknown,
}

fn detect_stack(dir: &Path) -> Stack {
    if dir.join("Cargo.toml").exists() {
        return Stack::Rust;
    }
    if dir.join("package.json").exists() {
        return Stack::Node;
    }
    if dir.join("pyproject.toml").exists() || dir.join("setup.py").exists() {
        return Stack::Python;
    }
    Stack::Unknown
}

// Grep exit-code routing for script rules.
//
// Naive idiom `grep PATTERN file && exit 1 || exit 0` collapses grep's
// exit-2 (regex/parse error, unreadable file, binary refusal) into a
// pass — a broken rule silently passes forever (P2-9).
//
// The case-statement form routes each exit explicitly:
//   - 0 (match found)     → exit 1   → script-engine reports violation
//   - 1 (no match)        → exit 0   → pass
//   - * (grep error >=2)  → exit $?  → runner treats as violation, so a
//                                       broken regex fails loudly
//
// Kept on a single YAML line (POSIX `;`-separated) so the templated
// `script:` value remains a scalar.
const RUST_TEMPLATE: &str = r#"schema_version: 2

rules:
  no-unwrap-in-src:
    description: "Avoid .unwrap() in non-test source. Use ? or expect with context."
    engine: script
    scope: ["src/**/*.rs"]
    severity: warning
    script: "grep -nE '\\.unwrap\\(\\)' {file}; case $? in 0) exit 1;; 1) exit 0;; *) exit $?;; esac"
"#;

const NODE_TEMPLATE: &str = r#"schema_version: 2

rules:
  no-console-log:
    description: "No console.log in committed source."
    engine: script
    scope: ["src/**/*.ts", "src/**/*.tsx", "src/**/*.js"]
    severity: error
    script: "grep -nE 'console\\.log\\(' {file}; case $? in 0) exit 1;; 1) exit 0;; *) exit $?;; esac"
"#;

const PYTHON_TEMPLATE: &str = r#"schema_version: 2

rules:
  ruff-check:
    description: "Code must pass ruff check."
    engine: script
    scope: ["**/*.py"]
    severity: error
    script: "ruff check --quiet {file}"
"#;

const GENERIC_TEMPLATE: &str = r#"schema_version: 2

rules:
  no-fixme:
    description: "Don't commit FIXME markers."
    engine: script
    scope: ["*"]
    severity: warning
    script: "grep -nE 'FIXME' {file}; case $? in 0) exit 1;; 1) exit 0;; *) exit $?;; esac"
"#;
