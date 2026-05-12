use anyhow::{anyhow, Context, Result};
use serde_yaml::Value;
use sha2::{Digest, Sha256};

/// Strip the `trust:` block and serialize keys in canonical (sorted) order.
pub fn canonicalize_for_fingerprint(input: &str) -> Result<String> {
    let mut value: Value = serde_yaml::from_str(input).context("parse yaml")?;
    if let Value::Mapping(ref mut map) = value {
        map.remove(Value::String("trust".into()));
    }
    let canonical = canonical_serialize(&value);
    Ok(canonical)
}

fn canonical_serialize(value: &Value) -> String {
    let sorted = sort_keys(value.clone());
    serde_yaml::to_string(&sorted).expect("serialize sorted yaml")
}

fn sort_keys(value: Value) -> Value {
    match value {
        Value::Mapping(m) => {
            let mut pairs: Vec<(Value, Value)> = m
                .into_iter()
                .map(|(k, v)| (k, sort_keys(v)))
                .collect();
            pairs.sort_by(|a, b| {
                serde_yaml::to_string(&a.0)
                    .unwrap_or_default()
                    .cmp(&serde_yaml::to_string(&b.0).unwrap_or_default())
            });
            let mut out = serde_yaml::Mapping::new();
            for (k, v) in pairs {
                out.insert(k, v);
            }
            Value::Mapping(out)
        }
        Value::Sequence(s) => Value::Sequence(s.into_iter().map(sort_keys).collect()),
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
    let value: Value = serde_yaml::from_str(input).context("parse yaml")?;
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
        Err(anyhow!(
            "config changed since last trust — review changes and run `hector trust` to acknowledge"
        ))
    }
}

/// Update or insert the `trust:` block in the YAML source with a fresh fingerprint.
/// Preserves the rest of the YAML structure.
pub fn write_trust_block(input: &str) -> Result<String> {
    let fp = fingerprint(input)?;
    let mut value: Value = serde_yaml::from_str(input).context("parse yaml")?;
    if let Value::Mapping(ref mut map) = value {
        map.remove(Value::String("trust".into()));
        let mut trust_map = serde_yaml::Mapping::new();
        trust_map.insert(Value::String("fingerprint".into()), Value::String(fp));
        map.insert(Value::String("trust".into()), Value::Mapping(trust_map));
    }
    Ok(serde_yaml::to_string(&value)?)
}
