use std::rc::Rc;

use crate::event::EventContext;
use crate::parser::command::ReasonExpr;
use crate::parser::duration::{DurationParseError, DurationParser, ParsedDuration};
use crate::parser::reason::{ExpandedReason, ReasonAliasRegistry};
use crate::parser::target::{
    ParsedTargetSelector, ResolvedTarget, TargetParseError, TargetSelectorParser, resolve_target,
};
use crate::storage::{KvEntry, StorageConnection, StorageError, UserPatch, UserRecord};
use crate::unit::{UnitDiagnostic, UnitRegistry, UnitRegistryStatus, UnitStatus};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct HostApi {
    dry_run: bool,
    storage: Option<Rc<StorageConnection>>,
    unit_registry: Option<Rc<UnitRegistry>>,
    target_parser: TargetSelectorParser,
    duration_parser: DurationParser,
    aliases: ReasonAliasRegistry,
}

impl HostApi {
    pub fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            storage: None,
            unit_registry: None,
            target_parser: TargetSelectorParser::new(),
            duration_parser: DurationParser::new(),
            aliases: ReasonAliasRegistry::new(),
        }
    }

    pub fn with_reason_aliases(mut self, aliases: ReasonAliasRegistry) -> Self {
        self.aliases = aliases;
        self
    }

    pub fn with_storage(mut self, storage: StorageConnection) -> Self {
        self.storage = Some(Rc::new(storage));
        self
    }

    pub fn with_storage_handle(mut self, storage: Rc<StorageConnection>) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn with_unit_registry(mut self, registry: UnitRegistry) -> Self {
        self.unit_registry = Some(Rc::new(registry));
        self
    }

    pub fn with_unit_registry_handle(mut self, registry: Rc<UnitRegistry>) -> Self {
        self.unit_registry = Some(registry);
        self
    }

    pub fn dry_run(&self) -> bool {
        self.dry_run
    }

    pub fn call(
        &self,
        event: &EventContext,
        request: HostApiRequest,
    ) -> Result<HostApiResponse<HostApiValue>, HostApiError> {
        match request {
            HostApiRequest::CtxCurrent => self
                .ctx_current(event)
                .map(|response| response.map(|value| HostApiValue::CtxCurrent(Box::new(value)))),
            HostApiRequest::CtxResolveTarget(request) => self
                .ctx_resolve_target(event, request)
                .map(|response| response.map(HostApiValue::ResolvedTarget)),
            HostApiRequest::CtxParseDuration(request) => self
                .ctx_parse_duration(event, request)
                .map(|response| response.map(HostApiValue::ParsedDuration)),
            HostApiRequest::CtxExpandReason(request) => self
                .ctx_expand_reason(event, request)
                .map(|response| response.map(HostApiValue::ExpandedReason)),
            HostApiRequest::DbUserGet(request) => self
                .db_user_get(event, request)
                .map(|response| response.map(HostApiValue::DbUserGet)),
            HostApiRequest::DbUserPatch(request) => self
                .db_user_patch(event, request)
                .map(|response| response.map(HostApiValue::DbUserPatch)),
            HostApiRequest::DbUserIncr(request) => self
                .db_user_incr(event, request)
                .map(|response| response.map(HostApiValue::DbUserIncr)),
            HostApiRequest::DbKvGet(request) => self
                .db_kv_get(event, request)
                .map(|response| response.map(HostApiValue::DbKvGet)),
            HostApiRequest::DbKvSet(request) => self
                .db_kv_set(event, request)
                .map(|response| response.map(HostApiValue::DbKvSet)),
            HostApiRequest::UnitStatus(request) => self
                .unit_status(event, request)
                .map(|response| response.map(HostApiValue::UnitStatus)),
        }
    }

    pub fn ctx_current(
        &self,
        event: &EventContext,
    ) -> Result<HostApiResponse<CtxCurrentValue>, HostApiError> {
        validate_event(event, HostApiOperation::CtxCurrent)?;

        Ok(self.response(
            HostApiOperation::CtxCurrent,
            CtxCurrentValue {
                event: event.clone(),
            },
        ))
    }

    pub fn ctx_resolve_target(
        &self,
        event: &EventContext,
        request: CtxResolveTargetRequest,
    ) -> Result<HostApiResponse<ResolvedTarget>, HostApiError> {
        validate_event(event, HostApiOperation::CtxResolveTarget)?;

        let positional = request
            .positional
            .as_deref()
            .map(|value| {
                self.target_parser.parse(value).map_err(|source| {
                    HostApiError::parse(
                        HostApiOperation::CtxResolveTarget,
                        HostApiErrorDetail::InvalidTarget {
                            value: value.to_owned(),
                            source,
                        },
                    )
                })
            })
            .transpose()?;
        let selector_flag = request
            .selector_flag
            .as_deref()
            .map(|value| {
                self.target_parser.parse(value).map_err(|source| {
                    HostApiError::parse(
                        HostApiOperation::CtxResolveTarget,
                        HostApiErrorDetail::InvalidTarget {
                            value: value.to_owned(),
                            source,
                        },
                    )
                })
            })
            .transpose()?;
        let resolved = resolve_target(positional, selector_flag, event, |_| {
            request.implicit.clone()
        })
        .ok_or_else(|| {
            HostApiError::validation(
                HostApiOperation::CtxResolveTarget,
                HostApiErrorDetail::NoResolvableTarget,
            )
        })?;

        Ok(self.response(HostApiOperation::CtxResolveTarget, resolved))
    }

    pub fn ctx_parse_duration(
        &self,
        event: &EventContext,
        request: CtxParseDurationRequest,
    ) -> Result<HostApiResponse<ParsedDuration>, HostApiError> {
        validate_event(event, HostApiOperation::CtxParseDuration)?;

        let parsed = self
            .duration_parser
            .parse(request.input.trim())
            .map_err(|source| {
                HostApiError::parse(
                    HostApiOperation::CtxParseDuration,
                    HostApiErrorDetail::InvalidDuration {
                        value: request.input,
                        source,
                    },
                )
            })?;

        Ok(self.response(HostApiOperation::CtxParseDuration, parsed))
    }

    pub fn ctx_expand_reason(
        &self,
        event: &EventContext,
        request: CtxExpandReasonRequest,
    ) -> Result<HostApiResponse<ExpandedReason>, HostApiError> {
        validate_event(event, HostApiOperation::CtxExpandReason)?;

        let expanded = self
            .aliases
            .expand_reason(Some(&request.reason))
            .ok_or_else(|| {
                HostApiError::internal(
                    HostApiOperation::CtxExpandReason,
                    HostApiErrorDetail::ReasonExpansionUnavailable,
                )
            })?;

        Ok(self.response(HostApiOperation::CtxExpandReason, expanded))
    }

    pub fn db_user_get(
        &self,
        event: &EventContext,
        request: DbUserGetRequest,
    ) -> Result<HostApiResponse<DbUserGetValue>, HostApiError> {
        validate_event(event, HostApiOperation::DbUserGet)?;
        validate_user_id(request.user_id, HostApiOperation::DbUserGet)?;

        let user = self
            .storage(HostApiOperation::DbUserGet)?
            .get_user(request.user_id)
            .map_err(|source| storage_error(HostApiOperation::DbUserGet, source))?;

        Ok(self.response(HostApiOperation::DbUserGet, DbUserGetValue { user }))
    }

    pub fn db_user_patch(
        &self,
        event: &EventContext,
        request: DbUserPatchRequest,
    ) -> Result<HostApiResponse<DbUserPatchValue>, HostApiError> {
        validate_event(event, HostApiOperation::DbUserPatch)?;
        validate_user_patch(&request.patch, HostApiOperation::DbUserPatch)?;

        let storage = self.storage(HostApiOperation::DbUserPatch)?;
        let current = storage
            .get_user(request.patch.user_id)
            .map_err(|source| storage_error(HostApiOperation::DbUserPatch, source))?;
        let predicted = apply_user_patch(current.as_ref(), &request.patch);

        if !self.dry_run {
            storage
                .upsert_user(&request.patch)
                .map_err(|source| storage_error(HostApiOperation::DbUserPatch, source))?;
        }

        Ok(self.response(
            HostApiOperation::DbUserPatch,
            DbUserPatchValue { user: predicted },
        ))
    }

    pub fn db_user_incr(
        &self,
        event: &EventContext,
        request: DbUserIncrRequest,
    ) -> Result<HostApiResponse<DbUserIncrValue>, HostApiError> {
        validate_event(event, HostApiOperation::DbUserIncr)?;
        validate_user_incr_request(&request, HostApiOperation::DbUserIncr)?;

        let storage = self.storage(HostApiOperation::DbUserIncr)?;
        let current = storage
            .get_user(request.user_id)
            .map_err(|source| storage_error(HostApiOperation::DbUserIncr, source))?;
        let patch =
            user_patch_from_increment(current.as_ref(), &request, HostApiOperation::DbUserIncr)?;
        let predicted = apply_user_patch(current.as_ref(), &patch);

        if !self.dry_run {
            storage
                .upsert_user(&patch)
                .map_err(|source| storage_error(HostApiOperation::DbUserIncr, source))?;
        }

        Ok(self.response(
            HostApiOperation::DbUserIncr,
            DbUserIncrValue { user: predicted },
        ))
    }

    pub fn db_kv_get(
        &self,
        event: &EventContext,
        request: DbKvGetRequest,
    ) -> Result<HostApiResponse<DbKvGetValue>, HostApiError> {
        validate_event(event, HostApiOperation::DbKvGet)?;
        validate_kv_key(
            &request.scope_kind,
            &request.scope_id,
            &request.key,
            HostApiOperation::DbKvGet,
        )?;

        let entry = self
            .storage(HostApiOperation::DbKvGet)?
            .get_kv(&request.scope_kind, &request.scope_id, &request.key)
            .map_err(|source| storage_error(HostApiOperation::DbKvGet, source))?;

        Ok(self.response(HostApiOperation::DbKvGet, DbKvGetValue { entry }))
    }

    pub fn db_kv_set(
        &self,
        event: &EventContext,
        request: DbKvSetRequest,
    ) -> Result<HostApiResponse<DbKvSetValue>, HostApiError> {
        validate_event(event, HostApiOperation::DbKvSet)?;
        validate_kv_entry(&request.entry, HostApiOperation::DbKvSet)?;

        if !self.dry_run {
            self.storage(HostApiOperation::DbKvSet)?
                .set_kv(&request.entry)
                .map_err(|source| storage_error(HostApiOperation::DbKvSet, source))?;
        }

        Ok(self.response(
            HostApiOperation::DbKvSet,
            DbKvSetValue {
                entry: request.entry,
            },
        ))
    }

    pub fn unit_status(
        &self,
        event: &EventContext,
        request: UnitStatusRequest,
    ) -> Result<HostApiResponse<UnitStatusValue>, HostApiError> {
        validate_event(event, HostApiOperation::UnitStatus)?;
        if let Some(unit_id) = request.unit_id.as_deref() {
            validate_non_empty(unit_id, "unit_id", HostApiOperation::UnitStatus)?;
        }

        let registry = self.unit_registry(HostApiOperation::UnitStatus)?;
        let summary = registry.status_summary();
        let unit = if let Some(unit_id) = request.unit_id.clone() {
            let descriptor = registry.get(&unit_id).ok_or_else(|| {
                HostApiError::validation(
                    HostApiOperation::UnitStatus,
                    HostApiErrorDetail::UnknownUnit {
                        unit_id: unit_id.clone(),
                    },
                )
            })?;
            Some(UnitStatusEntry::from_descriptor(descriptor))
        } else {
            None
        };

        Ok(self.response(
            HostApiOperation::UnitStatus,
            UnitStatusValue {
                requested_unit_id: request.unit_id,
                summary,
                unit,
            },
        ))
    }

    fn storage(&self, operation: HostApiOperation) -> Result<&StorageConnection, HostApiError> {
        self.storage.as_deref().ok_or_else(|| {
            HostApiError::internal(
                operation,
                HostApiErrorDetail::ResourceUnavailable {
                    resource: "storage".to_owned(),
                },
            )
        })
    }

    fn unit_registry(&self, operation: HostApiOperation) -> Result<&UnitRegistry, HostApiError> {
        self.unit_registry.as_deref().ok_or_else(|| {
            HostApiError::internal(
                operation,
                HostApiErrorDetail::ResourceUnavailable {
                    resource: "unit_registry".to_owned(),
                },
            )
        })
    }

    fn response<T>(&self, operation: HostApiOperation, value: T) -> HostApiResponse<T> {
        HostApiResponse {
            operation,
            dry_run: self.dry_run,
            value,
        }
    }
}

fn validate_event(event: &EventContext, operation: HostApiOperation) -> Result<(), HostApiError> {
    event.validate_invariants().map_err(|source| {
        HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidEventContext {
                message: source.to_string(),
            },
        )
    })
}

fn validate_user_id(user_id: i64, operation: HostApiOperation) -> Result<(), HostApiError> {
    if user_id == 0 {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "user_id".to_owned(),
                message: "must be non-zero".to_owned(),
            },
        ));
    }

    Ok(())
}

fn validate_non_empty(
    value: &str,
    field: &'static str,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    if value.trim().is_empty() {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: field.to_owned(),
                message: "must not be blank".to_owned(),
            },
        ));
    }

    Ok(())
}

fn validate_user_patch(patch: &UserPatch, operation: HostApiOperation) -> Result<(), HostApiError> {
    validate_user_id(patch.user_id, operation)?;
    validate_non_empty(&patch.seen_at, "seen_at", operation)?;
    validate_non_empty(&patch.updated_at, "updated_at", operation)?;
    if let Some(warn_count) = patch.warn_count
        && warn_count < 0
    {
        return Err(HostApiError::validation(
            operation,
            HostApiErrorDetail::InvalidField {
                field: "warn_count".to_owned(),
                message: "must be non-negative".to_owned(),
            },
        ));
    }

    Ok(())
}

fn validate_user_incr_request(
    request: &DbUserIncrRequest,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    validate_user_id(request.user_id, operation)?;
    validate_non_empty(&request.seen_at, "seen_at", operation)?;
    validate_non_empty(&request.updated_at, "updated_at", operation)?;
    Ok(())
}

fn validate_kv_key(
    scope_kind: &str,
    scope_id: &str,
    key: &str,
    operation: HostApiOperation,
) -> Result<(), HostApiError> {
    validate_non_empty(scope_kind, "scope_kind", operation)?;
    validate_non_empty(scope_id, "scope_id", operation)?;
    validate_non_empty(key, "key", operation)?;
    Ok(())
}

fn validate_kv_entry(entry: &KvEntry, operation: HostApiOperation) -> Result<(), HostApiError> {
    validate_kv_key(&entry.scope_kind, &entry.scope_id, &entry.key, operation)?;
    validate_non_empty(&entry.value_json, "value_json", operation)?;
    validate_non_empty(&entry.updated_at, "updated_at", operation)?;
    Ok(())
}

fn storage_error(operation: HostApiOperation, source: StorageError) -> HostApiError {
    HostApiError::internal(
        operation,
        HostApiErrorDetail::StorageFailure {
            message: source.to_string(),
        },
    )
}

fn apply_user_patch(current: Option<&UserRecord>, patch: &UserPatch) -> UserRecord {
    let first_seen_at = match current {
        Some(existing) if existing.first_seen_at < patch.seen_at => existing.first_seen_at.clone(),
        _ => patch.seen_at.clone(),
    };
    let last_seen_at = match current {
        Some(existing) if existing.last_seen_at > patch.seen_at => existing.last_seen_at.clone(),
        _ => patch.seen_at.clone(),
    };

    UserRecord {
        user_id: patch.user_id,
        username: patch
            .username
            .clone()
            .or_else(|| current.and_then(|existing| existing.username.clone())),
        display_name: patch
            .display_name
            .clone()
            .or_else(|| current.and_then(|existing| existing.display_name.clone())),
        first_seen_at,
        last_seen_at,
        warn_count: patch
            .warn_count
            .unwrap_or_else(|| current.map_or(0, |existing| existing.warn_count)),
        shadowbanned: patch
            .shadowbanned
            .unwrap_or_else(|| current.is_some_and(|existing| existing.shadowbanned)),
        reputation: patch
            .reputation
            .unwrap_or_else(|| current.map_or(0, |existing| existing.reputation)),
        state_json: patch
            .state_json
            .clone()
            .or_else(|| current.and_then(|existing| existing.state_json.clone())),
        updated_at: patch.updated_at.clone(),
    }
}

fn user_patch_from_increment(
    current: Option<&UserRecord>,
    request: &DbUserIncrRequest,
    operation: HostApiOperation,
) -> Result<UserPatch, HostApiError> {
    let current_warn_count = current.map_or(0, |user| user.warn_count);
    let warn_count = current_warn_count
        .checked_add(request.warn_count_delta)
        .ok_or_else(|| {
            counter_error(
                operation,
                "warn_count",
                current_warn_count,
                request.warn_count_delta,
            )
        })?;
    if warn_count < 0 {
        return Err(counter_error(
            operation,
            "warn_count",
            current_warn_count,
            request.warn_count_delta,
        ));
    }

    let current_reputation = current.map_or(0, |user| user.reputation);
    let reputation = current_reputation
        .checked_add(request.reputation_delta)
        .ok_or_else(|| {
            counter_error(
                operation,
                "reputation",
                current_reputation,
                request.reputation_delta,
            )
        })?;

    Ok(UserPatch {
        user_id: request.user_id,
        username: request.username.clone(),
        display_name: request.display_name.clone(),
        seen_at: request.seen_at.clone(),
        warn_count: Some(warn_count),
        shadowbanned: request.shadowbanned,
        reputation: Some(reputation),
        state_json: request.state_json.clone(),
        updated_at: request.updated_at.clone(),
    })
}

fn counter_error(
    operation: HostApiOperation,
    field: &'static str,
    current: i64,
    delta: i64,
) -> HostApiError {
    HostApiError::validation(
        operation,
        HostApiErrorDetail::InvalidCounterChange {
            field: field.to_owned(),
            current,
            delta,
        },
    )
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum HostApiRequest {
    CtxCurrent,
    CtxResolveTarget(CtxResolveTargetRequest),
    CtxParseDuration(CtxParseDurationRequest),
    CtxExpandReason(CtxExpandReasonRequest),
    DbUserGet(DbUserGetRequest),
    DbUserPatch(DbUserPatchRequest),
    DbUserIncr(DbUserIncrRequest),
    DbKvGet(DbKvGetRequest),
    DbKvSet(DbKvSetRequest),
    UnitStatus(UnitStatusRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostApiValue {
    CtxCurrent(Box<CtxCurrentValue>),
    ResolvedTarget(ResolvedTarget),
    ParsedDuration(ParsedDuration),
    ExpandedReason(ExpandedReason),
    DbUserGet(DbUserGetValue),
    DbUserPatch(DbUserPatchValue),
    DbUserIncr(DbUserIncrValue),
    DbKvGet(DbKvGetValue),
    DbKvSet(DbKvSetValue),
    UnitStatus(UnitStatusValue),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostApiOperation {
    CtxCurrent,
    CtxResolveTarget,
    CtxParseDuration,
    CtxExpandReason,
    DbUserGet,
    DbUserPatch,
    DbUserIncr,
    DbKvGet,
    DbKvSet,
    UnitStatus,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostApiResponse<T> {
    pub operation: HostApiOperation,
    pub dry_run: bool,
    pub value: T,
}

impl<T> HostApiResponse<T> {
    fn map<U>(self, map: impl FnOnce(T) -> U) -> HostApiResponse<U> {
        HostApiResponse {
            operation: self.operation,
            dry_run: self.dry_run,
            value: map(self.value),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtxCurrentValue {
    pub event: EventContext,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxResolveTargetRequest {
    pub positional: Option<String>,
    pub selector_flag: Option<String>,
    pub implicit: Option<ParsedTargetSelector>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxParseDurationRequest {
    pub input: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxExpandReasonRequest {
    pub reason: ReasonExpr,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserGetRequest {
    pub user_id: i64,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserPatchRequest {
    pub patch: UserPatch,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserIncrRequest {
    pub user_id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub seen_at: String,
    pub updated_at: String,
    pub warn_count_delta: i64,
    pub reputation_delta: i64,
    pub shadowbanned: Option<bool>,
    pub state_json: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvGetRequest {
    pub scope_kind: String,
    pub scope_id: String,
    pub key: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvSetRequest {
    pub entry: KvEntry,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnitStatusRequest {
    pub unit_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserGetValue {
    pub user: Option<UserRecord>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserPatchValue {
    pub user: UserRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbUserIncrValue {
    pub user: UserRecord,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvGetValue {
    pub entry: Option<KvEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DbKvSetValue {
    pub entry: KvEntry,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnitStatusValue {
    pub requested_unit_id: Option<String>,
    pub summary: UnitRegistryStatus,
    pub unit: Option<UnitStatusEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnitStatusEntry {
    pub unit_id: String,
    pub status: UnitStatus,
    pub enabled: Option<bool>,
    pub diagnostics: Vec<UnitDiagnostic>,
}

impl UnitStatusEntry {
    fn from_descriptor(descriptor: &crate::unit::UnitDescriptor) -> Self {
        Self {
            unit_id: descriptor.id.clone(),
            status: descriptor.status,
            enabled: descriptor
                .manifest
                .as_ref()
                .map(|manifest| manifest.unit.enabled),
            diagnostics: descriptor.diagnostics.clone(),
        }
    }
}

#[derive(Debug, Clone, Error, Eq, PartialEq, Serialize, Deserialize)]
#[error("{kind:?} host api error in {operation:?}: {detail}")]
pub struct HostApiError {
    pub operation: HostApiOperation,
    pub kind: HostApiErrorKind,
    pub detail: HostApiErrorDetail,
}

impl HostApiError {
    fn validation(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Validation,
            detail,
        }
    }

    fn parse(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
        Self {
            operation,
            kind: HostApiErrorKind::Parse,
            detail,
        }
    }

    fn internal(operation: HostApiOperation, detail: HostApiErrorDetail) -> Self {
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
    #[error("unknown unit `{unit_id}`")]
    UnknownUnit { unit_id: String },
    #[error("required host resource `{resource}` is unavailable")]
    ResourceUnavailable { resource: String },
    #[error("storage failure: {message}")]
    StorageFailure { message: String },
    #[error("reason expansion unexpectedly returned no result")]
    ReasonExpansionUnavailable,
}

#[cfg(test)]
mod tests {
    use super::{
        CtxExpandReasonRequest, CtxParseDurationRequest, CtxResolveTargetRequest, DbKvGetRequest,
        DbKvSetRequest, DbUserGetRequest, DbUserIncrRequest, DbUserPatchRequest, HostApi,
        HostApiError, HostApiErrorDetail, HostApiErrorKind, HostApiOperation, HostApiRequest,
        HostApiValue, UnitStatusEntry, UnitStatusRequest,
    };
    use crate::event::{
        ChatContext, EventContext, EventNormalizer, ExecutionMode, ManualInvocationInput,
        ReplyContext, SystemContext, SystemOrigin, UnitContext, UpdateType,
    };
    use crate::parser::command::ReasonExpr;
    use crate::parser::duration::{DurationParseError, DurationUnit, ParsedDuration};
    use crate::parser::reason::{ExpandedReason, ReasonAliasDefinition, ReasonAliasRegistry};
    use crate::parser::target::{ParsedTargetSelector, TargetParseError, TargetSource};
    use crate::storage::{KvEntry, Storage, UserPatch};
    use crate::unit::{
        ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry, UnitStatus,
    };
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0)
            .single()
            .expect("valid timestamp")
    }

    fn manual_event() -> EventContext {
        let normalizer = EventNormalizer::new();
        let mut input = ManualInvocationInput::new(
            UnitContext::new("moderation.test").with_trigger("manual"),
            "/warn @spam spam",
        );
        input.event_id = Some("evt_host_api_manual".to_owned());
        input.received_at = ts();
        input.chat = Some(ChatContext {
            id: -100123,
            chat_type: "supergroup".to_owned(),
            title: Some("Moderation HQ".to_owned()),
            username: Some("mod_hq".to_owned()),
            thread_id: Some(7),
        });
        input.reply = Some(ReplyContext {
            message_id: 99,
            sender_user_id: Some(77),
            sender_username: Some("reply_user".to_owned()),
            text: Some("reply".to_owned()),
            has_media: false,
        });

        normalizer
            .normalize_manual(input)
            .expect("manual event normalizes")
    }

    fn storage_api() -> (TempDir, HostApi) {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = dir.path().join("host-api.sqlite3");
        let storage = Storage::new(path)
            .init()
            .unwrap_or_else(|error| panic!("storage init failed: {error}"));
        (dir, HostApi::new(false).with_storage(storage))
    }

    fn dry_run_storage_api() -> (TempDir, HostApi) {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = dir.path().join("host-api.sqlite3");
        let storage = Storage::new(path)
            .init()
            .unwrap_or_else(|error| panic!("storage init failed: {error}"));
        (dir, HostApi::new(true).with_storage(storage))
    }

    fn unit_registry_api() -> HostApi {
        let active = UnitManifest::new(
            UnitDefinition::new("moderation.warn"),
            TriggerSpec::command(["warn"]),
            ServiceSpec::new("cargo run"),
        );
        let mut disabled = UnitManifest::new(
            UnitDefinition::new("moderation.mute"),
            TriggerSpec::command(["mute"]),
            ServiceSpec::new("cargo run"),
        );
        disabled.unit.enabled = false;

        let report = UnitRegistry::load_manifests(vec![active, disabled]);
        assert!(report.is_fully_valid());

        HostApi::new(false).with_unit_registry(report.registry)
    }

    #[test]
    fn ctx_current_returns_cloned_event_with_operation_metadata() {
        let event = manual_event();
        let api = HostApi::new(false);

        let response = api.ctx_current(&event).expect("ctx.current succeeds");

        assert_eq!(response.operation, HostApiOperation::CtxCurrent);
        assert!(!response.dry_run);
        assert_eq!(response.value.event.event_id, event.event_id);
        assert_eq!(response.value.event.execution_mode, ExecutionMode::Manual);
    }

    #[test]
    fn call_surface_routes_ctx_current_request() {
        let event = manual_event();
        let api = HostApi::new(false);

        let response = api
            .call(&event, HostApiRequest::CtxCurrent)
            .expect("typed call succeeds");

        assert_eq!(response.operation, HostApiOperation::CtxCurrent);
        assert!(!response.dry_run);
        match response.value {
            HostApiValue::CtxCurrent(value) => assert_eq!(value.event.event_id, event.event_id),
            other => panic!("unexpected host api value: {other:?}"),
        }
    }

    #[test]
    fn ctx_resolve_target_uses_parser_and_reply_fallback() {
        let event = manual_event();
        let api = HostApi::new(false);

        let explicit = api
            .ctx_resolve_target(
                &event,
                CtxResolveTargetRequest {
                    positional: Some("@spam_user".to_owned()),
                    selector_flag: None,
                    implicit: None,
                },
            )
            .expect("explicit target resolves");
        assert_eq!(explicit.value.source, TargetSource::ExplicitPositional);
        assert_eq!(
            explicit.value.selector,
            ParsedTargetSelector::Username {
                username: "spam_user".to_owned(),
            }
        );

        let reply = api
            .ctx_resolve_target(
                &event,
                CtxResolveTargetRequest {
                    positional: None,
                    selector_flag: None,
                    implicit: None,
                },
            )
            .expect("reply fallback resolves");
        assert_eq!(reply.value.source, TargetSource::ReplyContext);
        assert_eq!(
            reply.value.selector,
            ParsedTargetSelector::UserId { user_id: 77 }
        );
    }

    #[test]
    fn ctx_resolve_target_returns_structured_parse_error() {
        let event = manual_event();
        let api = HostApi::new(false);

        let error = api
            .ctx_resolve_target(
                &event,
                CtxResolveTargetRequest {
                    positional: Some("@bad-name".to_owned()),
                    selector_flag: None,
                    implicit: None,
                },
            )
            .expect_err("invalid target must fail");

        assert_eq!(error.kind, HostApiErrorKind::Parse);
        assert_eq!(error.operation, HostApiOperation::CtxResolveTarget);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidTarget {
                value: "@bad-name".to_owned(),
                source: TargetParseError::InvalidUsername("@bad-name".to_owned()),
            }
        );
    }

    #[test]
    fn ctx_parse_duration_returns_typed_value() {
        let event = manual_event();
        let api = HostApi::new(false);

        let response = api
            .ctx_parse_duration(
                &event,
                CtxParseDurationRequest {
                    input: "15m".to_owned(),
                },
            )
            .expect("duration parses");

        assert_eq!(response.operation, HostApiOperation::CtxParseDuration);
        assert_eq!(
            response.value,
            ParsedDuration {
                value: 15,
                unit: DurationUnit::Minutes,
            }
        );
    }

    #[test]
    fn ctx_parse_duration_returns_structured_error() {
        let event = manual_event();
        let api = HostApi::new(false);

        let error = api
            .ctx_parse_duration(
                &event,
                CtxParseDurationRequest {
                    input: "30".to_owned(),
                },
            )
            .expect_err("missing unit must fail");

        assert_eq!(error.kind, HostApiErrorKind::Parse);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidDuration {
                value: "30".to_owned(),
                source: DurationParseError::MissingUnit,
            }
        );
    }

    #[test]
    fn ctx_expand_reason_uses_alias_registry() {
        let event = manual_event();
        let mut aliases = ReasonAliasRegistry::new();
        aliases.insert(
            "spam",
            ReasonAliasDefinition::new("spam or scam promotion")
                .with_rule_code("2.8")
                .with_title("Spam"),
        );
        let api = HostApi::new(false).with_reason_aliases(aliases);

        let response = api
            .ctx_expand_reason(
                &event,
                CtxExpandReasonRequest {
                    reason: ReasonExpr::Alias("spam".to_owned()),
                },
            )
            .expect("reason expands");

        assert_eq!(response.operation, HostApiOperation::CtxExpandReason);
        assert_eq!(
            response.value,
            ExpandedReason::Alias {
                alias: "spam".to_owned(),
                definition: ReasonAliasDefinition {
                    canonical: "spam or scam promotion".to_owned(),
                    rule_code: Some("2.8".to_owned()),
                    title: Some("Spam".to_owned()),
                },
            }
        );
    }

    #[test]
    fn db_user_get_returns_typed_user_value() {
        let event = manual_event();
        let (_dir, api) = storage_api();
        api.storage(HostApiOperation::DbUserGet)
            .expect("storage")
            .upsert_user(&UserPatch {
                user_id: 77,
                username: Some("reply_user".to_owned()),
                display_name: Some("Reply User".to_owned()),
                seen_at: "2026-04-21T12:00:00Z".to_owned(),
                warn_count: Some(1),
                shadowbanned: Some(false),
                reputation: Some(4),
                state_json: Some("{\"state\":\"ok\"}".to_owned()),
                updated_at: "2026-04-21T12:00:00Z".to_owned(),
            })
            .expect("seed user");

        let response = api
            .db_user_get(&event, DbUserGetRequest { user_id: 77 })
            .expect("db.user_get succeeds");

        assert_eq!(response.operation, HostApiOperation::DbUserGet);
        assert_eq!(
            response
                .value
                .user
                .expect("user exists")
                .username
                .as_deref(),
            Some("reply_user")
        );
    }

    #[test]
    fn db_user_patch_dry_run_validates_without_mutation() {
        let event = manual_event();
        let (_dir, api) = dry_run_storage_api();

        let response = api
            .db_user_patch(
                &event,
                DbUserPatchRequest {
                    patch: UserPatch {
                        user_id: 77,
                        username: Some("dry_run_user".to_owned()),
                        display_name: Some("Dry Run".to_owned()),
                        seen_at: "2026-04-21T12:05:00Z".to_owned(),
                        warn_count: Some(2),
                        shadowbanned: Some(true),
                        reputation: Some(5),
                        state_json: Some("{\"mode\":\"dry\"}".to_owned()),
                        updated_at: "2026-04-21T12:05:00Z".to_owned(),
                    },
                },
            )
            .expect("dry-run patch succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.user.warn_count, 2);
        assert!(
            api.storage(HostApiOperation::DbUserPatch)
                .expect("storage")
                .get_user(77)
                .expect("query succeeds")
                .is_none()
        );
    }

    #[test]
    fn db_user_patch_returns_structured_validation_error() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .db_user_patch(
                &event,
                DbUserPatchRequest {
                    patch: UserPatch {
                        user_id: 0,
                        username: None,
                        display_name: None,
                        seen_at: "".to_owned(),
                        warn_count: Some(-1),
                        shadowbanned: None,
                        reputation: None,
                        state_json: None,
                        updated_at: "".to_owned(),
                    },
                },
            )
            .expect_err("invalid patch must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::DbUserPatch);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidField {
                field: "user_id".to_owned(),
                message: "must be non-zero".to_owned(),
            }
        );
    }

    #[test]
    fn db_user_incr_updates_existing_user() {
        let event = manual_event();
        let (_dir, api) = storage_api();
        api.storage(HostApiOperation::DbUserIncr)
            .expect("storage")
            .upsert_user(&UserPatch {
                user_id: 77,
                username: Some("reply_user".to_owned()),
                display_name: Some("Reply User".to_owned()),
                seen_at: "2026-04-21T12:00:00Z".to_owned(),
                warn_count: Some(1),
                shadowbanned: Some(false),
                reputation: Some(4),
                state_json: None,
                updated_at: "2026-04-21T12:00:00Z".to_owned(),
            })
            .expect("seed user");

        let response = api
            .db_user_incr(
                &event,
                DbUserIncrRequest {
                    user_id: 77,
                    username: None,
                    display_name: Some("Reply User Updated".to_owned()),
                    seen_at: "2026-04-21T12:10:00Z".to_owned(),
                    updated_at: "2026-04-21T12:10:00Z".to_owned(),
                    warn_count_delta: 2,
                    reputation_delta: -1,
                    shadowbanned: Some(true),
                    state_json: Some("{\"escalated\":true}".to_owned()),
                },
            )
            .expect("increment succeeds");

        assert_eq!(response.value.user.warn_count, 3);
        assert_eq!(response.value.user.reputation, 3);
        assert!(response.value.user.shadowbanned);
        assert_eq!(
            api.storage(HostApiOperation::DbUserIncr)
                .expect("storage")
                .get_user(77)
                .expect("query succeeds")
                .expect("user exists")
                .warn_count,
            3
        );
    }

    #[test]
    fn db_user_incr_returns_structured_counter_error() {
        let event = manual_event();
        let (_dir, api) = storage_api();

        let error = api
            .db_user_incr(
                &event,
                DbUserIncrRequest {
                    user_id: 77,
                    username: None,
                    display_name: None,
                    seen_at: "2026-04-21T12:10:00Z".to_owned(),
                    updated_at: "2026-04-21T12:10:00Z".to_owned(),
                    warn_count_delta: -1,
                    reputation_delta: 0,
                    shadowbanned: None,
                    state_json: None,
                },
            )
            .expect_err("negative increment from zero must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::InvalidCounterChange {
                field: "warn_count".to_owned(),
                current: 0,
                delta: -1,
            }
        );
    }

    #[test]
    fn db_kv_set_dry_run_does_not_mutate_storage() {
        let event = manual_event();
        let (_dir, api) = dry_run_storage_api();

        let response = api
            .db_kv_set(
                &event,
                DbKvSetRequest {
                    entry: KvEntry {
                        scope_kind: "chat".to_owned(),
                        scope_id: "-100123".to_owned(),
                        key: "policy".to_owned(),
                        value_json: "{\"mode\":\"strict\"}".to_owned(),
                        updated_at: "2026-04-21T12:00:00Z".to_owned(),
                    },
                },
            )
            .expect("dry-run kv set succeeds");

        assert!(response.dry_run);
        assert_eq!(response.value.entry.key, "policy");
        assert!(
            api.storage(HostApiOperation::DbKvSet)
                .expect("storage")
                .get_kv("chat", "-100123", "policy")
                .expect("query succeeds")
                .is_none()
        );
    }

    #[test]
    fn db_kv_get_returns_seeded_entry() {
        let event = manual_event();
        let (_dir, api) = storage_api();
        api.storage(HostApiOperation::DbKvGet)
            .expect("storage")
            .set_kv(&KvEntry {
                scope_kind: "chat".to_owned(),
                scope_id: "-100123".to_owned(),
                key: "policy".to_owned(),
                value_json: "{\"mode\":\"strict\"}".to_owned(),
                updated_at: "2026-04-21T12:00:00Z".to_owned(),
            })
            .expect("seed kv");

        let response = api
            .db_kv_get(
                &event,
                DbKvGetRequest {
                    scope_kind: "chat".to_owned(),
                    scope_id: "-100123".to_owned(),
                    key: "policy".to_owned(),
                },
            )
            .expect("kv get succeeds");

        assert_eq!(
            response.value.entry.expect("entry exists").value_json,
            "{\"mode\":\"strict\"}"
        );
    }

    #[test]
    fn unit_status_returns_summary_and_specific_entry() {
        let event = manual_event();
        let api = unit_registry_api();

        let response = api
            .unit_status(
                &event,
                UnitStatusRequest {
                    unit_id: Some("moderation.warn".to_owned()),
                },
            )
            .expect("unit status succeeds");

        assert_eq!(response.operation, HostApiOperation::UnitStatus);
        assert_eq!(response.value.summary.total_units, 2);
        assert_eq!(response.value.summary.active_units, 1);
        assert_eq!(response.value.summary.disabled_units, 1);
        assert_eq!(
            response.value.unit,
            Some(UnitStatusEntry {
                unit_id: "moderation.warn".to_owned(),
                status: UnitStatus::Active,
                enabled: Some(true),
                diagnostics: Vec::new(),
            })
        );
    }

    #[test]
    fn unit_status_returns_structured_not_found_error() {
        let event = manual_event();
        let api = unit_registry_api();

        let error = api
            .unit_status(
                &event,
                UnitStatusRequest {
                    unit_id: Some("missing.unit".to_owned()),
                },
            )
            .expect_err("unknown unit must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(
            error.detail,
            HostApiErrorDetail::UnknownUnit {
                unit_id: "missing.unit".to_owned(),
            }
        );
    }

    #[test]
    fn call_surface_routes_db_and_unit_requests() {
        let event = manual_event();
        let api = unit_registry_api();

        let response = api
            .call(
                &event,
                HostApiRequest::UnitStatus(UnitStatusRequest { unit_id: None }),
            )
            .expect("typed call succeeds");

        match response.value {
            HostApiValue::UnitStatus(value) => assert_eq!(value.summary.total_units, 2),
            other => panic!("unexpected host api value: {other:?}"),
        }
    }

    #[test]
    fn dry_run_is_preserved_in_ctx_responses() {
        let event = manual_event();
        let api = HostApi::new(true);

        let response = api
            .ctx_parse_duration(
                &event,
                CtxParseDurationRequest {
                    input: "1h".to_owned(),
                },
            )
            .expect("ctx op still succeeds in dry run");

        assert!(response.dry_run);
        assert_eq!(response.operation, HostApiOperation::CtxParseDuration);
    }

    #[test]
    fn invalid_event_maps_to_validation_error() {
        let mut event = EventContext::new(
            "evt_invalid",
            UpdateType::Message,
            ExecutionMode::Realtime,
            SystemContext::synthetic(SystemOrigin::Manual),
        );
        event.message = None;

        let api = HostApi::new(false);
        let error = api
            .ctx_current(&event)
            .expect_err("invalid event must fail");

        assert_eq!(error.kind, HostApiErrorKind::Validation);
        assert_eq!(error.operation, HostApiOperation::CtxCurrent);
        assert!(
            matches!(
                error,
                HostApiError {
                    detail: HostApiErrorDetail::InvalidEventContext { .. },
                    ..
                }
            ),
            "unexpected error shape: {error:?}"
        );
    }
}
