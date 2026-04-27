use crate::storage::Storage;
use crate::tg::{ChatId, TelegramRequest, TelegramResult, TelegramTransport};
use anyhow::{Result, anyhow};

pub struct ChatInitializer<'a> {
    transport: &'a dyn TelegramTransport,
    storage: &'a Storage,
}

impl<'a> ChatInitializer<'a> {
    pub fn new(transport: &'a dyn TelegramTransport, storage: &'a Storage) -> Self {
        Self { transport, storage }
    }

    pub async fn initialize_chat(&self, chat_id: ChatId, bot_id: i64) -> Result<()> {
        let request = TelegramRequest::GetChatAdministrators(
            crate::tg::TelegramGetChatAdministratorsRequest { chat_id },
        );

        match self.transport.execute(request).await? {
            TelegramResult::ChatAdministrators(result) => {
                let is_admin = result.administrators.iter().any(|m| m.user_id == bot_id);
                let mut conn = self.storage.open()?;
                conn.set_bot_is_admin(chat_id, is_admin)?;
                Ok(())
            }
            _ => Err(anyhow!("Unexpected result type from GetChatAdministrators")),
        }
    }
}
