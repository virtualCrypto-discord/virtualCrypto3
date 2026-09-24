//! Read integer values independently of JSON decimal or exponent notation.

use serde::{Deserialize, Deserializer, de::Error};
use serde_json::Value;

pub(crate) fn as_i64(value: &Value) -> Option<i64> {
    // Preserve all bits of numbers parsed directly as integers.
    if let Some(value) = value.as_i64() {
        return Some(value);
    }
    let value = value.as_f64()?;
    // The upper bound is exclusive: i64::MAX rounds to 2^63 as an f64.
    (value.fract() == 0.0 && (i64::MIN as f64..-(i64::MIN as f64)).contains(&value))
        .then_some(value as i64)
}

pub(crate) fn as_u64(value: &Value) -> Option<u64> {
    if let Some(value) = value.as_u64() {
        return Some(value);
    }
    let value = value.as_f64()?;
    // u64::MAX rounds to 2^64, which must not saturate to a valid permission mask.
    (value.fract() == 0.0 && (0.0..u64::MAX as f64).contains(&value)).then_some(value as u64)
}

pub(crate) fn deserialize_i64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    as_i64(&Value::deserialize(deserializer)?)
        .ok_or_else(|| D::Error::custom("expected an integer in the i64 range"))
}

pub(crate) fn deserialize_optional_i64<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    Option::<Value>::deserialize(deserializer)?
        .map(|value| {
            as_i64(&value).ok_or_else(|| D::Error::custom("expected an integer in the i64 range"))
        })
        .transpose()
}

pub(crate) fn deserialize_optional_i64_vec<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<i64>>, D::Error> {
    Option::<Vec<Value>>::deserialize(deserializer)?
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    as_i64(value)
                        .ok_or_else(|| D::Error::custom("expected an integer in the i64 range"))
                })
                .collect()
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decimal_and_exponent_notation_preserve_integer_values() {
        for text in ["3600", "3600.0", "3.6e3", "360000e-2"] {
            let value = serde_json::from_str(text).unwrap();
            assert_eq!(as_i64(&value), Some(3600), "{text}");
            assert_eq!(as_u64(&value), Some(3600), "{text}");
        }
        assert_eq!(as_i64(&json!(-3.0)), Some(-3));
        assert_eq!(as_u64(&json!(-0.0)), Some(0));
        assert_eq!(as_u64(&json!(-3.0)), None);
    }

    #[test]
    fn fractions_and_non_numbers_are_not_integers() {
        for value in [
            json!(1.5),
            json!(-1.5),
            json!(1e100),
            json!(-1e100),
            json!("1"),
            json!(true),
            Value::Null,
        ] {
            assert_eq!(as_i64(&value), None, "{value}");
            assert_eq!(as_u64(&value), None, "{value}");
        }
    }

    #[test]
    fn integer_boundaries_neither_round_nor_saturate() {
        assert_eq!(as_i64(&json!(i64::MIN)), Some(i64::MIN));
        assert_eq!(as_i64(&json!(i64::MAX)), Some(i64::MAX));
        assert_eq!(as_u64(&json!(u64::MAX)), Some(u64::MAX));
        assert_eq!(as_i64(&json!(u64::MAX)), None);
        assert_eq!(as_u64(&json!(i64::MIN)), None);
        assert_eq!(as_i64(&json!(i64::MAX as f64)), None);
        assert_eq!(as_u64(&json!(u64::MAX as f64)), None);
        let snowflake = 100_000_000_000_000_001_i64;
        assert_eq!(as_i64(&json!(snowflake)), Some(snowflake));
        assert_eq!(as_u64(&json!(snowflake)), Some(snowflake as u64));
    }
}
