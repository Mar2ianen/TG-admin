use crate::parser::duration::DurationParseError;
use crate::parser::target::TargetParseError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::HostApiOperation;

#[derive(Debug, Clone, Error, Eq, PartialEq, Serialize, Deserialize)]
#[error("{kind:?} host api error in {operation:?}: {detail}")]
pub struct HostApiError {
    pub operation: HostApiOperation,
    pub kind: HostApiErrorKind,
    pub detail: HostApiErrorDetail,
}

impl HostApiError {
    pub(crate) fn validation(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Validation,
            detail,
        }
    }

    pub(crate) fn parse(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Parse,
            detail,
        }
    }

    pub(crate) fn denied(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Denied,
            detail,
        }
    }

    pub(crate) fn internal(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Internal,
            detail,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostApiErrorKind {
    Validation,
    Parse,
    Denied,
    Internal,
}

#[derive(Debug, Clone, Error, Eq, PartialEq, Serialize, Deserialize)]
pub enum HostApiErrorDetail {
    #[error("invalid event context: {message}")]
    InvalidEventContext { message: String },
    #[error("invalid target `{value}`: {source}")]
    InvalidTarget {
        value: String,
        source: TargetParseError,
    },
    #[error("no target could be resolved from request or event context")]
    NoResolvableTarget,
    #[error("invalid duration `{value}`: {source}")]
    InvalidDuration {
        value: String,
        source: DurationParseError,
    },
    #[error("invalid field `{field}`: {message}")]
    InvalidField { field: String, message: String },
    #[error("invalid counter change for `{field}`: current={current}, delta={delta}")]
    InvalidCounterChange {
        field: String,
        current: i64,
        delta: i64,
    },
    #[error("message window too large: requested {requested}, max {max}")]
    MessageWindowTooLarge { requested: usize, max: usize },
    #[error("scheduled job delay `{delay}` exceeds max {max_days} days")]
    JobTooFarInFuture { delay: String, max_days: i64 },
    #[error("audit.find requires at least one filter")]
    MissingAuditFilter,
    #[error("unknown unit `{unit_id}`")]
    UnknownUnit { unit_id: String },
    #[error("operation denied for unit `{unit_id}`: missing capability `{capability}`")]
    CapabilityDenied { capability: String, unit_id: String },
    #[error("unknown audit action `{action_id}`")]
    UnknownAuditAction { action_id: String },
    #[error("required host resource `{resource}` is unavailable")]
    ResourceUnavailable { resource: String },
    #[error("storage failure: {message}")]
    StorageFailure { message: String },
    #[error("internal conversion failed: {message}")]
    InternalConversionFailure { message: String },
    #[error("reason expansion unexpectedly returned no result")]
    ReasonExpansionUnavailable,
}
