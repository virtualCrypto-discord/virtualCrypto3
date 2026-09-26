//! Discord IDs are positive ASCII decimal strings within the database's bigint range.
//! Amounts have separate parsing rules; never use a signed amount parser for an ID.
pub(crate) fn parse(value: &str) -> Option<i64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<i64>().ok().filter(|id| *id > 0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn accepts_exact_ids_and_rejects_invalid_targets() {
        for value in ["0", "-1", "+1", " 1", "1 ", "", "１", "9223372036854775808"] {
            assert_eq!(super::parse(value), None, "{value}");
        }
        assert_eq!(super::parse("9007199254740993"), Some(9007199254740993));
        assert_eq!(super::parse("9223372036854775807"), Some(i64::MAX));
        assert_eq!(super::parse("001"), Some(1));
    }
}
