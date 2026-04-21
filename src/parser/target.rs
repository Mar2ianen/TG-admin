use crate::event::EventContext;
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedTarget {
    pub selector: ParsedTargetSelector,
    pub source: TargetSource,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum TargetSource {
    ExplicitPositional,
    SelectorFlag,
    ReplyContext,
    ImplicitContext,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum ParsedTargetSelector {
    Reply,
    Username { username: String },
    UserId { user_id: i64 },
    MessageAnchor { message_id: i32 },
    JsonSelector { raw: Value },
}

#[derive(Debug, Clone, Error, Eq, PartialEq, Serialize, Deserialize)]
pub enum TargetParseError {
    #[error("target input is empty")]
    EmptyInput,
    #[error("invalid username target `{0}`")]
    InvalidUsername(String),
    #[error("invalid target selector `{0}`")]
    InvalidSelector(String),
}

pub fn parse_target_selector(input: &str) -> Result<ParsedTargetSelector, TargetParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(TargetParseError::EmptyInput);
    }

    if matches!(input, "reply" | "reply_target") {
        return Ok(ParsedTargetSelector::Reply);
    }

    if let Some(username) = input.strip_prefix('@') {
        if username.is_empty()
            || !username
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return Err(TargetParseError::InvalidUsername(input.to_owned()));
        }

        return Ok(ParsedTargetSelector::Username {
            username: username.to_owned(),
        });
    }

    if let Some(anchor) = input
        .strip_prefix("msg:")
        .or_else(|| input.strip_prefix("message:"))
    {
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

    if let Ok(user_id) = input.parse::<i64>() {
        return Ok(ParsedTargetSelector::UserId { user_id });
    }

    Err(TargetParseError::InvalidSelector(input.to_owned()))
}

pub fn resolve_target(
    positional: Option<ParsedTargetSelector>,
    selector_flag: Option<ParsedTargetSelector>,
    event: &EventContext,
    implicit_target: impl Fn(&EventContext) -> Option<ParsedTargetSelector>,
) -> Option<ResolvedTarget> {
    positional
        .map(|selector| ResolvedTarget {
            selector,
            source: TargetSource::ExplicitPositional,
        })
        .or_else(|| {
            selector_flag.map(|selector| ResolvedTarget {
                selector,
                source: TargetSource::SelectorFlag,
            })
        })
        .or_else(|| {
            event.reply.as_ref().map(|reply| ResolvedTarget {
                selector: reply
                    .sender_user_id
                    .map(|user_id| ParsedTargetSelector::UserId { user_id })
                    .unwrap_or(ParsedTargetSelector::Reply),
                source: TargetSource::ReplyContext,
            })
        })
        .or_else(|| {
            implicit_target(event).map(|selector| ResolvedTarget {
                selector,
                source: TargetSource::ImplicitContext,
            })
        })
}

#[cfg(test)]
mod tests {
    use super::{
        ParsedTargetSelector, ResolvedTarget, TargetParseError, TargetSelectorParser, TargetSource,
        resolve_target,
    };
    use crate::event::{
        EventContext, ExecutionMode, MessageContext, ReplyContext, SystemContext, UpdateType,
    };
    use chrono::Utc;
    use serde_json::json;

    fn event_with_reply() -> EventContext {
        let mut event = EventContext::new(
            "evt_reply",
            UpdateType::Message,
            ExecutionMode::Realtime,
            SystemContext::realtime(),
        );
        event.reply = Some(ReplyContext {
            message_id: 55,
            sender_user_id: Some(9001),
            sender_username: Some("reply_user".to_owned()),
            text: Some("hi".to_owned()),
            has_media: false,
        });
        event.message = Some(MessageContext {
            id: 77,
            date: Utc::now(),
            text: Some("/del".to_owned()),
            entities: Vec::new(),
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: Some(55),
            media_group_id: None,
        });
        event
    }

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
        assert_eq!(
            parser
                .parse(r#"{"kind":"user","id":42}"#)
                .expect("json parses"),
            ParsedTargetSelector::JsonSelector {
                raw: json!({"kind": "user", "id": 42}),
            }
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

    #[test]
    fn resolves_target_in_documented_precedence_order() {
        let event = event_with_reply();

        let resolved = resolve_target(
            Some(ParsedTargetSelector::Username {
                username: "explicit".to_owned(),
            }),
            Some(ParsedTargetSelector::UserId { user_id: 42 }),
            &event,
            |_| Some(ParsedTargetSelector::MessageAnchor { message_id: 77 }),
        )
        .expect("resolved target");

        assert_eq!(
            resolved,
            ResolvedTarget {
                selector: ParsedTargetSelector::Username {
                    username: "explicit".to_owned(),
                },
                source: TargetSource::ExplicitPositional,
            }
        );
    }

    #[test]
    fn falls_back_to_reply_and_implicit_context_targets() {
        let event = event_with_reply();

        let reply_target = resolve_target(None, None, &event, |_| None).expect("reply target");
        assert_eq!(reply_target.source, TargetSource::ReplyContext);
        assert_eq!(
            reply_target.selector,
            ParsedTargetSelector::UserId { user_id: 9001 }
        );

        let event_without_reply = EventContext::new(
            "evt_implicit",
            UpdateType::Message,
            ExecutionMode::Realtime,
            SystemContext::realtime(),
        );
        let implicit = resolve_target(None, None, &event_without_reply, |_| {
            Some(ParsedTargetSelector::MessageAnchor { message_id: 77 })
        })
        .expect("implicit target");
        assert_eq!(implicit.source, TargetSource::ImplicitContext);
        assert_eq!(
            implicit.selector,
            ParsedTargetSelector::MessageAnchor { message_id: 77 }
        );
    }
}
