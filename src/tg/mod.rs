mod predict;
mod transport;
mod types;
mod validation;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

pub use predict::*;
pub use transport::*;
pub use types::*;
pub use validation::*;

#[derive(Clone)]
pub struct TelegramGateway {
    polling: bool,
    transport: Arc<dyn TelegramTransport>,
    idempotency_cache: Arc<Mutex<HashMap<String, TelegramResult>>>,
}

impl TelegramGateway {
    pub fn new(polling: bool) -> Self {
        Self {
            polling,
            transport: Arc::new(NoopTelegramTransport),
            idempotency_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_transport<T>(mut self, transport: T) -> Self
    where
        T: TelegramTransport + 'static,
    {
        self.transport = Arc::new(transport);
        self
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

        let result = self.transport.execute(request).await?;

        if let Some(key) = idempotency_key.clone() {
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
