use crate::event::EventContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

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
        validate_json_selector(&raw, input)?;
        return Ok(ParsedTargetSelector::JsonSelector { raw });
    }

    if let Ok(user_id) = input.parse::<i64>() {
        return Ok(ParsedTargetSelector::UserId { user_id });
    }

    Err(TargetParseError::InvalidSelector(input.to_owned()))
}

fn validate_json_selector(raw: &Value, input: &str) -> Result<(), TargetParseError> {
    let object = raw
        .as_object()
        .ok_or_else(|| TargetParseError::InvalidSelector(input.to_owned()))?;

    if object.is_empty() {
        return Err(TargetParseError::InvalidSelector(input.to_owned()));
    }

    for key in object.keys() {
        if !matches!(key.as_str(), "kind" | "id" | "username") {
            return Err(TargetParseError::InvalidSelector(input.to_owned()));
        }
    }

    if let Some(kind) = object.get("kind") {
        if kind.as_str() != Some("user") {
            return Err(TargetParseError::InvalidSelector(input.to_owned()));
        }
    }

    let has_id = match object.get("id") {
        Some(value) => {
            let Some(id) = value.as_i64() else {
                return Err(TargetParseError::InvalidSelector(input.to_owned()));
            };
            if id == 0 {
                return Err(TargetParseError::InvalidSelector(input.to_owned()));
            }
            true
        }
        None => false,
    };

    let has_username = match object.get("username") {
        Some(value) => {
            let Some(username) = value.as_str() else {
                return Err(TargetParseError::InvalidSelector(input.to_owned()));
            };
            if username.is_empty()
                || !username
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                return Err(TargetParseError::InvalidSelector(input.to_owned()));
            }
            true
        }
        None => false,
    };

    if !has_id && !has_username {
        return Err(TargetParseError::InvalidSelector(input.to_owned()));
    }

    Ok(())
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
        ParsedTargetSelector, ResolvedTarget, TargetParseError, TargetSource,
        parse_target_selector, resolve_target,
    };
    use crate::event::{
        EventContext, ExecutionMode, MessageContentKind, MessageContext, ReplyContext,
        SystemContext, UpdateType,
    };
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn valid_username_roundtrips() {
        let usernames = vec!["user", "User123", "user_123", "a", "test_user_123"];
        for username in usernames {
            let input = format!("@{}", username);
            let parsed =
                parse_target_selector(&input).unwrap_or_else(|_| panic!("{} should parse", input));
            match parsed {
                ParsedTargetSelector::Username { username: u } => {
                    assert_eq!(u, username);
                }
                other => panic!("expected Username, got {:?}", other),
            }
        }
    }

    #[test]
    fn valid_user_id_roundtrips() {
        let ids = vec![1, 42, 1000, 999999, -100, -42];
        for id in ids {
            let input = id.to_string();
            let parsed =
                parse_target_selector(&input).unwrap_or_else(|_| panic!("{} should parse", input));
            match parsed {
                ParsedTargetSelector::UserId { user_id } => {
                    assert_eq!(user_id, id);
                }
                other => panic!("expected UserId, got {:?}", other),
            }
        }
    }

    #[test]
    fn user_id_parses_negative() {
        let ids = vec![-100, -42, -1];
        for id in ids {
            let input = id.to_string();
            let parsed =
                parse_target_selector(&input).unwrap_or_else(|_| panic!("{} should parse", input));
            let is_user_id = matches!(parsed, ParsedTargetSelector::UserId { .. });
            assert!(is_user_id);
        }
    }

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
            content_kind: Some(MessageContentKind::Text),
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
        assert_eq!(
            parse_target_selector("@spam_user").expect("username parses"),
            ParsedTargetSelector::Username {
                username: "spam_user".to_owned(),
            }
        );
        assert_eq!(
            parse_target_selector("12345").expect("id parses"),
            ParsedTargetSelector::UserId { user_id: 12345 }
        );
        assert_eq!(
            parse_target_selector("msg:42").expect("anchor parses"),
            ParsedTargetSelector::MessageAnchor { message_id: 42 }
        );
        assert_eq!(
            parse_target_selector(r#"{"kind":"user","id":42}"#).expect("json parses"),
            ParsedTargetSelector::JsonSelector {
                raw: json!({"kind": "user", "id": 42}),
            }
        );
    }

    #[test]
    fn rejects_invalid_target_selector() {
        let err = parse_target_selector("@bad-name").expect_err("invalid username");
        assert_eq!(
            err,
            TargetParseError::InvalidUsername("@bad-name".to_owned())
        );
    }

    #[test]
    fn accepts_only_bounded_json_selector_shapes() {
        assert_eq!(
            parse_target_selector(r#"{"id":42}"#).expect("id-only json selector parses"),
            ParsedTargetSelector::JsonSelector {
                raw: json!({"id": 42}),
            }
        );
        assert_eq!(
            parse_target_selector(r#"{"username":"spam_user"}"#)
                .expect("username-only json selector parses"),
            ParsedTargetSelector::JsonSelector {
                raw: json!({"username": "spam_user"}),
            }
        );
        assert_eq!(
            parse_target_selector(r#"{"kind":"user","id":42,"username":"spam_user"}"#)
                .expect("full json selector parses"),
            ParsedTargetSelector::JsonSelector {
                raw: json!({"kind": "user", "id": 42, "username": "spam_user"}),
            }
        );
    }

    #[test]
    fn rejects_loose_or_invalid_json_selectors() {
        let invalid_inputs = vec![
            r#"{}"#,
            r#"[]"#,
            r#"{"kind":"chat","id":42}"#,
            r#"{"kind":"user"}"#,
            r#"{"id":0}"#,
            r#"{"id":"42"}"#,
            r#"{"username":""}"#,
            r#"{"username":"bad-name"}"#,
            r#"{"kind":"user","id":42,"extra":true}"#,
        ];

        for input in invalid_inputs {
            let err = parse_target_selector(input).expect_err("selector should be rejected");
            assert_eq!(err, TargetParseError::InvalidSelector(input.to_owned()));
        }
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
