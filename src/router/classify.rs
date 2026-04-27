use super::types::*;
use crate::event::{EventContext, MessageContentKind, UpdateType};

pub fn classify_event(event: &EventContext) -> ClassifiedEvent {
    let mut traits = Vec::new();
    push_unique(&mut traits, event_trait_for_update_type(event.update_type));

    if event
        .message
        .as_ref()
        .and_then(|message| message.text.as_deref())
        .is_some_and(|text| !text.trim().is_empty())
    {
        push_unique(&mut traits, EventTrait::Text);
    }
    if event.reply.is_some() {
        push_unique(&mut traits, EventTrait::Reply);
    }
    if event
        .message
        .as_ref()
        .is_some_and(|message| message.has_media)
    {
        push_unique(&mut traits, EventTrait::Media);
    }
    if let Some(content_kind) = event
        .message
        .as_ref()
        .and_then(|message| message.content_kind)
    {
        push_unique(&mut traits, event_trait_for_content_kind(content_kind));
        if !matches!(content_kind, MessageContentKind::Text) {
            push_unique(&mut traits, EventTrait::Media);
        }
    }
    if event
        .message
        .as_ref()
        .and_then(|message| message.media_group_id.as_deref())
        .is_some()
    {
        push_unique(&mut traits, EventTrait::MediaGroup);
    }
    if event
        .callback
        .as_ref()
        .and_then(|callback| callback.data.as_deref())
        .is_some()
    {
        push_unique(&mut traits, EventTrait::CallbackData);
    }
    if event.is_linked_channel_style_approx() {
        push_unique(&mut traits, EventTrait::LinkedChannelStyle);
    }

    let command_name = extract_command_name(event);
    if command_name.is_some() {
        push_unique(&mut traits, EventTrait::Command);
    }

    ClassifiedEvent {
        ingress_class: ingress_class_for(event.execution_mode),
        chat_scope: chat_scope_for(event),
        author_kind: author_kind_for(event),
        update_type: event.update_type,
        traits,
        command_name,
    }
}

fn ingress_class_for(mode: crate::event::ExecutionMode) -> IngressClass {
    match mode {
        crate::event::ExecutionMode::Realtime => IngressClass::Realtime,
        crate::event::ExecutionMode::Recovery => IngressClass::Recovery,
        crate::event::ExecutionMode::Scheduled => IngressClass::Scheduled,
        crate::event::ExecutionMode::Manual => IngressClass::Manual,
    }
}

fn chat_scope_for(event: &EventContext) -> ChatScope {
    match event.chat.as_ref().map(|chat| chat.route_class()) {
        Some(crate::event::ChatRouteClass::Private) => ChatScope::Private,
        Some(crate::event::ChatRouteClass::GroupLike) => {
            match event.chat.as_ref().map(|chat| chat.chat_type.as_str()) {
                Some("group") => ChatScope::Group,
                Some("supergroup") => ChatScope::Supergroup,
                _ => ChatScope::Unknown,
            }
        }
        Some(crate::event::ChatRouteClass::Channel) => ChatScope::Channel,
        _ => ChatScope::Unknown,
    }
}

fn author_kind_for(event: &EventContext) -> AuthorKind {
    match event.author_source_class() {
        crate::event::AuthorSourceClass::HumanAdmin => AuthorKind::HumanAdmin,
        crate::event::AuthorSourceClass::HumanMember => AuthorKind::HumanMember,
        crate::event::AuthorSourceClass::Bot => AuthorKind::Bot,
        crate::event::AuthorSourceClass::ChannelStyleNoSender => AuthorKind::ChannelIdentity,
        crate::event::AuthorSourceClass::Unknown => AuthorKind::System,
    }
}

fn event_trait_for_update_type(update_type: UpdateType) -> EventTrait {
    match update_type {
        UpdateType::Message => EventTrait::Message,
        UpdateType::EditedMessage => EventTrait::EditedMessage,
        UpdateType::ChannelPost => EventTrait::ChannelPost,
        UpdateType::EditedChannelPost => EventTrait::EditedChannelPost,
        UpdateType::CallbackQuery => EventTrait::CallbackQuery,
        UpdateType::ChatMember => EventTrait::ChatMember,
        UpdateType::MyChatMember => EventTrait::MyChatMember,
        UpdateType::ChatMemberUpdated => EventTrait::ChatMemberUpdated,
        UpdateType::MessageReaction => EventTrait::MessageReaction,
        UpdateType::MessageReactionCount => EventTrait::MessageReactionCount,
        UpdateType::JoinRequest => EventTrait::JoinRequest,
        UpdateType::Job => EventTrait::Job,
        UpdateType::System => EventTrait::System,
    }
}

fn event_trait_for_content_kind(content_kind: MessageContentKind) -> EventTrait {
    match content_kind {
        MessageContentKind::Text => EventTrait::Text,
        MessageContentKind::Photo => EventTrait::Photo,
        MessageContentKind::Voice => EventTrait::Voice,
        MessageContentKind::Video => EventTrait::Video,
        MessageContentKind::Audio => EventTrait::Audio,
        MessageContentKind::Document => EventTrait::Document,
        MessageContentKind::Sticker => EventTrait::Sticker,
        MessageContentKind::Animation => EventTrait::Animation,
        MessageContentKind::VideoNote => EventTrait::VideoNote,
        MessageContentKind::Contact => EventTrait::Contact,
        MessageContentKind::Location => EventTrait::Location,
        MessageContentKind::Poll => EventTrait::Poll,
        MessageContentKind::Dice => EventTrait::Dice,
        MessageContentKind::Venue => EventTrait::Venue,
        MessageContentKind::Game => EventTrait::Game,
        MessageContentKind::Invoice => EventTrait::Invoice,
        MessageContentKind::Story => EventTrait::Story,
        MessageContentKind::UnknownMedia => EventTrait::UnknownMedia,
    }
}

pub(crate) fn extract_command_name(event: &EventContext) -> Option<String> {
    match event.command_source()? {
        crate::event::CommandSource::MessageText(text)
        | crate::event::CommandSource::CallbackData(text) => parse_command_name(text),
    }
}

fn parse_command_name(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(command) = raw.strip_prefix('/') {
        let token = command.split_whitespace().next()?;
        let command_name = token.split('@').next().unwrap_or(token);
        if command_name.is_empty()
            || !command_name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return None;
        }
        return Some(command_name.to_ascii_lowercase());
    }

    // Обработка префиксов callback data
    if let Some(rest) = raw.strip_prefix("warn:") {
        if rest.chars().all(|c| c.is_ascii_digit()) {
            return Some("warn".to_owned());
        }
    }
    if let Some(rest) = raw.strip_prefix("mute:") {
        if rest.chars().all(|c| c.is_ascii_digit()) {
            return Some("mute".to_owned());
        }
    }
    if let Some(rest) = raw.strip_prefix("ban:") {
        if rest.chars().all(|c| c.is_ascii_digit()) {
            return Some("ban".to_owned());
        }
    }

    None
}

fn push_unique<T>(items: &mut Vec<T>, item: T)
where
    T: PartialEq,
{
    if !items.contains(&item) {
        items.push(item);
    }
}
