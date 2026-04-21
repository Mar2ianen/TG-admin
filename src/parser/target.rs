use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TargetSelectorParser;

impl TargetSelectorParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&self, input: &str) -> Result<ParsedTargetSelector, TargetParseError> {
        parse_target_selector(input)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedTargetSelector {
    Reply,
    Username { username: String },
    UserId { user_id: i64 },
    MessageAnchor { message_id: i32 },
    JsonSelector { raw: Value },
    ImplicitContext,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum TargetParseError {
    #[error("target input is empty")]
    EmptyInput,
    #[error("invalid username target `{0}`")]
    InvalidUsername(String),
    #[error("invalid target selector `{0}`")]
    InvalidSelector(String),
}

fn parse_target_selector(input: &str) -> Result<ParsedTargetSelector, TargetParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(TargetParseError::EmptyInput);
    }

    if matches!(input, "reply" | "reply_target") {
        return Ok(ParsedTargetSelector::Reply);
    }

    if matches!(input, "implicit" | "context") {
        return Ok(ParsedTargetSelector::ImplicitContext);
    }

    if let Some(username) = input.strip_prefix('@') {
        if username.is_empty()
            || !username
                .chars()
                .all(|char| char.is_ascii_alphanumeric() || char == '_')
        {
            return Err(TargetParseError::InvalidUsername(input.to_owned()));
        }

        return Ok(ParsedTargetSelector::Username {
            username: username.to_owned(),
        });
    }

    if let Ok(user_id) = input.parse::<i64>() {
        return Ok(ParsedTargetSelector::UserId { user_id });
    }

    if let Some(anchor) = input.strip_prefix("msg:") {
        let message_id = anchor
            .parse::<i32>()
            .map_err(|_| TargetParseError::InvalidSelector(input.to_owned()))?;
        return Ok(ParsedTargetSelector::MessageAnchor { message_id });
    }

    if input.starts_with('{') {
        let raw = serde_json::from_str(input)
            .map_err(|_| TargetParseError::InvalidSelector(input.to_owned()))?;
        return Ok(ParsedTargetSelector::JsonSelector { raw });
    }

    Err(TargetParseError::InvalidSelector(input.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{ParsedTargetSelector, TargetParseError, TargetSelectorParser};

    #[test]
    fn parses_supported_target_selector_forms() {
        let parser = TargetSelectorParser::new();

        assert_eq!(
            parser.parse("@spam_user").expect("username parses"),
            ParsedTargetSelector::Username {
                username: "spam_user".to_owned(),
            }
        );
        assert_eq!(
            parser.parse("12345").expect("id parses"),
            ParsedTargetSelector::UserId { user_id: 12345 }
        );
        assert_eq!(
            parser.parse("msg:42").expect("anchor parses"),
            ParsedTargetSelector::MessageAnchor { message_id: 42 }
        );
    }

    #[test]
    fn rejects_invalid_target_selector() {
        let parser = TargetSelectorParser::new();

        let err = parser.parse("@bad-name").expect_err("invalid username");
        assert_eq!(
            err,
            TargetParseError::InvalidUsername("@bad-name".to_owned())
        );
    }
}
