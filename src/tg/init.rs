use crate::storage::Storage;
use crate::tg::{ChatId, TelegramRequest, TelegramResult, TelegramTransport};
use anyhow::{Context, Result, anyhow};
use std::convert::TryFrom;
use teloxide_core::requests::Requester;

pub struct ChatInitializer<'a> {
    transport: &'a dyn TelegramTransport,
    storage: &'a Storage,
}

impl<'a> ChatInitializer<'a> {
    pub fn new(transport: &'a dyn TelegramTransport, storage: &'a Storage) -> Self {
        Self { transport, storage }
    }

    pub async fn initialize_chat(&self, chat_id: ChatId, bot_id: i64) -> Result<bool> {
        let request = TelegramRequest::GetChatAdministrators(
            crate::tg::TelegramGetChatAdministratorsRequest { chat_id },
        );

        match self.transport.execute(request).await.with_context(|| {
            format!("failed to fetch Telegram administrators for chat {chat_id}")
        })? {
            TelegramResult::ChatAdministrators(result) => {
                let admin_user_ids: Vec<i64> = result
                    .administrators
                    .iter()
                    .map(|member| member.user_id)
                    .collect();
                let is_admin = result.administrators.iter().any(|m| m.user_id == bot_id);
                let conn = self.storage.open()?;
                conn.replace_chat_admin_roster(chat_id, &admin_user_ids)
                    .with_context(|| {
                        format!("failed to persist chat admin roster for chat {chat_id}")
                    })?;
                conn.set_bot_is_admin(chat_id, is_admin).with_context(|| {
                    format!("failed to persist bot admin state for chat {chat_id}")
                })?;
                Ok(is_admin)
            }
            _ => Err(anyhow!(
                "unexpected result type from GetChatAdministrators for chat {chat_id}"
            )),
        }
    }

    pub async fn initialize_primary_chats<I>(&self, chat_ids: I, bot_id: i64) -> Result<()>
    where
        I: IntoIterator<Item = ChatId>,
    {
        for chat_id in chat_ids {
            let is_admin = self
                .initialize_chat(chat_id, bot_id)
                .await
                .with_context(|| format!("failed to initialize primary chat {chat_id}"))?;
            if !is_admin {
                return Err(anyhow!(
                    "Telegram bot {bot_id} is not an administrator in primary chat {chat_id}"
                ));
            }
        }

        Ok(())
    }
}

pub async fn fetch_bot_id(bot: &teloxide_core::Bot) -> Result<i64> {
    let me = bot
        .get_me()
        .await
        .context("failed to fetch Telegram bot identity with getMe")?;

    i64::try_from(me.id.0).context("Telegram bot identity does not fit in i64")
}

#[cfg(test)]
mod tests {
    use super::ChatInitializer;
    use crate::storage::Storage;
    use crate::tg::{
        TelegramChatAdministratorsResult, TelegramChatMember, TelegramError, TelegramRequest,
        TelegramResult, TelegramTransport,
    };
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    #[derive(Debug)]
    struct StaticTransport {
        result: TelegramResult,
        requests: Arc<Mutex<Vec<TelegramRequest>>>,
    }

    #[async_trait]
    impl TelegramTransport for StaticTransport {
        fn name(&self) -> &'static str {
            "static"
        }

        async fn execute(
            &self,
            request: TelegramRequest,
        ) -> std::result::Result<TelegramResult, TelegramError> {
            self.requests.lock().expect("requests lock").push(request);
            Ok(self.result.clone())
        }
    }

    #[derive(Debug)]
    struct FailingTransport;

    #[async_trait]
    impl TelegramTransport for FailingTransport {
        fn name(&self) -> &'static str {
            "failing"
        }

        async fn execute(
            &self,
            _request: TelegramRequest,
        ) -> std::result::Result<TelegramResult, TelegramError> {
            Err(TelegramError::transport_unavailable(
                crate::tg::TelegramOperation::GetChatAdministrators,
                "boom",
            ))
        }
    }

    fn storage() -> (TempDir, Storage) {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = Storage::new(dir.path().join("runtime.sqlite3"));
        storage.init().expect("init storage schema");
        (dir, storage)
    }

    #[tokio::test]
    async fn initialize_chat_records_admin_status_for_matching_bot_id() {
        let (_dir, storage) = storage();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let transport = StaticTransport {
            result: TelegramResult::ChatAdministrators(TelegramChatAdministratorsResult {
                chat_id: -100123,
                administrators: vec![TelegramChatMember {
                    user_id: 42,
                    is_admin: true,
                    can_restrict_members: Some(true),
                }],
            }),
            requests,
        };
        let initializer = ChatInitializer::new(&transport, &storage);

        let is_admin = initializer
            .initialize_chat(-100123, 42)
            .await
            .expect("chat initialization succeeds");

        assert!(is_admin);
        assert!(
            storage
                .open()
                .expect("storage open")
                .get_bot_is_admin(-100123)
                .expect("load state")
        );
        assert_eq!(
            storage
                .open()
                .expect("storage open")
                .get_chat_user_is_admin(-100123, 42)
                .expect("load admin cache"),
            Some(true)
        );
    }

    #[tokio::test]
    async fn initialize_primary_chats_reports_clear_context_on_transport_failure() {
        let (_dir, storage) = storage();
        let transport = FailingTransport;
        let initializer = ChatInitializer::new(&transport, &storage);

        let error = initializer
            .initialize_primary_chats([-100123], 42)
            .await
            .expect_err("primary chat initialization must fail");

        assert!(error.to_string().contains("primary chat -100123"));
    }
}
