use crate::event::{CommandSource, EventContext};
use crate::parser::command::{CommandParseError, CommandParser, ParsedCommandLine};
use crate::parser::reason::{ExpandedCommandLine, ReasonAliasRegistry};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EventCommandDispatcher {
    parser: CommandParser,
    aliases: ReasonAliasRegistry,
}

impl Default for EventCommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl EventCommandDispatcher {
    pub fn new() -> Self {
        Self {
            parser: CommandParser::new(),
            aliases: ReasonAliasRegistry::new(),
        }
    }

    pub fn with_aliases(aliases: ReasonAliasRegistry) -> Self {
        Self {
            parser: CommandParser::new(),
            aliases,
        }
    }

    pub fn dispatch(&self, event: &EventContext) -> CommandDispatchResult {
        let Some(source) = event.command_source() else {
            return CommandDispatchResult::Skipped(CommandDispatchSkip {
                reason: CommandDispatchSkipReason::NoCommandSource,
                source_kind: None,
                raw_source: None,
            });
        };

        let (source_kind, raw_source) = match source {
            CommandSource::MessageText(text) => (CommandSourceKind::MessageText, text.trim()),
            CommandSource::CallbackData(data) => (CommandSourceKind::CallbackData, data.trim()),
        };

        if raw_source.is_empty() {
            return CommandDispatchResult::Skipped(CommandDispatchSkip {
                reason: CommandDispatchSkipReason::EmptySource,
                source_kind: Some(source_kind),
                raw_source: Some(raw_source.to_owned()),
            });
        }

        if !raw_source.starts_with('/') {
            return CommandDispatchResult::Skipped(CommandDispatchSkip {
                reason: CommandDispatchSkipReason::NotACommand,
                source_kind: Some(source_kind),
                raw_source: Some(raw_source.to_owned()),
            });
        }

        match self.parser.parse(raw_source, event) {
            Ok(parsed) => CommandDispatchResult::Parsed(Box::new(DispatchedCommand {
                source_kind,
                raw_source: raw_source.to_owned(),
                expanded: self.aliases.expand_command_line(&parsed),
                parsed,
            })),
            Err(error) => CommandDispatchResult::ParseError(CommandDispatchParseError {
                source_kind,
                raw_source: raw_source.to_owned(),
                error,
            }),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum CommandDispatchResult {
    Parsed(Box<DispatchedCommand>),
    Skipped(CommandDispatchSkip),
    ParseError(CommandDispatchParseError),
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DispatchedCommand {
    pub source_kind: CommandSourceKind,
    pub raw_source: String,
    pub parsed: ParsedCommandLine,
    pub expanded: ExpandedCommandLine,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandDispatchSkip {
    pub reason: CommandDispatchSkipReason,
    pub source_kind: Option<CommandSourceKind>,
    pub raw_source: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandDispatchParseError {
    pub source_kind: CommandSourceKind,
    pub raw_source: String,
    pub error: CommandParseError,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum CommandDispatchSkipReason {
    NoCommandSource,
    EmptySource,
    NotACommand,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum CommandSourceKind {
    MessageText,
    CallbackData,
}

#[cfg(test)]
mod tests {
    use super::{
        CommandDispatchResult, CommandDispatchSkip, CommandDispatchSkipReason, CommandSourceKind,
        EventCommandDispatcher,
    };
    use crate::event::{
        CallbackContext, ChatContext, EventNormalizer, ManualInvocationInput, MessageContext,
        SenderContext, TelegramUpdateInput, UnitContext,
    };
    use crate::parser::reason::{ReasonAliasDefinition, ReasonAliasRegistry};
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 21, 10, 0, 0)
            .single()
            .expect("valid timestamp")
    }

    fn chat() -> ChatContext {
        ChatContext {
            id: -100123,
            chat_type: "supergroup".to_owned(),
            title: Some("Moderation HQ".to_owned()),
            username: Some("mod_hq".to_owned()),
            thread_id: Some(11),
        }
    }

    fn sender() -> SenderContext {
        SenderContext {
            id: 42,
            username: Some("admin".to_owned()),
            display_name: Some("Admin".to_owned()),
            is_bot: false,
            is_admin: true,
            role: Some("owner".to_owned()),
        }
    }

    #[test]
    fn dispatches_normalized_manual_event_with_alias_expansion_snapshot() {
        let normalizer = EventNormalizer::new();
        let mut input = ManualInvocationInput::new(
            UnitContext::new("moderation.warn").with_trigger("manual"),
            "/warn @spam spam -s",
        );
        input.event_id = Some("evt_manual_dispatch_snapshot".to_owned());
        input.received_at = ts();
        input.chat = Some(chat());
        input.sender = Some(sender());

        let event = normalizer
            .normalize_manual(input)
            .expect("manual normalization must succeed");

        let mut aliases = ReasonAliasRegistry::new();
        aliases.insert(
            "spam",
            ReasonAliasDefinition::new("spam or scam promotion")
                .with_rule_code("2.8")
                .with_title("Spam"),
        );
        let dispatcher = EventCommandDispatcher::with_aliases(aliases);

        let result = dispatcher.dispatch(&event);
        let snapshot = serde_json::to_string_pretty(&result).expect("snapshot serializes");

        assert_eq!(
            snapshot,
            r#"{
  "Parsed": {
    "source_kind": "MessageText",
    "raw_source": "/warn @spam spam -s",
    "parsed": {
      "command": {
        "Warn": {
          "name": "Warn",
          "target": {
            "selector": {
              "Username": {
                "username": "spam"
              }
            },
            "source": "ExplicitPositional"
          },
          "reason": {
            "Alias": "spam"
          },
          "flags": {
            "silent": true,
            "public_notice": false,
            "delete_history": false,
            "dry_run": false,
            "force": false
          }
        }
      },
      "pipe": null,
      "execution_mode": "manual",
      "synthetic": true
    },
    "expanded": {
      "command": {
        "Warn": {
          "command": {
            "name": "Warn",
            "target": {
              "selector": {
                "Username": {
                  "username": "spam"
                }
              },
              "source": "ExplicitPositional"
            },
            "reason": {
              "Alias": "spam"
            },
            "flags": {
              "silent": true,
              "public_notice": false,
              "delete_history": false,
              "dry_run": false,
              "force": false
            }
          },
          "expanded_reason": {
            "Alias": {
              "alias": "spam",
              "definition": {
                "canonical": "spam or scam promotion",
                "rule_code": "2.8",
                "title": "Spam"
              }
            }
          }
        }
      },
      "pipe": null,
      "execution_mode": "manual",
      "synthetic": true
    }
  }
}"#
        );
    }

    #[test]
    fn skips_non_command_callback_payload_with_typed_reason() {
        let normalizer = EventNormalizer::new();
        let input = TelegramUpdateInput {
            event_id: Some("evt_callback_skip".to_owned()),
            update_id: 55,
            update_type: crate::event::UpdateType::CallbackQuery,
            received_at: ts(),
            execution_mode: crate::event::ExecutionMode::Realtime,
            chat: chat(),
            sender: Some(sender()),
            message: Some(MessageContext {
                id: 777,
                date: ts(),
                text: Some("button".to_owned()),
                entities: Vec::new(),
                has_media: false,
                file_ids: Vec::new(),
                reply_to_message_id: None,
                media_group_id: None,
            }),
            reply: None,
            callback: Some(CallbackContext {
                query_id: "cbq-1".to_owned(),
                data: Some("noop:button".to_owned()),
                message_id: Some(777),
                origin_chat_id: Some(-100123),
                from_user_id: 42,
            }),
            locale: None,
            trace_id: None,
            build: None,
        };
        let event = normalizer
            .normalize_telegram(input)
            .expect("telegram normalization must succeed");
        let dispatcher = EventCommandDispatcher::new();

        assert_eq!(
            dispatcher.dispatch(&event),
            CommandDispatchResult::Skipped(CommandDispatchSkip {
                reason: CommandDispatchSkipReason::NotACommand,
                source_kind: Some(CommandSourceKind::CallbackData),
                raw_source: Some("noop:button".to_owned()),
            })
        );
    }

    #[test]
    fn snapshot_like_parse_error_shape_stays_stable() {
        let normalizer = EventNormalizer::new();
        let mut input = ManualInvocationInput::new(
            UnitContext::new("moderation.mute").with_trigger("manual"),
            "/mute @spam 30",
        );
        input.event_id = Some("evt_manual_parse_error".to_owned());
        input.received_at = ts();
        input.chat = Some(chat());
        input.sender = Some(sender());

        let event = normalizer
            .normalize_manual(input)
            .expect("manual normalization must succeed");
        let dispatcher = EventCommandDispatcher::new();

        let result = dispatcher.dispatch(&event);
        assert_eq!(
            serde_json::to_value(&result).expect("result serializes"),
            json!({
                "ParseError": {
                    "source_kind": "MessageText",
                    "raw_source": "/mute @spam 30",
                    "error": {
                        "InvalidDuration": {
                            "value": "30",
                            "source": "MissingUnit"
                        }
                    }
                }
            })
        );
    }
}
