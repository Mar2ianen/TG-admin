use crate::event::EventContext;
use crate::moderation::{ModerationEngine, ModerationError};
use crate::tg::{ParseMode, TelegramRequest, TelegramSendMessageRequest};

impl ModerationEngine {
    pub async fn on_member_joined(&self, event: &EventContext) -> Result<(), ModerationError> {
        let member = event
            .chat_member
            .as_ref()
            .ok_or_else(|| ModerationError::InvalidEvent("missing chat member".into()))?;
        let user = &member.user;

        // 1. Регистрация в базе
        self.register_member(
            user.id,
            user.username.clone(),
            Some(user.first_name.clone()),
        )
        .await?;

        // 2. Приветствие через шаблоны
        let chat_id = event.chat.as_ref().map(|c| c.id).unwrap_or(0);
        let template = "Привет, {{user_name}}! Добро пожаловать."; // TODO: Загрузка из bundled_templates

        let request = TelegramRequest::SendMessage(TelegramSendMessageRequest {
            chat_id,
            text: template.replace("{{user_name}}", &user.first_name),
            reply_to_message_id: None,
            silent: true,
            parse_mode: ParseMode::PlainText,
            markup: None,
        });

        let _ = self
            .gateway
            .execute(request)
            .await
            .map_err(ModerationError::Telegram)?;
        Ok(())
    }

    pub async fn on_member_left(&self, event: &EventContext) -> Result<(), ModerationError> {
        let member = event
            .chat_member
            .as_ref()
            .ok_or_else(|| ModerationError::InvalidEvent("missing chat member".into()))?;
        let user = &member.user;

        let chat_id = event.chat.as_ref().map(|c| c.id).unwrap_or(0);
        let template = "Пользователь {{user_name}} покинул нас.";

        let request = TelegramRequest::SendMessage(TelegramSendMessageRequest {
            chat_id,
            text: template.replace("{{user_name}}", &user.first_name),
            reply_to_message_id: None,
            silent: true,
            parse_mode: ParseMode::PlainText,
            markup: None,
        });

        let _ = self
            .gateway
            .execute(request)
            .await
            .map_err(ModerationError::Telegram)?;
        Ok(())
    }
}
