use crate::parser::command::{
    CommandAst, DeleteCommand, MessageCommand, ModerationCommand, MuteCommand, ParsedCommandLine,
    ReasonExpr, UndoCommand,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct ReasonAliasRegistry {
    aliases: IndexMap<String, ReasonAliasDefinition>,
}

impl ReasonAliasRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        alias: impl Into<String>,
        definition: ReasonAliasDefinition,
    ) -> Option<ReasonAliasDefinition> {
        self.aliases.insert(alias.into(), definition)
    }

    pub fn expand_reason(&self, reason: Option<&ReasonExpr>) -> Option<ExpandedReason> {
        let reason = reason?;
        Some(match reason {
            ReasonExpr::RuleCode(code) => ExpandedReason::RuleCode { code: code.clone() },
            ReasonExpr::Alias(alias) => match self.aliases.get(alias) {
                Some(definition) => ExpandedReason::Alias {
                    alias: alias.clone(),
                    definition: definition.clone(),
                },
                None => ExpandedReason::UnknownAlias {
                    alias: alias.clone(),
                },
            },
            ReasonExpr::Quoted(text) => ExpandedReason::Quoted { text: text.clone() },
            ReasonExpr::FreeText(text) => ExpandedReason::FreeText { text: text.clone() },
        })
    }

    pub fn expand_command_line(&self, parsed: &ParsedCommandLine) -> ExpandedCommandLine {
        ExpandedCommandLine {
            command: self.expand_command_ast(&parsed.command),
            pipe: parsed
                .pipe
                .as_deref()
                .map(|pipe| Box::new(self.expand_command_line(pipe))),
            execution_mode: parsed.execution_mode,
            synthetic: parsed.synthetic,
        }
    }

    fn expand_command_ast(&self, command: &CommandAst) -> ExpandedCommandAst {
        match command {
            CommandAst::Warn(parsed) => ExpandedCommandAst::Warn(ExpandedModerationCommand {
                command: parsed.clone(),
                expanded_reason: self.expand_reason(parsed.reason.as_ref()),
            }),
            CommandAst::Mute(parsed) => ExpandedCommandAst::Mute(ExpandedMuteCommand {
                command: parsed.clone(),
                expanded_reason: self.expand_reason(parsed.reason.as_ref()),
            }),
            CommandAst::Ban(parsed) => ExpandedCommandAst::Ban(ExpandedModerationCommand {
                command: parsed.clone(),
                expanded_reason: self.expand_reason(parsed.reason.as_ref()),
            }),
            CommandAst::Del(parsed) => ExpandedCommandAst::Del(parsed.clone()),
            CommandAst::Undo(parsed) => ExpandedCommandAst::Undo(parsed.clone()),
            CommandAst::Msg(parsed) => ExpandedCommandAst::Msg(parsed.clone()),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReasonAliasDefinition {
    pub canonical: String,
    pub rule_code: Option<String>,
    pub title: Option<String>,
}

impl ReasonAliasDefinition {
    pub fn new(canonical: impl Into<String>) -> Self {
        Self {
            canonical: canonical.into(),
            rule_code: None,
            title: None,
        }
    }

    pub fn with_rule_code(mut self, rule_code: impl Into<String>) -> Self {
        self.rule_code = Some(rule_code.into());
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum ExpandedReason {
    RuleCode {
        code: String,
    },
    Alias {
        alias: String,
        definition: ReasonAliasDefinition,
    },
    UnknownAlias {
        alias: String,
    },
    Quoted {
        text: String,
    },
    FreeText {
        text: String,
    },
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExpandedCommandLine {
    pub command: ExpandedCommandAst,
    pub pipe: Option<Box<ExpandedCommandLine>>,
    pub execution_mode: crate::event::ExecutionMode,
    pub synthetic: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum ExpandedCommandAst {
    Warn(ExpandedModerationCommand),
    Mute(ExpandedMuteCommand),
    Ban(ExpandedModerationCommand),
    Del(DeleteCommand),
    Undo(UndoCommand),
    Msg(MessageCommand),
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExpandedModerationCommand {
    pub command: ModerationCommand,
    pub expanded_reason: Option<ExpandedReason>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExpandedMuteCommand {
    pub command: MuteCommand,
    pub expanded_reason: Option<ExpandedReason>,
}

#[cfg(test)]
mod tests {
    use super::{ExpandedReason, ReasonAliasDefinition, ReasonAliasRegistry};
    use crate::parser::command::ReasonExpr;

    #[test]
    fn expands_known_and_unknown_aliases() {
        let mut registry = ReasonAliasRegistry::new();
        registry.insert(
            "spam",
            ReasonAliasDefinition::new("spam or scam promotion")
                .with_rule_code("2.8")
                .with_title("Spam"),
        );

        assert_eq!(
            registry.expand_reason(Some(&ReasonExpr::Alias("spam".to_owned()))),
            Some(ExpandedReason::Alias {
                alias: "spam".to_owned(),
                definition: ReasonAliasDefinition {
                    canonical: "spam or scam promotion".to_owned(),
                    rule_code: Some("2.8".to_owned()),
                    title: Some("Spam".to_owned()),
                },
            })
        );
        assert_eq!(
            registry.expand_reason(Some(&ReasonExpr::Alias("unknown".to_owned()))),
            Some(ExpandedReason::UnknownAlias {
                alias: "unknown".to_owned(),
            })
        );
    }
}
