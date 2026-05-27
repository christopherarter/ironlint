use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

/// Strip the `trust:` block and serialize to canonical JSON for hashing.
///
/// Canonicalization route: YAML → serde_yaml::Value → serde_json::Value
/// (with recursive key sorting via BTreeMap) → serde_json::to_string.
/// RFC 8259 normatively specifies the JSON byte form, so this output is
/// stable across serde_yaml emitter changes (which are not normative).
///
/// Anchors and aliases (`&name`/`*name`) are rejected pre-parse because
/// serde_yaml 0.9 expands them transparently before building the Value tree,
/// so post-parse detection is not possible. We reject them to prevent
/// serde_yaml's expansion from silently flattening structural ambiguity.
pub fn canonicalize_for_fingerprint(input: &str) -> Result<String> {
    // Pre-parse anchor/alias guard: reject any YAML that contains anchor
    // definitions (&name) or alias references (*name) outside of quoted strings.
    // serde_yaml 0.9 expands anchors at the Event level — the Value tree never
    // preserves them — so detection must happen here on the raw bytes.
    if contains_yaml_anchor_or_alias(input) {
        anyhow::bail!("YAML anchors/tags are not supported in trust fingerprint");
    }
    let mut yaml_value: serde_yaml::Value = serde_yaml::from_str(input).context("parse yaml")?;
    // Strip the trust block before canonicalization.
    if let serde_yaml::Value::Mapping(ref mut map) = yaml_value {
        map.remove(serde_yaml::Value::String("trust".into()));
    }
    // Convert YAML → JSON (lossless for our schema; we don't use
    // binary scalars, anchors-as-values, or complex keys).
    let json_value = yaml_to_json(yaml_value)?;
    let sorted = sort_json_keys(json_value);
    Ok(serde_json::to_string(&sorted)?)
}

/// Scan raw YAML for anchor definitions (`&name`) or alias references (`*name`)
/// that appear outside quoted strings. Returns true if any are found.
///
/// This is a heuristic scan, not a full YAML parser. It detects the common
/// cases: `&anchor` after a mapping key on the same line, and `*alias` as
/// a standalone value. Two known false-positive paths, both fail-safe
/// (rejection, not silent misaccept):
///
/// - **Unquoted scalars** containing `&word`/`*word` (e.g.
///   `description: Foo&Bar`, or a script value with a shell-glob like
///   `grep *main`).
/// - **Double-quoted strings with backslash-escaped quotes** like
///   `script: "grep -E \"pattern\" {file} && exit 0"` — `\"` is not
///   parsed as an escape sequence here, so `in_double` toggles off
///   prematurely and a later `&word`/`*word` inside the same string
///   is seen as unquoted. Workaround: rewrite the string with single
///   quotes or use YAML's `|`/`>` block-scalar styles.
///
/// Both cases are uncommon; both fail in the safe direction (operator
/// sees a clear error and can re-author the config).
fn contains_yaml_anchor_or_alias(input: &str) -> bool {
    for line in input.lines() {
        // Strip the leading whitespace then check if this is a quoted value
        let trimmed = line.trim();
        // Skip pure comment lines
        if trimmed.starts_with('#') {
            continue;
        }
        // Look for & or * outside of quoted regions
        let mut in_single = false;
        let mut in_double = false;
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                '#' if !in_single && !in_double => break, // rest is comment
                '&' | '*' if !in_single && !in_double => {
                    // Confirm the next char is alphanumeric (anchor/alias name start)
                    if i + 1 < chars.len()
                        && (chars[i + 1].is_alphanumeric() || chars[i + 1] == '_')
                    {
                        return true;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
    false
}

fn yaml_to_json(v: serde_yaml::Value) -> Result<serde_json::Value> {
    Ok(match v {
        serde_yaml::Value::Null => serde_json::Value::Null,
        serde_yaml::Value::Bool(b) => serde_json::Value::Bool(b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                serde_json::Value::Number(u.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| anyhow!("non-finite number in trust fingerprint"))?
            } else {
                return Err(anyhow!("unsupported number in trust fingerprint: {n:?}"));
            }
        }
        serde_yaml::Value::String(s) => serde_json::Value::String(s),
        serde_yaml::Value::Sequence(seq) => {
            let items: Result<Vec<_>> = seq.into_iter().map(yaml_to_json).collect();
            serde_json::Value::Array(items?)
        }
        serde_yaml::Value::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    serde_yaml::Value::String(s) => s,
                    other => {
                        return Err(anyhow!(
                            "trust fingerprint requires string keys, got {other:?}"
                        ))
                    }
                };
                obj.insert(key, yaml_to_json(v)?);
            }
            serde_json::Value::Object(obj)
        }
        serde_yaml::Value::Tagged(_) => {
            return Err(anyhow!(
                "YAML anchors/tags are not supported in trust fingerprint"
            ));
        }
    })
}

fn sort_json_keys(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let sorted: std::collections::BTreeMap<String, serde_json::Value> = map
                .into_iter()
                .map(|(k, v)| (k, sort_json_keys(v)))
                .collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(sort_json_keys).collect())
        }
        other => other,
    }
}

/// Compute the sha256 fingerprint of a config, prefixed with `sha256:`.
pub fn fingerprint(input: &str) -> Result<String> {
    let canonical = canonicalize_for_fingerprint(input)?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    Ok(format!("sha256:{:x}", digest))
}

/// Verify that the `trust.fingerprint` field of the config equals the recomputed fingerprint.
pub fn verify(input: &str) -> Result<()> {
    let value: serde_yaml::Value = serde_yaml::from_str(input).context("parse yaml")?;
    let recorded = value
        .get("trust")
        .and_then(|t| t.get("fingerprint"))
        .and_then(|f| f.as_str())
        .ok_or_else(|| anyhow!("trust block missing or empty; run `hector trust`"))?
        .to_string();
    let expected = fingerprint(input)?;
    if recorded == expected {
        Ok(())
    } else {
        anyhow::bail!(
            "trust fingerprint mismatch — config body has changed since `hector trust`. \
             If you just upgraded hector, the canonicalization algorithm changed in 0.2; \
             run `hector trust <path>` to re-sign. Otherwise inspect the diff."
        )
    }
}

/// Update or insert the `trust:` block in the YAML source with a fresh fingerprint.
///
/// Performs a string-level edit so comments, key order, and scalar style in the
/// rest of the file are preserved verbatim (P2-7). The fingerprint itself is
/// computed via [`fingerprint`], which canonicalizes the YAML semantically and
/// is unaffected by comments or formatting.
///
/// If a top-level `trust:` block already exists, it is replaced in place. The
/// block is identified as a line starting with `trust:` at column 0 and ending
/// at the next top-level key (or EOF). Otherwise, a fresh block is appended.
pub fn write_trust_block(input: &str) -> Result<String> {
    let fp = fingerprint(input)?;
    let new_block = format!("trust:\n  fingerprint: {fp}\n");

    let lines: Vec<&str> = input.lines().collect();
    let trust_start = lines.iter().position(|l| {
        // Top-level `trust:` key (column 0, no leading whitespace). The trust
        // block is always written `trust:\n` with the fingerprint on the
        // following line, so an exact match on `trust:` (after stripping
        // trailing whitespace and inline comments) is sufficient and
        // unambiguously avoids matching `trusted:`, `trust_chain:`, etc.
        let no_comment = match l.find('#') {
            Some(i) => &l[..i],
            None => l,
        };
        no_comment.trim_end() == "trust:"
    });

    if let Some(start) = trust_start {
        // End-of-block: first subsequent non-empty line whose first byte is
        // not whitespace (i.e. another top-level key) — or EOF.
        let end = (start + 1..lines.len())
            .find(|i| {
                let l = lines[*i];
                !l.is_empty() && !l.starts_with(' ') && !l.starts_with('\t')
            })
            .unwrap_or(lines.len());

        let mut out = String::with_capacity(input.len() + new_block.len());
        if start > 0 {
            for l in &lines[..start] {
                out.push_str(l);
                out.push('\n');
            }
        }
        out.push_str(&new_block);
        if end < lines.len() {
            for l in &lines[end..] {
                out.push_str(l);
                out.push('\n');
            }
            // Preserve trailing-newline shape of the original.
            if !input.ends_with('\n') {
                out.pop();
            }
        }
        return Ok(out);
    }

    // No existing trust block — append at EOF.
    let mut out = String::with_capacity(input.len() + new_block.len() + 1);
    out.push_str(input);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&new_block);
    Ok(out)
}
