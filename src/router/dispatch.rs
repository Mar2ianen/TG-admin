use super::classify::extract_command_name;
use super::types::{UnitDispatchInvocation, UnitDispatchTrigger};
use crate::event::{EventContext, UpdateType};
use crate::unit::{TriggerSpec, UnitRegistry, UnitStatus};
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

            println!("Matched unit {} with trigger {:?}", descriptor.id, trigger);

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
            let unit_event = unit_event_type_for(event)?;
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

fn unit_event_type_for(event: &EventContext) -> Option<crate::unit::UnitEventType> {
    match event.update_type {
        UpdateType::Message
        | UpdateType::EditedMessage
        | UpdateType::ChannelPost
        | UpdateType::EditedChannelPost => Some(crate::unit::UnitEventType::Message),
        UpdateType::CallbackQuery => Some(crate::unit::UnitEventType::CallbackQuery),
        UpdateType::Job => Some(crate::unit::UnitEventType::Job),
        UpdateType::ChatMember | UpdateType::MyChatMember | UpdateType::ChatMemberUpdated => {
            let member = event.chat_member.as_ref()?;
            match (member.old_status.as_str(), member.new_status.as_str()) {
                ("Left" | "Kicked", "Member" | "Administrator" | "Owner" | "Restricted") => {
                    Some(crate::unit::UnitEventType::MemberJoined)
                }
                ("Member" | "Administrator" | "Owner" | "Restricted", "Left" | "Kicked") => {
                    Some(crate::unit::UnitEventType::MemberLeft)
                }
                _ => Some(crate::unit::UnitEventType::MemberUpdated),
            }
        }
        UpdateType::MessageReaction | UpdateType::MessageReactionCount => {
            let reaction = event.reaction.as_ref()?;
            if reaction.new_reaction.is_empty() && !reaction.old_reaction.is_empty() {
                Some(crate::unit::UnitEventType::ReactionRemoved)
            } else {
                Some(crate::unit::UnitEventType::ReactionAdded)
            }
        }
        UpdateType::JoinRequest => Some(crate::unit::UnitEventType::MemberJoined),
        _ => None,
    }
}
