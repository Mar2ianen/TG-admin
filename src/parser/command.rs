use crate::event::EventContext;
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
    pub command: ParsedCommand,
    pub pipe: Option<Box<ParsedCommandLine>>,
    pub execution_mode: crate::event::ExecutionMode,
    pub synthetic: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ParsedCommand {
    pub name: CommandName,
    pub raw: String,
    pub raw_arguments: Vec<String>,
    pub flags: IndexMap<String, Option<String>>,
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
}

fn parse_command_line(
    input: &str,
    event: &EventContext,
) -> Result<ParsedCommandLine, CommandParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(CommandParseError::EmptyInput);
    }

    let segments: Vec<&str> = input.split('|').map(str::trim).collect();
    if segments.len() > 2 {
        return Err(CommandParseError::NestedPipeNotSupported);
    }

    let head = parse_single_command(segments[0])?;
    let pipe = segments
        .get(1)
        .map(|tail| parse_single_command(tail).map(ParsedCommandLine::without_pipe))
        .transpose()?
        .map(Box::new);

    Ok(ParsedCommandLine {
        command: head,
        pipe,
        execution_mode: event.execution_mode,
        synthetic: event.is_synthetic(),
    })
}

fn parse_single_command(input: &str) -> Result<ParsedCommand, CommandParseError> {
    let mut parts = input.split_whitespace();
    let raw_name = parts.next().ok_or(CommandParseError::EmptyInput)?;
    let name = raw_name
        .strip_prefix('/')
        .ok_or(CommandParseError::MissingCommandPrefix)
        .and_then(CommandName::parse)?;

    let raw_arguments = parts.map(ToOwned::to_owned).collect();

    Ok(ParsedCommand {
        name,
        raw: input.to_owned(),
        raw_arguments,
        flags: IndexMap::new(),
    })
}

impl ParsedCommandLine {
    fn without_pipe(command: ParsedCommand) -> Self {
        Self {
            command,
            pipe: None,
            execution_mode: crate::event::ExecutionMode::Manual,
            synthetic: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandName, CommandParseError, CommandParser};
    use crate::event::EventContext;

    #[test]
    fn parser_preserves_execution_mode_from_event_context() {
        let parser = CommandParser::new();
        let event = EventContext::system_event();

        let parsed = parser
            .parse("/warn @user 2.8", &event)
            .expect("parse succeeds");

        assert_eq!(parsed.command.name, CommandName::Warn);
        assert_eq!(parsed.execution_mode, event.execution_mode);
        assert!(parsed.synthetic);
        assert_eq!(parsed.command.raw_arguments, vec!["@user", "2.8"]);
    }

    #[test]
    fn parser_supports_single_pipe_but_rejects_nested_pipe() {
        let parser = CommandParser::new();
        let event = EventContext::system_event();

        let parsed = parser
            .parse("/mute @user 7d | /msg done", &event)
            .expect("single pipe is allowed");
        assert!(parsed.pipe.is_some());

        let err = parser
            .parse("/mute @user 7d | /msg done | /undo", &event)
            .expect_err("nested pipe must fail");
        assert_eq!(err, CommandParseError::NestedPipeNotSupported);
    }
}
