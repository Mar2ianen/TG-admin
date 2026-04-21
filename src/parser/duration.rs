use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DurationParser;

impl DurationParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&self, input: &str) -> Result<ParsedDuration, DurationParseError> {
        parse_duration(input)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ParsedDuration {
    pub value: u64,
    pub unit: DurationUnit,
}

impl ParsedDuration {
    pub fn into_std(self) -> Duration {
        Duration::from_secs(self.value * self.unit.seconds_multiplier())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DurationUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
    Weeks,
}

impl DurationUnit {
    fn from_suffix(suffix: char) -> Result<Self, DurationParseError> {
        match suffix {
            's' => Ok(Self::Seconds),
            'm' => Ok(Self::Minutes),
            'h' => Ok(Self::Hours),
            'd' => Ok(Self::Days),
            'w' => Ok(Self::Weeks),
            _ => Err(DurationParseError::InvalidUnit(suffix)),
        }
    }

    fn seconds_multiplier(self) -> u64 {
        match self {
            Self::Seconds => 1,
            Self::Minutes => 60,
            Self::Hours => 60 * 60,
            Self::Days => 60 * 60 * 24,
            Self::Weeks => 60 * 60 * 24 * 7,
        }
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum DurationParseError {
    #[error("duration input is empty")]
    EmptyInput,
    #[error("duration must end with one of: s, m, h, d, w")]
    MissingUnit,
    #[error("invalid duration unit `{0}`")]
    InvalidUnit(char),
    #[error("invalid duration value `{0}`")]
    InvalidValue(String),
}

fn parse_duration(input: &str) -> Result<ParsedDuration, DurationParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(DurationParseError::EmptyInput);
    }

    if input.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(DurationParseError::MissingUnit);
    }

    let mut chars = input.chars();
    let suffix = chars.next_back().ok_or(DurationParseError::MissingUnit)?;
    let number = chars.as_str();
    let unit = DurationUnit::from_suffix(suffix)?;

    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) || number.starts_with('0')
    {
        return Err(DurationParseError::InvalidValue(number.to_owned()));
    }

    let value = number
        .parse::<u64>()
        .map_err(|_| DurationParseError::InvalidValue(number.to_owned()))?;

    Ok(ParsedDuration { value, unit })
}

#[cfg(test)]
mod tests {
    use super::{DurationParseError, DurationParser, DurationUnit};

    #[test]
    fn parses_supported_duration_units() {
        let parser = DurationParser::new();

        let parsed = parser.parse("7d").expect("duration parses");
        assert_eq!(parsed.value, 7);
        assert_eq!(parsed.unit, DurationUnit::Days);
        assert_eq!(parsed.into_std().as_secs(), 7 * 24 * 60 * 60);
    }

    #[test]
    fn rejects_duration_without_unit() {
        let parser = DurationParser::new();

        let err = parser.parse("30").expect_err("suffix required");
        assert_eq!(err, DurationParseError::MissingUnit);
    }

    #[test]
    fn rejects_invalid_duration_value() {
        let parser = DurationParser::new();

        let err = parser.parse("xh").expect_err("numeric value required");
        assert_eq!(err, DurationParseError::InvalidValue("x".to_owned()));
    }

    #[test]
    fn rejects_invalid_duration_unit() {
        let parser = DurationParser::new();

        let err = parser.parse("7y").expect_err("unsupported unit must fail");
        assert_eq!(err, DurationParseError::InvalidUnit('y'));
    }
}
