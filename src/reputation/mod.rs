use crate::moderation::GlobalSpammerRecord;
use anyhow::Result;
use chrono::Utc;
use moka::future::Cache;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationCheckRequest {
    pub user_id: i64,
    pub phash: Option<String>,
    pub bio_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationCheckResponse {
    pub is_spammer: bool,
    pub record: Option<GlobalSpammerRecord>,
    pub action_recommended: String,
    pub similarity_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotlistResponse {
    pub count: usize,
    pub records: Vec<GlobalSpammerRecord>,
}

#[derive(Debug)]
pub struct ReputationClient {
    http: Client,
    base_url: String,
    cache: Cache<i64, ReputationCheckResponse>,
    bot_id: String,
}

impl ReputationClient {
    pub fn new(base_url: String, bot_id: String) -> Self {
        let cache = Cache::builder()
            .max_capacity(10_000)
            .time_to_live(std::time::Duration::from_secs(3600)) // 1 час жизни записи
            .build();

        Self {
            http: Client::new(),
            base_url,
            cache,
            bot_id,
        }
    }

    /// Проверка пользователя (сначала кэш, потом API)
    pub async fn check(
        &self,
        user_id: i64,
        phash: Option<String>,
        bio_text: Option<String>,
    ) -> Result<ReputationCheckResponse> {
        if let Some(cached) = self.cache.get(&user_id).await {
            return Ok(cached);
        }

        let req = ReputationCheckRequest {
            user_id,
            phash,
            bio_text,
        };

        let res: ReputationCheckResponse = self
            .http
            .post(format!("{}/v1/reputation/check", self.base_url))
            .json(&req)
            .send()
            .await?
            .json()
            .await?;

        self.cache.insert(user_id, res.clone()).await;
        Ok(res)
    }

    /// Репорт нового спамера в глобальную базу
    pub async fn report(&self, record: GlobalSpammerRecord, evidence: String) -> Result<()> {
        #[derive(Serialize)]
        struct ReportRequest {
            #[serde(flatten)]
            record: GlobalSpammerRecord,
            evidence_message: String,
        }

        let req = ReportRequest {
            record,
            evidence_message: evidence,
        };

        self.http
            .post(format!("{}/v1/reputation/report", self.base_url))
            .json(&req)
            .send()
            .await?;

        Ok(())
    }

    /// Прогрев кэша (загрузка горячего списка)
    pub async fn warm_cache(&self, limit: usize) -> Result<()> {
        let res: HotlistResponse = self
            .http
            .get(format!(
                "{}/v1/reputation/hotlist?limit={}",
                self.base_url, limit
            ))
            .send()
            .await?
            .json()
            .await?;

        for record in res.records {
            let user_id = record.user_id;
            let check_res = ReputationCheckResponse {
                is_spammer: true,
                record: Some(record),
                action_recommended: "ban".to_string(),
                similarity_score: 1.0,
            };
            self.cache.insert(user_id, check_res).await;
        }

        Ok(())
    }
}
