use super::{
    storage_error, validate_event,
    validation::{validate_msg_by_user_request, validate_msg_window_request},
    HostApi, HostApiError, HostApiOperation, HostApiResponse, MsgByUserRequest, MsgByUserValue,
    MsgWindowRequest, MsgWindowValue,
};
use crate::event::EventContext;

impl HostApi {
    pub fn msg_window(
        &self,
        event: &EventContext,
        request: MsgWindowRequest,
    ) -> Result<HostApiResponse<MsgWindowValue>, HostApiError> {
        validate_event(event, HostApiOperation::MsgWindow)?;
        self.require_operation_capability(event, HostApiOperation::MsgWindow)?;
        validate_msg_window_request(&request, HostApiOperation::MsgWindow)?;

        let messages = self
            .storage(HostApiOperation::MsgWindow)?
            .message_window(
                request.chat_id,
                request.anchor_message_id,
                request.up,
                request.down,
                request.include_anchor,
            )
            .map_err(|source| storage_error(HostApiOperation::MsgWindow, source))?;

        Ok(self.response(HostApiOperation::MsgWindow, MsgWindowValue { messages }))
    }

    pub fn msg_by_user(
        &self,
        event: &EventContext,
        request: MsgByUserRequest,
    ) -> Result<HostApiResponse<MsgByUserValue>, HostApiError> {
        validate_event(event, HostApiOperation::MsgByUser)?;
        self.require_operation_capability(event, HostApiOperation::MsgByUser)?;
        validate_msg_by_user_request(&request, HostApiOperation::MsgByUser)?;

        let messages = self
            .storage(HostApiOperation::MsgByUser)?
            .messages_by_user(
                request.chat_id,
                request.user_id,
                &request.since,
                request.limit,
            )
            .map_err(|source| storage_error(HostApiOperation::MsgByUser, source))?;

        Ok(self.response(HostApiOperation::MsgByUser, MsgByUserValue { messages }))
    }
}
