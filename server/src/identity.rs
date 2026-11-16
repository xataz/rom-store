use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn identifier(parts: &[&str]) -> String {
    hex::encode(Sha256::digest(parts.join("\0").as_bytes()))
}

pub fn casefold(value: &str) -> String {
    icu_casemap::CaseMapper::new()
        .fold_string(value)
        .into_owned()
}

/// Python json.dumps(sort_keys=True, ensure_ascii=True) representation. This is
/// part of the existing revision/cache format, not a new wire serialization.
pub fn python_json(value: &Value) -> String {
    match value {
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_json)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort();
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|key| format!(
                        "{}: {}",
                        python_json(&Value::String(key.clone())),
                        python_json(&values[key])
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::String(_) => {
            let encoded = serde_json::to_string(value).expect("JSON string serialization");
            let mut output = String::new();
            for c in encoded.chars() {
                if c >= '\u{7f}' {
                    for unit in c.encode_utf16(&mut [0; 2]) {
                        output.push_str(&format!("\\u{unit:04x}"));
                    }
                } else {
                    output.push(c);
                }
            }
            output
        }
        _ => value.to_string(),
    }
}
