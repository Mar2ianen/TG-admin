use super::types::*;

pub fn validate_request(request: &TelegramRequest) -> Result<(), TelegramError> {
    let operation = request.operation();
    if operation.requires_idempotency() && request.idempotency_key().is_none() {
        return Err(TelegramError::new(
            operation,
            TelegramErrorKind::Validation,
            "idempotency key is required for this operation",
        )
        .with_details(serde_json::json!({
            "field": "idempotency_key",
        })));
    }

    match request {
        TelegramRequest::SendUi(request) => {
            validate_chat_id(operation, request.chat_id)?;
            if request.template.trim().is_empty() {
                return Err(validation_error(
                    operation,
                    "template",
                    "template must not be empty",
                ));
            }
        }
        TelegramRequest::SendMessage(request) => {
            validate_chat_id(operation, request.chat_id)?;
            if request.text.trim().is_empty() {
                return Err(validation_error(
                    operation,
                    "text",
                    "text must not be empty",
                ));
            }
        }
        TelegramRequest::EditUi(request) => {
            validate_chat_id(operation, request.chat_id)?;
            if request.template.trim().is_empty() {
                return Err(validation_error(
                    operation,
                    "template",
                    "template must not be empty",
                ));
            }
            if request.message_id <= 0 {
                return Err(validation_error(
                    operation,
                    "message_id",
                    "message_id must be positive",
                ));
            }
        }
        TelegramRequest::Delete(request) => {
            validate_chat_id(operation, request.chat_id)?;
            if request.message_id <= 0 {
                return Err(validation_error(
                    operation,
                    "message_id",
                    "message_id must be positive",
                ));
            }
        }
        TelegramRequest::DeleteMany(request) => {
            validate_chat_id(operation, request.chat_id)?;
            if request.message_ids.is_empty() {
                return Err(validation_error(
                    operation,
                    "message_ids",
                    "message_ids must not be empty",
                ));
            }
            if request
                .message_ids
                .iter()
                .any(|message_id| *message_id <= 0)
            {
                return Err(validation_error(
                    operation,
                    "message_ids",
                    "message_ids must contain only positive ids",
                ));
            }
        }
        TelegramRequest::Restrict(request) => {
            validate_chat_id(operation, request.chat_id)?;
            if request.user_id <= 0 {
                return Err(validation_error(
                    operation,
                    "user_id",
                    "user_id must be positive",
                ));
            }
        }
        TelegramRequest::Unrestrict(request) => {
            validate_chat_id(operation, request.chat_id)?;
            if request.user_id <= 0 {
                return Err(validation_error(
                    operation,
                    "user_id",
                    "user_id must be positive",
                ));
            }
        }
        TelegramRequest::Ban(request) => {
            validate_chat_id(operation, request.chat_id)?;
            if request.user_id <= 0 {
                return Err(validation_error(
                    operation,
                    "user_id",
                    "user_id must be positive",
                ));
            }
        }
        TelegramRequest::Unban(request) => {
            validate_chat_id(operation, request.chat_id)?;
            if request.user_id <= 0 {
                return Err(validation_error(
                    operation,
                    "user_id",
                    "user_id must be positive",
                ));
            }
        }
        TelegramRequest::GetChatAdministrators(request) => {
            validate_chat_id(operation, request.chat_id)?;
        }
        TelegramRequest::GetChatMember(request) => {
            validate_chat_id(operation, request.chat_id)?;
            if request.user_id <= 0 {
                return Err(validation_error(
                    operation,
                    "user_id",
                    "user_id must be positive",
                ));
            }
        }
        TelegramRequest::AnswerCallback(request) => {
            if request.callback_query_id.trim().is_empty() {
                return Err(validation_error(
                    operation,
                    "callback_query_id",
                    "callback_query_id must not be empty",
                ));
            }
        }
    }

    Ok(())
}

pub fn validate_chat_id(
    operation: TelegramOperation,
    chat_id: ChatId,
) -> Result<(), TelegramError> {
    if chat_id == 0 {
        return Err(validation_error(
            operation,
            "chat_id",
            "chat_id must not be zero",
        ));
    }

    Ok(())
}

pub fn validation_error(
    operation: TelegramOperation,
    field: &'static str,
    message: &'static str,
) -> TelegramError {
    TelegramError::new(operation, TelegramErrorKind::Validation, message).with_details(
        serde_json::json!({
            "field": field,
        }),
    )
}
