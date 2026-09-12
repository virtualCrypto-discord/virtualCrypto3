//! `VirtualCrypto.Metadata.Validator` — the metadata rules the API rejects on.
//!
//! Limits are counted and truncated in **codepoints**, and the messages are part
//! of the response contract (`error_description_details`), so the wording and the
//! truncation have to match exactly:
//!
//! * a key over 40 codepoints yields `too large(max: 40) metadata key(<first 41>)...`;
//! * a string value over 500 codepoints yields
//!   `too large metadata value(max: 500) at <key>(<value>)`, where each part is
//!   itself truncated by [`slice_string`];
//! * any other non-null value yields
//!   `invalid value at <key>(value must be string or null)`;
//! * more than 50 entries (null values are not counted) yields
//!   `too many entries in metadata(max: 50)`, reported first.
//!
//! The order of the reported details is not part of the contract; the Elixir
//! enumeration order is not reproduced.

use serde_json::Value;

pub const MAX_KEY_CODEPOINTS: usize = 40;
pub const MAX_VALUE_CODEPOINTS: usize = 500;
pub const MAX_ENTRIES: usize = 50;

/// `slice_string/2`: truncate to `max + 1` codepoints and append an ellipsis,
/// but only when the string is actually longer than `max`.
pub fn slice_string(value: &str, max: usize) -> String {
    if value.chars().count() > max {
        format!("{}...", take_codepoints(value, max + 1))
    } else {
        value.to_string()
    }
}

fn take_codepoints(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

pub fn validate_metadata_key(key: &str) -> Option<String> {
    if key.chars().count() > MAX_KEY_CODEPOINTS {
        Some(format!(
            "too large(max: {MAX_KEY_CODEPOINTS}) metadata key({}...)",
            take_codepoints(key, MAX_KEY_CODEPOINTS + 1)
        ))
    } else {
        None
    }
}

pub fn validate_metadata_value(key: &str, value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            if text.chars().count() > MAX_VALUE_CODEPOINTS {
                Some(format!(
                    "too large metadata value(max: {MAX_VALUE_CODEPOINTS}) at {}({})",
                    slice_string(key, MAX_KEY_CODEPOINTS),
                    slice_string(text, MAX_VALUE_CODEPOINTS)
                ))
            } else {
                None
            }
        }
        Value::Null => None,
        _ => Some(format!(
            "invalid value at {}(value must be string or null)",
            slice_string(key, MAX_KEY_CODEPOINTS)
        )),
    }
}

/// Validate a metadata document, returning every problem found.
///
/// A non-object is accepted here: the `metadata_must_be_object` check constraint
/// is what rejects it, exactly as it does in Elixir.
pub fn validate(metadata: &Value) -> Vec<String> {
    let Some(entries) = metadata.as_object() else {
        return Vec::new();
    };

    let mut errors = Vec::new();
    let mut counted = 0;

    for (key, value) in entries {
        // `validate_metadata_entry/1` reports the key problem before the value one.
        if let Some(error) = validate_metadata_key(key) {
            errors.push(error);
        }
        if let Some(error) = validate_metadata_value(key, value) {
            errors.push(error);
        }

        if !value.is_null() {
            counted += 1;
        }
    }

    if counted > MAX_ENTRIES {
        // `validate_metadata/3` prepends this to the accumulated errors.
        errors.insert(
            0,
            format!("too many entries in metadata(max: {MAX_ENTRIES})"),
        );
    }

    errors
}

/// Entries are iterated in the order `serde_json::Map` yields, which is sorted by
/// key; Elixir's enumeration order is not part of the contract.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    #[test]
    fn short_values_pass() {
        assert!(validate(&json!({ "a": "b", "c": "d" })).is_empty());
        assert!(validate(&json!({})).is_empty());
    }

    #[test]
    fn a_long_key_is_reported_with_the_first_41_codepoints() {
        let key = "b".repeat(41);
        let errors = validate(&json!({ key: "x" }));

        assert_eq!(
            errors,
            vec![format!(
                "too large(max: 40) metadata key({}...)",
                "b".repeat(41)
            )]
        );
    }

    #[test]
    fn a_long_value_is_reported_with_both_truncations() {
        let errors = validate(&json!({ "x": "b".repeat(501) }));

        assert_eq!(
            errors,
            vec![format!(
                "too large metadata value(max: 500) at x({}...)",
                "b".repeat(501)
            )]
        );
    }

    #[test]
    fn a_non_string_non_null_value_is_invalid() {
        assert_eq!(
            validate(&json!({ "a": 1 })),
            vec!["invalid value at a(value must be string or null)".to_string()]
        );
    }

    #[test]
    fn null_values_do_not_count_towards_the_entry_limit() {
        let mut metadata = Map::new();
        for index in 0..51 {
            metadata.insert(index.to_string(), Value::Null);
        }

        assert!(validate(&Value::Object(metadata)).is_empty());
    }

    #[test]
    fn more_than_fifty_entries_is_reported_first() {
        let mut source = Map::new();
        for index in 1..=51 {
            source.insert(index.to_string(), Value::String(index.to_string()));
        }

        let errors = validate(&Value::Object(source));

        assert_eq!(
            errors,
            vec!["too many entries in metadata(max: 50)".to_string()]
        );
    }
}
