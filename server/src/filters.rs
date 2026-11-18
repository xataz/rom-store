//! Bounded metadata criteria, evaluated by SQLite against the complete catalogue.
use crate::identity::casefold;
use anyhow::{bail, Result};
use std::collections::BTreeMap;

pub type Filters = BTreeMap<String, String>;

pub fn validate(filters: &Filters) -> Result<()> {
    if filters.len() > 32
        || filters.iter().any(|(key, value)| {
            key.is_empty()
                || key.len() > 128
                || !key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-.".contains(&c))
                || value.chars().count() > 256
        })
    {
        bail!("Filtres invalides (32 critères maximum, valeurs de 256 caractères maximum)");
    }
    Ok(())
}

pub fn matches(value: &str, criterion: &str, field: &str) -> bool {
    let value = if field == "year" {
        value.get(..4).unwrap_or("")
    } else {
        value
    }
    .trim();
    if value.is_empty() {
        return false;
    }
    let criterion = criterion.trim();
    for operator in [">=", "<=", ">", "<", "="] {
        if let Some(expected) = criterion.strip_prefix(operator) {
            let expected = expected.trim();
            if operator == "=" {
                return casefold(value) == casefold(expected);
            }
            let (Ok(actual), Ok(expected)) = (value.parse::<f64>(), expected.parse::<f64>()) else {
                return false;
            };
            if !actual.is_finite() || !expected.is_finite() {
                return false;
            }
            return match operator {
                ">=" => actual >= expected,
                "<=" => actual <= expected,
                ">" => actual > expected,
                _ => actual < expected,
            };
        }
    }
    if field == "players" {
        if let Ok(wanted) = criterion.parse::<u32>() {
            if let Some((min, max)) = value.split_once('-') {
                if let (Ok(min), Ok(max)) = (min.trim().parse::<u32>(), max.trim().parse::<u32>()) {
                    return (min..=max).contains(&wanted);
                }
            }
            if let Some(min) = value
                .strip_suffix('+')
                .and_then(|min| min.trim().parse::<u32>().ok())
            {
                return wanted >= min;
            }
            return value.parse::<u32>() == Ok(wanted);
        }
    }
    casefold(value).contains(&casefold(criterion))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_year_numeric_and_player_ranges() {
        assert!(matches("Straße", "STRASSE", "genre"));
        assert!(matches("19911224T000000", "1991", "year"));
        assert!(matches("19911224T000000", ">=1990", "year"));
        assert!(!matches("", "1991", "year"));
        assert!(matches("1-4", "2", "players"));
        assert!(!matches("1-4", "5", "players"));
        assert!(!matches("12", "2", "players"));
        assert!(matches("4+", "6", "players"));
        assert!(matches("0.8", ">=0.7", "rating"));
        assert!(!matches("NaN", ">=0", "rating"));
        assert!(!matches("Action RPG", "=Action", "genre"));
    }
}
