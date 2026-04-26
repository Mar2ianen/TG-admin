use super::classify::extract_command_name;
use super::types::{UnitDispatchInvocation, UnitDispatchTrigger};
use crate::event::{EventContext, UpdateType};
use crate::unit::{TriggerSpec, UnitEventType, UnitRegistry, UnitStatus};
use regex::Regex;

pub fn select_unit_dispatches(
    registry: &UnitRegistry,
    event: &EventContext,
) -> Vec<UnitDispatchInvocation> {
    registry
        .entries()
        .iter()
        .filter(|descriptor| matches!(descriptor.status, UnitStatus::Loaded | UnitStatus::Active))
        .filter_map(|descriptor| {
            let manifest = descriptor.manifest.as_ref()?;
            let trigger = match_trigger(&manifest.trigger, event)?;

            tracing::debug!(unit_id = %descriptor.id, trigger = ?trigger, "Matched unit with trigger");

            Some(UnitDispatchInvocation {
                unit_id: descriptor.id.clone(),
                exec_start: manifest.service.exec_start.clone(),
                entry_point: manifest.service.entry_point.clone(),
                trigger,
            })
        })
        .collect()
}

pub fn match_trigger(trigger: &TriggerSpec, event: &EventContext) -> Option<UnitDispatchTrigger> {
    match trigger {
        TriggerSpec::Command { commands } => {
            let command = extract_command_name(event)?;
            commands
                .iter()
                .find(|candidate| candidate.trim().eq_ignore_ascii_case(&command))
                .map(|_| UnitDispatchTrigger::Command { command })
        }
        TriggerSpec::Regex { pattern } => {
            let haystack = trigger_text(event)?;
            Regex::new(pattern)
                .ok()
                .filter(|regex| regex.is_match(haystack))
                .map(|_| UnitDispatchTrigger::Regex {
                    pattern: pattern.clone(),
                })
        }
        TriggerSpec::EventType { events } => {
            let unit_event = unit_event_type_for(event.update_type)?;
            events
                .iter()
                .copied()
                .find(|candidate| *candidate == unit_event)
                .map(|event| UnitDispatchTrigger::EventType { event })
        }
    }
}

fn trigger_text(event: &EventContext) -> Option<&str> {
    event
        .message
        .as_ref()
        .and_then(|message| message.text.as_deref())
        .or_else(|| {
            event
                .callback
                .as_ref()
                .and_then(|callback| callback.data.as_deref())
        })
}

fn unit_event_type_for(update_type: UpdateType) -> Option<crate::unit::UnitEventType> {
    match update_type {
        UpdateType::Message
        | UpdateType::EditedMessage
        | UpdateType::ChannelPost
        | UpdateType::EditedChannelPost => Some(crate::unit::UnitEventType::Message),
        UpdateType::CallbackQuery => Some(crate::unit::UnitEventType::CallbackQuery),
        UpdateType::Job => Some(crate::unit::UnitEventType::Job),
        UpdateType::ChatMember
        | UpdateType::MyChatMember
        | UpdateType::JoinRequest
        | UpdateType::System => None,
    }
}
