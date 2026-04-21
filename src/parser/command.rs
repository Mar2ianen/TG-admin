use crate::event::{EventContext, ExecutionMode};
use crate::parser::duration::{DurationParseError, DurationParser, ParsedDuration};
use crate::parser::target::{
    ParsedTargetSelector, ResolvedTarget, TargetParseError, parse_target_selector, resolve_target,
};
use indexmap::IndexMap;
use thiserror::Error;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CommandParser;

impl CommandParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(
        &self,
        input: &str,
        event: &EventContext,
    ) -> Result<ParsedCommandLine, CommandParseError> {
        parse_command_line(input, event)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ParsedCommandLine {
    pub command: CommandAst,
    pub pipe: Option<Box<ParsedCommandLine>>,
    pub execution_mode: ExecutionMode,
    pub synthetic: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CommandAst {
    Warn(ModerationCommand),
    Mute(MuteCommand),
    Ban(ModerationCommand),
    Del(DeleteCommand),
    Undo(UndoCommand),
    Msg(MessageCommand),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CommandName {
    Warn,
    Mute,
    Ban,
    Del,
    Undo,
    Msg,
}

impl CommandName {
    fn parse(input: &str) -> Result<Self, CommandParseError> {
        match input {
            "warn" => Ok(Self::Warn),
            "mute" => Ok(Self::Mute),
            "ban" => Ok(Self::Ban),
            "del" => Ok(Self::Del),
            "undo" => Ok(Self::Undo),
            "msg" => Ok(Self::Msg),
            other => Err(CommandParseError::UnknownCommand(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModerationCommand {
    pub name: CommandName,
    pub target: ResolvedTarget,
    pub reason: Option<ReasonExpr>,
    pub flags: ModerationFlags,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MuteCommand {
    pub target: ResolvedTarget,
    pub duration: ParsedDuration,
    pub reason: Option<ReasonExpr>,
    pub flags: ModerationFlags,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DeleteCommand {
    pub target: ResolvedTarget,
    pub window: DeleteWindow,
    pub user_filter: Option<ParsedTargetSelector>,
    pub since: Option<ParsedDuration>,
    pub flags: DeleteFlags,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub struct DeleteWindow {
    pub up: u16,
    pub down: u16,
}

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct ModerationFlags {
    pub silent: bool,
    pub public_notice: bool,
    pub delete_history: bool,
    pub dry_run: bool,
    pub force: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct DeleteFlags {
    pub dry_run: bool,
    pub force: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct UndoCommand {
    pub dry_run: bool,
    pub force: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MessageCommand {
    pub text: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ReasonExpr {
    RuleCode(String),
    Alias(String),
    Quoted(String),
    FreeText(String),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum CommandParseError {
    #[error("command input is empty")]
    EmptyInput,
    #[error("command must start with '/'")]
    MissingCommandPrefix,
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("nested pipes are not supported in MVP")]
    NestedPipeNotSupported,
    #[error("pipe is not allowed for `/{0}`")]
    PipeNotAllowed(String),
    #[error("piped head command must include explicit duration")]
    PipeRequiresExplicitDuration,
    #[error("missing target for `/{0}`")]
    MissingTarget(String),
    #[error("missing duration for `/{0}`")]
    MissingDuration(String),
    #[error("invalid duration `{value}`: {source}")]
    InvalidDuration {
        value: String,
        source: DurationParseError,
    },
    #[error("invalid target `{value}`: {source}")]
    InvalidTarget {
        value: String,
        source: TargetParseError,
    },
    #[error("unknown flag `-{0}`")]
    UnknownFlag(String),
    #[error("missing value for flag `-{0}`")]
    MissingFlagValue(String),
    #[error("invalid numeric flag value for `-{flag}`: `{value}`")]
    InvalidNumericFlagValue { flag: String, value: String },
    #[error("conflicting flags `-{left}` and `-{right}`")]
    ConflictingFlags { left: String, right: String },
    #[error("unexpected arguments for `/{0}`")]
    UnexpectedArguments(String),
    #[error("quoted string is not terminated")]
    UnterminatedQuote,
    #[error("json selector is not balanced")]
    UnbalancedJsonSelector,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct Token {
    text: String,
    quoted: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct CommandTokens {
    name: CommandName,
    arguments: Vec<Token>,
    flags: IndexMap<String, Token>,
}

fn parse_command_line(
    input: &str,
    event: &EventContext,
) -> Result<ParsedCommandLine, CommandParseError> {
    let segments = split_pipe_segments(input)?;
    let head_tokens = parse_tokens(segments[0])?;
    let head_command = parse_command_tokens(head_tokens, event)?;

    let pipe = segments
        .get(1)
        .map(|segment| parse_tokens(segment).and_then(|tokens| parse_command_tokens(tokens, event)))
        .transpose()?
        .map(|command| ParsedCommandLine {
            command,
            pipe: None,
            execution_mode: event.execution_mode,
            synthetic: event.is_synthetic(),
        })
        .map(Box::new);

    if pipe.is_some() {
        validate_pipe(&head_command)?;
    }

    Ok(ParsedCommandLine {
        command: head_command,
        pipe,
        execution_mode: event.execution_mode,
        synthetic: event.is_synthetic(),
    })
}

fn validate_pipe(command: &CommandAst) -> Result<(), CommandParseError> {
    match command {
        CommandAst::Mute(parsed) => {
            if parsed.duration.value == 0 {
                return Err(CommandParseError::PipeRequiresExplicitDuration);
            }
            Ok(())
        }
        CommandAst::Warn(parsed) => Err(CommandParseError::PipeNotAllowed(
            command_name_from_ast(parsed.name).to_owned(),
        )),
        CommandAst::Ban(_) => Err(CommandParseError::PipeNotAllowed("ban".to_owned())),
        CommandAst::Del(_) => Err(CommandParseError::PipeNotAllowed("del".to_owned())),
        CommandAst::Undo(_) => Err(CommandParseError::PipeNotAllowed("undo".to_owned())),
        CommandAst::Msg(_) => Err(CommandParseError::PipeNotAllowed("msg".to_owned())),
    }
}

fn command_name_from_ast(name: CommandName) -> &'static str {
    match name {
        CommandName::Warn => "warn",
        CommandName::Mute => "mute",
        CommandName::Ban => "ban",
        CommandName::Del => "del",
        CommandName::Undo => "undo",
        CommandName::Msg => "msg",
    }
}

fn parse_command_tokens(
    tokens: CommandTokens,
    event: &EventContext,
) -> Result<CommandAst, CommandParseError> {
    match tokens.name {
        CommandName::Warn => parse_moderation_command(tokens, event, CommandName::Warn),
        CommandName::Mute => parse_mute_command(tokens, event),
        CommandName::Ban => parse_moderation_command(tokens, event, CommandName::Ban),
        CommandName::Del => parse_delete_command(tokens, event),
        CommandName::Undo => parse_undo_command(tokens),
        CommandName::Msg => parse_msg_command(tokens),
    }
}

fn parse_moderation_command(
    mut tokens: CommandTokens,
    event: &EventContext,
    name: CommandName,
) -> Result<CommandAst, CommandParseError> {
    let selector_flag = take_selector_flag(&mut tokens)?;
    let flags = parse_moderation_flags(&mut tokens)?;
    let positional_target = take_positional_target(&mut tokens)?;
    let target = resolve_target(positional_target, selector_flag, event, |_| None)
        .ok_or_else(|| CommandParseError::MissingTarget(command_name_from_ast(name).to_owned()))?;
    let reason = parse_reason_tokens(&tokens.arguments);

    let command = ModerationCommand {
        name,
        target,
        reason,
        flags,
    };

    Ok(match name {
        CommandName::Warn => CommandAst::Warn(command),
        CommandName::Ban => CommandAst::Ban(command),
        _ => unreachable!("moderation parser is only used for warn/ban"),
    })
}

fn parse_mute_command(
    mut tokens: CommandTokens,
    event: &EventContext,
) -> Result<CommandAst, CommandParseError> {
    let selector_flag = take_selector_flag(&mut tokens)?;
    let flags = parse_moderation_flags(&mut tokens)?;
    let positional_target = take_positional_target(&mut tokens)?;
    let target = resolve_target(positional_target, selector_flag, event, |_| None)
        .ok_or_else(|| CommandParseError::MissingTarget("mute".to_owned()))?;

    let duration_token = tokens
        .arguments
        .first()
        .cloned()
        .ok_or_else(|| CommandParseError::MissingDuration("mute".to_owned()))?;
    let duration = DurationParser::new()
        .parse(&duration_token.text)
        .map_err(|source| CommandParseError::InvalidDuration {
            value: duration_token.text.clone(),
            source,
        })?;
    tokens.arguments.remove(0);
    let reason = parse_reason_tokens(&tokens.arguments);

    Ok(CommandAst::Mute(MuteCommand {
        target,
        duration,
        reason,
        flags,
    }))
}

fn parse_delete_command(
    mut tokens: CommandTokens,
    event: &EventContext,
) -> Result<CommandAst, CommandParseError> {
    let user_filter = take_selector_flag(&mut tokens)?;
    let flags = parse_delete_flags(&mut tokens);
    let since = take_duration_flag(&mut tokens, "since")?;
    let window = DeleteWindow {
        up: take_numeric_flag(&mut tokens, "up")?.unwrap_or_default(),
        down: take_numeric_flag(&mut tokens, "dn")?.unwrap_or_default(),
    };
    reject_remaining_flags(&tokens)?;
    let positional_target = take_positional_target(&mut tokens)?;
    let target = resolve_target(positional_target, user_filter.clone(), event, |ctx| {
        ctx.message
            .as_ref()
            .map(|message| ParsedTargetSelector::MessageAnchor {
                message_id: message.id,
            })
    })
    .ok_or_else(|| CommandParseError::MissingTarget("del".to_owned()))?;

    if !tokens.arguments.is_empty() {
        return Err(CommandParseError::UnexpectedArguments("del".to_owned()));
    }

    Ok(CommandAst::Del(DeleteCommand {
        target,
        window,
        user_filter,
        since,
        flags,
    }))
}

fn parse_undo_command(mut tokens: CommandTokens) -> Result<CommandAst, CommandParseError> {
    let dry_run = take_bool_flag(&mut tokens, "dry");
    let force = take_bool_flag(&mut tokens, "force");
    reject_remaining_flags(&tokens)?;

    if !tokens.arguments.is_empty() {
        return Err(CommandParseError::UnexpectedArguments("undo".to_owned()));
    }

    Ok(CommandAst::Undo(UndoCommand { dry_run, force }))
}

fn parse_msg_command(tokens: CommandTokens) -> Result<CommandAst, CommandParseError> {
    reject_remaining_flags(&tokens)?;

    if tokens.arguments.is_empty() {
        return Err(CommandParseError::UnexpectedArguments("msg".to_owned()));
    }

    let text = tokens
        .arguments
        .iter()
        .map(|token| token.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    Ok(CommandAst::Msg(MessageCommand { text }))
}

fn parse_moderation_flags(
    tokens: &mut CommandTokens,
) -> Result<ModerationFlags, CommandParseError> {
    let silent = take_bool_flag(tokens, "s");
    let public_notice = take_bool_flag(tokens, "pub");
    if silent && public_notice {
        return Err(CommandParseError::ConflictingFlags {
            left: "s".to_owned(),
            right: "pub".to_owned(),
        });
    }

    let delete_history = take_bool_flag(tokens, "del");
    let dry_run = take_bool_flag(tokens, "dry");
    let force = take_bool_flag(tokens, "force");
    reject_remaining_flags(tokens)?;

    Ok(ModerationFlags {
        silent,
        public_notice,
        delete_history,
        dry_run,
        force,
    })
}

fn parse_delete_flags(tokens: &mut CommandTokens) -> DeleteFlags {
    let dry_run = take_bool_flag(tokens, "dry");
    let force = take_bool_flag(tokens, "force");

    DeleteFlags { dry_run, force }
}

fn take_selector_flag(
    tokens: &mut CommandTokens,
) -> Result<Option<ParsedTargetSelector>, CommandParseError> {
    take_token_flag(tokens, "user")?
        .map(|token| {
            parse_target_selector(&token.text).map_err(|source| CommandParseError::InvalidTarget {
                value: token.text,
                source,
            })
        })
        .transpose()
}

fn take_duration_flag(
    tokens: &mut CommandTokens,
    name: &'static str,
) -> Result<Option<ParsedDuration>, CommandParseError> {
    take_token_flag(tokens, name)?
        .map(|token| {
            DurationParser::new().parse(&token.text).map_err(|source| {
                CommandParseError::InvalidDuration {
                    value: token.text,
                    source,
                }
            })
        })
        .transpose()
}

fn take_numeric_flag(
    tokens: &mut CommandTokens,
    name: &'static str,
) -> Result<Option<u16>, CommandParseError> {
    take_token_flag(tokens, name)?
        .map(|token| {
            token
                .text
                .parse::<u16>()
                .map_err(|_| CommandParseError::InvalidNumericFlagValue {
                    flag: name.to_owned(),
                    value: token.text,
                })
        })
        .transpose()
}

fn take_bool_flag(tokens: &mut CommandTokens, name: &'static str) -> bool {
    tokens.flags.swap_remove(name).is_some()
}

fn take_token_flag(
    tokens: &mut CommandTokens,
    name: &'static str,
) -> Result<Option<Token>, CommandParseError> {
    if let Some(value) = tokens.flags.swap_remove(name) {
        return Ok(Some(value));
    }

    Ok(None)
}

fn reject_remaining_flags(tokens: &CommandTokens) -> Result<(), CommandParseError> {
    if let Some(name) = tokens.flags.keys().next() {
        return Err(CommandParseError::UnknownFlag(name.clone()));
    }

    Ok(())
}

fn take_positional_target(
    tokens: &mut CommandTokens,
) -> Result<Option<ParsedTargetSelector>, CommandParseError> {
    let Some(first) = tokens.arguments.first().cloned() else {
        return Ok(None);
    };

    match parse_target_selector(&first.text) {
        Ok(target) => {
            tokens.arguments.remove(0);
            Ok(Some(target))
        }
        Err(TargetParseError::InvalidSelector(_)) => Ok(None),
        Err(source) => Err(CommandParseError::InvalidTarget {
            value: first.text,
            source,
        }),
    }
}

fn parse_reason_tokens(tokens: &[Token]) -> Option<ReasonExpr> {
    if tokens.is_empty() {
        return None;
    }

    if tokens.len() == 1 {
        let token = &tokens[0];
        if token.quoted {
            return Some(ReasonExpr::Quoted(token.text.clone()));
        }
        if is_rule_code(&token.text) {
            return Some(ReasonExpr::RuleCode(token.text.clone()));
        }
        if is_alias(&token.text) {
            return Some(ReasonExpr::Alias(token.text.clone()));
        }
    }

    Some(ReasonExpr::FreeText(
        tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    ))
}

fn is_rule_code(input: &str) -> bool {
    let mut segments = input.split('.');
    let Some(first) = segments.next() else {
        return false;
    };

    !first.is_empty()
        && first.chars().all(|ch| ch.is_ascii_digit())
        && segments
            .all(|segment| !segment.is_empty() && segment.chars().all(|ch| ch.is_ascii_digit()))
}

fn is_alias(input: &str) -> bool {
    input
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn parse_tokens(segment: &str) -> Result<CommandTokens, CommandParseError> {
    let raw_tokens = lex_segment(segment)?;
    let mut iter = raw_tokens.into_iter();
    let command = iter.next().ok_or(CommandParseError::EmptyInput)?;
    let command_name = command
        .text
        .strip_prefix('/')
        .ok_or(CommandParseError::MissingCommandPrefix)
        .and_then(CommandName::parse)?;

    let mut arguments = Vec::new();
    let mut flags = IndexMap::new();
    while let Some(token) = iter.next() {
        if !token.quoted && token.text.starts_with('-') && token.text.len() > 1 {
            let name = token.text.trim_start_matches('-').to_owned();
            let needs_value = matches!(name.as_str(), "user" | "since" | "up" | "dn");
            if needs_value {
                let value = iter
                    .next()
                    .ok_or_else(|| CommandParseError::MissingFlagValue(name.clone()))?;
                flags.insert(name, value);
            } else {
                flags.insert(
                    name,
                    Token {
                        text: "true".to_owned(),
                        quoted: false,
                    },
                );
            }
        } else {
            arguments.push(token);
        }
    }

    Ok(CommandTokens {
        name: command_name,
        arguments,
        flags,
    })
}

fn split_pipe_segments(input: &str) -> Result<Vec<&str>, CommandParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(CommandParseError::EmptyInput);
    }

    let mut in_quotes = false;
    let mut escape = false;
    let mut brace_depth = 0usize;
    let mut start = 0usize;
    let mut segments = Vec::new();

    for (idx, ch) in input.char_indices() {
        if escape {
            escape = false;
            continue;
        }

        match ch {
            '\\' if in_quotes => escape = true,
            '"' if brace_depth == 0 => in_quotes = !in_quotes,
            '{' if !in_quotes => brace_depth = brace_depth.saturating_add(1),
            '}' if !in_quotes && brace_depth > 0 => brace_depth -= 1,
            '|' if !in_quotes && brace_depth == 0 => {
                segments.push(input[start..idx].trim());
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }

    if in_quotes {
        return Err(CommandParseError::UnterminatedQuote);
    }
    if brace_depth != 0 {
        return Err(CommandParseError::UnbalancedJsonSelector);
    }

    segments.push(input[start..].trim());
    if segments.len() > 2 {
        return Err(CommandParseError::NestedPipeNotSupported);
    }

    Ok(segments)
}

fn lex_segment(segment: &str) -> Result<Vec<Token>, CommandParseError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut token_quoted = false;
    let mut escape = false;
    let mut brace_depth = 0usize;

    for ch in segment.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }

        match ch {
            '\\' if in_quotes => escape = true,
            '"' if brace_depth == 0 => {
                in_quotes = !in_quotes;
                token_quoted = true;
            }
            '{' if !in_quotes => {
                brace_depth = brace_depth.saturating_add(1);
                current.push(ch);
            }
            '}' if !in_quotes => {
                brace_depth = brace_depth.saturating_sub(1);
                current.push(ch);
            }
            ch if ch.is_whitespace() && !in_quotes && brace_depth == 0 => {
                if !current.is_empty() || token_quoted {
                    tokens.push(Token {
                        text: std::mem::take(&mut current),
                        quoted: token_quoted,
                    });
                    token_quoted = false;
                }
            }
            _ => current.push(ch),
        }
    }

    if in_quotes {
        return Err(CommandParseError::UnterminatedQuote);
    }
    if brace_depth != 0 {
        return Err(CommandParseError::UnbalancedJsonSelector);
    }
    if !current.is_empty() || token_quoted {
        tokens.push(Token {
            text: current,
            quoted: token_quoted,
        });
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::{
        CommandAst, CommandParseError, CommandParser, DeleteWindow, ReasonExpr,
        command_name_from_ast,
    };
    use crate::event::{
        EventContext, ExecutionMode, MessageContext, ReplyContext, SystemContext, UpdateType,
    };
    use crate::parser::duration::DurationUnit;
    use crate::parser::target::{ParsedTargetSelector, TargetSource};
    use chrono::Utc;
    use serde_json::json;

    fn realtime_event() -> EventContext {
        let mut event = EventContext::new(
            "evt_command",
            UpdateType::Message,
            ExecutionMode::Realtime,
            SystemContext::realtime(),
        );
        event.message = Some(MessageContext {
            id: 101,
            date: Utc::now(),
            text: Some("/cmd".to_owned()),
            entities: Vec::new(),
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: Some(77),
            media_group_id: None,
        });
        event.reply = Some(ReplyContext {
            message_id: 77,
            sender_user_id: Some(9090),
            sender_username: Some("reply_target".to_owned()),
            text: Some("reply".to_owned()),
            has_media: false,
        });
        event
    }

    fn event_without_reply() -> EventContext {
        let mut event = EventContext::new(
            "evt_command",
            UpdateType::Message,
            ExecutionMode::Realtime,
            SystemContext::realtime(),
        );
        event.message = Some(MessageContext {
            id: 101,
            date: Utc::now(),
            text: Some("/cmd".to_owned()),
            entities: Vec::new(),
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: None,
            media_group_id: None,
        });
        event
    }

    #[test]
    fn parses_warn_with_rule_code_reason() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let parsed = parser
            .parse("/warn @spam_user 2.8", &event)
            .expect("warn parses");

        match parsed.command {
            CommandAst::Warn(warn) => {
                assert_eq!(command_name_from_ast(warn.name), "warn");
                assert_eq!(
                    warn.target.selector,
                    ParsedTargetSelector::Username {
                        username: "spam_user".to_owned(),
                    }
                );
                assert_eq!(warn.target.source, TargetSource::ExplicitPositional);
                assert_eq!(warn.reason, Some(ReasonExpr::RuleCode("2.8".to_owned())));
            }
            other => panic!("unexpected AST: {other:?}"),
        }
    }

    #[test]
    fn parses_mute_with_duration_flags_and_pipe() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let parsed = parser
            .parse(r#"/mute @spam_user 7d 2.8 -s | /msg "время вышло""#, &event)
            .expect("mute parses");

        match parsed.command {
            CommandAst::Mute(mute) => {
                assert_eq!(mute.duration.value, 7);
                assert_eq!(mute.duration.unit, DurationUnit::Days);
                assert!(mute.flags.silent);
                assert_eq!(mute.reason, Some(ReasonExpr::RuleCode("2.8".to_owned())));
            }
            other => panic!("unexpected AST: {other:?}"),
        }

        match parsed.pipe.expect("pipe exists").command {
            CommandAst::Msg(msg) => assert_eq!(msg.text, "время вышло"),
            other => panic!("unexpected pipe AST: {other:?}"),
        }
    }

    #[test]
    fn parses_delete_window_with_implicit_anchor() {
        let parser = CommandParser::new();
        let event = event_without_reply();

        let parsed = parser
            .parse("/del -up 2 -dn 2", &event)
            .expect("delete window parses");

        match parsed.command {
            CommandAst::Del(del) => {
                assert_eq!(del.window, DeleteWindow { up: 2, down: 2 });
                assert_eq!(del.target.source, TargetSource::ImplicitContext);
                assert_eq!(
                    del.target.selector,
                    ParsedTargetSelector::MessageAnchor { message_id: 101 }
                );
            }
            other => panic!("unexpected AST: {other:?}"),
        }
    }

    #[test]
    fn parses_undo_command() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let parsed = parser.parse("/undo", &event).expect("undo parses");
        assert!(matches!(parsed.command, CommandAst::Undo(_)));
    }

    #[test]
    fn parses_quoted_reason_with_spaces() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let parsed = parser
            .parse(r#"/warn @spam_user "очень плохое поведение""#, &event)
            .expect("quoted reason parses");

        match parsed.command {
            CommandAst::Warn(warn) => assert_eq!(
                warn.reason,
                Some(ReasonExpr::Quoted("очень плохое поведение".to_owned()))
            ),
            other => panic!("unexpected AST: {other:?}"),
        }
    }

    #[test]
    fn prefers_explicit_target_over_flag_and_reply_context() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let parsed = parser
            .parse("/warn @explicit -user 42 2.8", &event)
            .expect("target precedence parses");

        match parsed.command {
            CommandAst::Warn(warn) => {
                assert_eq!(warn.target.source, TargetSource::ExplicitPositional);
                assert_eq!(
                    warn.target.selector,
                    ParsedTargetSelector::Username {
                        username: "explicit".to_owned(),
                    }
                );
            }
            other => panic!("unexpected AST: {other:?}"),
        }
    }

    #[test]
    fn falls_back_to_flag_target_then_reply_target() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let parsed = parser
            .parse("/warn -user 42 2.8", &event)
            .expect("selector flag parses");
        match parsed.command {
            CommandAst::Warn(warn) => {
                assert_eq!(warn.target.source, TargetSource::SelectorFlag);
                assert_eq!(
                    warn.target.selector,
                    ParsedTargetSelector::UserId { user_id: 42 }
                );
            }
            other => panic!("unexpected AST: {other:?}"),
        }

        let reply_only = parser
            .parse("/warn 2.8", &event)
            .expect("reply fallback parses");
        match reply_only.command {
            CommandAst::Warn(warn) => assert_eq!(warn.target.source, TargetSource::ReplyContext),
            other => panic!("unexpected AST: {other:?}"),
        }
    }

    #[test]
    fn supports_json_target_selector_via_flag() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let parsed = parser
            .parse(r#"/del -user {"kind":"user","id":42}"#, &event)
            .expect("json selector parses");

        match parsed.command {
            CommandAst::Del(del) => {
                assert_eq!(
                    del.user_filter,
                    Some(ParsedTargetSelector::JsonSelector {
                        raw: json!({"kind": "user", "id": 42}),
                    })
                );
                assert_eq!(del.target.source, TargetSource::SelectorFlag);
            }
            other => panic!("unexpected AST: {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_command() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let err = parser
            .parse("/explode @user", &event)
            .expect_err("unknown command must fail");
        assert_eq!(err, CommandParseError::UnknownCommand("explode".to_owned()));
    }

    #[test]
    fn rejects_conflicting_flags() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let err = parser
            .parse("/mute @user 7d 2.8 -s -pub", &event)
            .expect_err("conflict must fail");
        assert_eq!(
            err,
            CommandParseError::ConflictingFlags {
                left: "s".to_owned(),
                right: "pub".to_owned(),
            }
        );
    }

    #[test]
    fn rejects_missing_target_when_context_cannot_supply_it() {
        let parser = CommandParser::new();
        let event = event_without_reply();

        let err = parser
            .parse("/warn 2.8", &event)
            .expect_err("target is required");
        assert_eq!(err, CommandParseError::MissingTarget("warn".to_owned()));
    }

    #[test]
    fn rejects_invalid_duration_with_precise_error() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let err = parser
            .parse("/mute @user 30 2.8", &event)
            .expect_err("duration unit is required");
        assert_eq!(
            err,
            CommandParseError::InvalidDuration {
                value: "30".to_owned(),
                source: crate::parser::duration::DurationParseError::MissingUnit,
            }
        );
    }

    #[test]
    fn rejects_nested_pipe() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let err = parser
            .parse("/mute @user 7d | /msg done | /undo", &event)
            .expect_err("nested pipe must fail");
        assert_eq!(err, CommandParseError::NestedPipeNotSupported);
    }

    #[test]
    fn rejects_pipe_for_command_without_scheduling_semantics() {
        let parser = CommandParser::new();
        let event = realtime_event();

        let err = parser
            .parse("/warn @user 2.8 | /msg done", &event)
            .expect_err("warn pipe is invalid");
        assert_eq!(err, CommandParseError::PipeNotAllowed("warn".to_owned()));
    }
}
