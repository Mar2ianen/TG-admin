use super::{
    DbKvGetRequest, DbKvGetValue, DbKvSetRequest, DbKvSetValue, DbUserGetRequest, DbUserGetValue,
    DbUserIncrRequest, DbUserIncrValue, DbUserPatchRequest, DbUserPatchValue, HostApi,
    HostApiError, HostApiOperation, HostApiResponse, apply_user_patch, storage_error,
    user_patch_from_increment, validate_event, validate_kv_entry, validate_kv_key,
    validate_user_id, validate_user_incr_request, validate_user_patch,
};
use crate::event::EventContext;

impl HostApi {
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
}
