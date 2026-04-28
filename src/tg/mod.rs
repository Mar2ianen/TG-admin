pub mod init;
mod predict;
mod transport;
mod types;
mod validation;

use chrono::Utc;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::storage::{
    EXTERNAL_EFFECT_STATUS_COMPLETED, EXTERNAL_EFFECT_STATUS_ERROR,
    EXTERNAL_EFFECT_STATUS_IN_PROGRESS, ExternalEffectRecord, Storage,
};

pub use predict::*;
pub use transport::*;
pub use types::*;
pub use validation::*;

#[derive(Clone)]
pub struct TelegramGateway {
    polling: bool,
    transport: Arc<dyn TelegramTransport>,
    idempotency_storage: Option<Storage>,
    idempotency_cache: Arc<Mutex<HashMap<String, TelegramResult>>>,
}

impl TelegramGateway {
    pub fn new(polling: bool) -> Self {
        Self {
            polling,
            transport: Arc::new(NoopTelegramTransport),
            idempotency_storage: None,
            idempotency_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_idempotency_storage(mut self, storage: Storage) -> Self {
        self.idempotency_storage = Some(storage);
        self
    }

    pub fn with_transport<T>(mut self, transport: T) -> Self
    where
        T: TelegramTransport + 'static,
    {
        self.transport = Arc::new(transport);
        self
    }

    pub fn transport(&self) -> &dyn TelegramTransport {
        self.transport.as_ref()
    }

    pub fn polling(&self) -> bool {
        self.polling
    }

    pub fn transport_name(&self) -> &'static str {
        self.transport.name()
    }

    pub async fn execute(&self, request: TelegramRequest) -> Result<TelegramResult, TelegramError> {
        self.transport.execute(request).await
    }

    pub async fn execute_checked(
        &self,
        request: TelegramRequest,
        options: TelegramExecutionOptions,
    ) -> Result<TelegramExecution, TelegramError> {
        validate_request(&request)?;

        let operation = request.operation();
        let idempotency_key = request.idempotency_key().map(ToOwned::to_owned);

        if options.dry_run {
            return Ok(TelegramExecution {
                result: predict_result(&request),
                metadata: TelegramExecutionMetadata {
                    operation,
                    dry_run: true,
                    replayed: false,
                    idempotency_key,
                },
            });
        }

        if let Some(key) = request.idempotency_key() {
            if let Some(cached) = self
                .idempotency_cache
                .lock()
                .expect("telegram idempotency cache lock poisoned")
                .get(key)
                .cloned()
            {
                return Ok(TelegramExecution {
                    result: cached,
                    metadata: TelegramExecutionMetadata {
                        operation,
                        dry_run: false,
                        replayed: true,
                        idempotency_key,
                    },
                });
            }
        }

        if let Some(key) = idempotency_key.clone() {
            if let Some(storage) = self.idempotency_storage.as_ref() {
                let request_json = serde_json::to_string(&request).map_err(|error| {
                    TelegramError::new(
                        operation,
                        TelegramErrorKind::Internal,
                        format!("failed to serialize telegram request: {error}"),
                    )
                })?;
                let now = Utc::now().to_rfc3339();
                let connection = storage.open().map_err(|error| {
                    TelegramError::new(
                        operation,
                        TelegramErrorKind::Internal,
                        format!("failed to open telegram idempotency storage: {error}"),
                    )
                })?;
                let effect = connection
                    .reserve_external_effect(&ExternalEffectRecord {
                        idempotency_key: key.clone(),
                        operation: operation.as_str().to_owned(),
                        request_json: request_json.clone(),
                        result_json: None,
                        status: EXTERNAL_EFFECT_STATUS_IN_PROGRESS.to_owned(),
                        created_at: now.clone(),
                        updated_at: now,
                        error_json: None,
                    })
                    .map_err(|error| {
                        TelegramError::new(
                            operation,
                            TelegramErrorKind::Internal,
                            format!("failed to reserve telegram idempotency key: {error}"),
                        )
                    })?;

                let (effect, replayed) = match effect {
                    crate::storage::ExternalEffectReservation::Inserted(effect) => (effect, false),
                    crate::storage::ExternalEffectReservation::Existing(effect) => (effect, true),
                };

                if effect.request_json != request_json {
                    return Err(TelegramError::new(
                        operation,
                        TelegramErrorKind::Conflict,
                        "idempotency key already exists for a different telegram request",
                    )
                    .with_details(serde_json::json!({
                        "idempotency_key": key,
                        "stored_operation": effect.operation,
                    })));
                }

                if replayed {
                    match effect.status.as_str() {
                        EXTERNAL_EFFECT_STATUS_COMPLETED => {
                            let result_json = effect.result_json.as_deref().ok_or_else(|| {
                                TelegramError::new(
                                    operation,
                                    TelegramErrorKind::Internal,
                                    "completed telegram idempotency row is missing result_json",
                                )
                            })?;
                            let result = serde_json::from_str::<TelegramResult>(result_json)
                                .map_err(|error| {
                                    TelegramError::new(
                                        operation,
                                        TelegramErrorKind::Internal,
                                        format!("failed to deserialize telegram result: {error}"),
                                    )
                                })?;

                            self.idempotency_cache
                                .lock()
                                .expect("telegram idempotency cache lock poisoned")
                                .insert(key.clone(), result.clone());

                            return Ok(TelegramExecution {
                                result,
                                metadata: TelegramExecutionMetadata {
                                    operation,
                                    dry_run: false,
                                    replayed: true,
                                    idempotency_key,
                                },
                            });
                        }
                        EXTERNAL_EFFECT_STATUS_IN_PROGRESS | EXTERNAL_EFFECT_STATUS_ERROR => {
                            return Err(TelegramError::new(
                                operation,
                                TelegramErrorKind::Conflict,
                                "telegram idempotency key is already reserved",
                            )
                            .with_details(serde_json::json!({
                                "idempotency_key": effect.idempotency_key,
                                "status": effect.status,
                            })));
                        }
                        other => {
                            return Err(TelegramError::new(
                                operation,
                                TelegramErrorKind::Internal,
                                format!("unexpected telegram idempotency status `{other}`"),
                            ));
                        }
                    }
                }
            }
        }

        let result = match self.transport.execute(request).await {
            Ok(result) => result,
            Err(error) => {
                if let Some(key) = idempotency_key.as_ref() {
                    if let Some(storage) = self.idempotency_storage.as_ref() {
                        if let Ok(connection) = storage.open() {
                            if let Ok(error_json) = serde_json::to_string(&error) {
                                let now = Utc::now().to_rfc3339();
                                let _ = connection.fail_external_effect(key, &error_json, &now);
                            }
                        }
                    }
                }

                return Err(error);
            }
        };

        if let Some(key) = idempotency_key.clone() {
            if let Some(storage) = self.idempotency_storage.as_ref() {
                let connection = storage.open().map_err(|error| {
                    TelegramError::new(
                        operation,
                        TelegramErrorKind::Internal,
                        format!("failed to open telegram idempotency storage: {error}"),
                    )
                })?;
                let now = Utc::now().to_rfc3339();
                let result_json = serde_json::to_string(&result).map_err(|error| {
                    TelegramError::new(
                        operation,
                        TelegramErrorKind::Internal,
                        format!("failed to serialize telegram result: {error}"),
                    )
                })?;
                let completed = connection
                    .complete_external_effect(&key, &result_json, &now)
                    .map_err(|error| {
                        TelegramError::new(
                            operation,
                            TelegramErrorKind::Internal,
                            format!("failed to persist telegram idempotency result: {error}"),
                        )
                    })?;

                if !completed {
                    return Err(TelegramError::new(
                        operation,
                        TelegramErrorKind::Internal,
                        "telegram idempotency row was not in progress when completing",
                    ));
                }
            }

            self.idempotency_cache
                .lock()
                .expect("telegram idempotency cache lock poisoned")
                .insert(key, result.clone());
        }

        Ok(TelegramExecution {
            result,
            metadata: TelegramExecutionMetadata {
                operation,
                dry_run: false,
                replayed: false,
                idempotency_key,
            },
        })
    }
}

impl fmt::Debug for TelegramGateway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramGateway")
            .field("polling", &self.polling)
            .field("transport", &self.transport.name())
            .finish()
    }
}

impl Default for TelegramGateway {
    fn default() -> Self {
        Self::new(true)
    }
}

impl fmt::Display for TelegramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.operation.as_str(), self.message)
    }
}

impl std::error::Error for TelegramError {}

#[cfg(test)]
mod tests;
