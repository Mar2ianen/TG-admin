use crate::event::{
    ChatContext, EventContext, EventNormalizer, MessageContentKind, MessageContext, ReplyContext,
    SenderContext, TelegramUpdateInput, UpdateType,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramExport {
    pub name: String,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub id: i64,
    pub messages: Vec<ExportMessage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExportMessage {
    pub id: i32,
    #[serde(rename = "type")]
    pub message_type: String,
    pub date: String,
    pub date_unixtime: String,
    pub from: Option<String>,
    pub from_id: Option<String>,
    pub actor: Option<String>,
    pub actor_id: Option<String>,
    pub action: Option<String>,
    pub text: serde_json::Value, // Text can be string or array of entities
    pub reply_to_message_id: Option<i32>,
    pub media_type: Option<String>,
    pub file: Option<String>,
    pub photo: Option<String>,
    pub sticker_emoji: Option<String>,
}

pub struct JsonExportAdapter {
    normalizer: EventNormalizer,
}

impl JsonExportAdapter {
    pub fn new() -> Self {
        Self {
            normalizer: EventNormalizer::new(),
        }
    }

    pub fn convert_export(&self, export: TelegramExport) -> Result<Vec<EventContext>> {
        let chat = ChatContext {
            id: export.id,
            chat_type: export.chat_type,
            title: Some(export.name),
            username: None,
            thread_id: None,
        };

        let mut events = Vec::with_capacity(export.messages.len());
        for msg in export.messages {
            if let Some(input) = self.convert_message(&chat, msg) {
                let event = self.normalizer.normalize_telegram(input)?;
                events.push(event);
            }
        }
        Ok(events)
    }

    fn convert_message(
        &self,
        chat: &ChatContext,
        msg: ExportMessage,
    ) -> Option<TelegramUpdateInput> {
        let update_type = if msg.message_type == "service" {
            match msg.action.as_deref() {
                Some("invite_members") => UpdateType::ChatMember,
                _ => return None, // Ignore other service messages for now
            }
        } else {
            UpdateType::Message
        };

        let sender_id_str = msg.from_id.as_ref().or(msg.actor_id.as_ref())?;
        let sender_id = parse_telegram_id(sender_id_str)?;

        let sender = SenderContext {
            id: sender_id,
            username: None,
            display_name: msg.from.clone().or(msg.actor.clone()),
            first_name: msg.from.clone().or(msg.actor.clone()).unwrap_or_default(),
            last_name: None,
            is_bot: sender_id_str.starts_with("bot"),
            is_admin: false,
            role: None,
        };

        let text = parse_text_value(msg.text);

        let content_kind = match msg.media_type.as_deref() {
            Some("voice_message") => MessageContentKind::Voice,
            Some("video_message") => MessageContentKind::VideoNote,
            Some("sticker") => MessageContentKind::Sticker,
            _ if msg.photo.is_some() => MessageContentKind::Photo,
            _ => MessageContentKind::Text,
        };

        let timestamp = msg.date_unixtime.parse::<i64>().ok()?;
        let date = DateTime::from_timestamp(timestamp, 0)?;

        let message = if update_type == UpdateType::Message {
            Some(MessageContext {
                id: msg.id,
                date,
                text: Some(text),
                content_kind: Some(content_kind),
                entities: Vec::new(),
                has_media: content_kind != MessageContentKind::Text,
                file_ids: msg.file.or(msg.photo).into_iter().collect(),
                reply_to_message_id: msg.reply_to_message_id,
                media_group_id: None,
            })
        } else {
            None
        };

        Some(TelegramUpdateInput {
            event_id: Some(format!("json_{}", msg.id)),
            update_id: msg.id as u64,
            update_type,
            received_at: date,
            execution_mode: crate::event::ExecutionMode::Realtime,
            chat: chat.clone(),
            sender: Some(sender),
            message,
            reply: None, // We don't have enough info for full ReplyContext easily here
            callback: None,
            chat_member: None,
            reaction: None,
            locale: None,
            trace_id: None,
            build: None,
        })
    }
}

fn parse_telegram_id(id: &str) -> Option<i64> {
    if id.starts_with("user") {
        id.strip_prefix("user")?.parse().ok()
    } else if id.starts_with("channel") {
        id.strip_prefix("channel")?.parse().ok()
    } else {
        id.parse().ok()
    }
}

fn parse_text_value(val: serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s,
        serde_json::Value::Array(arr) => {
            let mut result = String::new();
            for item in arr {
                match item {
                    serde_json::Value::String(s) => result.push_str(&s),
                    serde_json::Value::Object(map) => {
                        if let Some(serde_json::Value::String(text)) = map.get("text") {
                            result.push_str(text);
                        }
                    }
                    _ => {}
                }
            }
            result
        }
        _ => String::new(),
    }
}
