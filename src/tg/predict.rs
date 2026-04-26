use super::types::*;

pub fn predict_result(request: &TelegramRequest) -> TelegramResult {
    match request {
        TelegramRequest::SendUi(request) => TelegramResult::Ui(TelegramUiResult {
            chat_id: request.chat_id,
            message_id: request.reply_to_message_id.unwrap_or(0).saturating_add(1),
            template: request.template.clone(),
            edited: false,
            raw_passthrough: false,
        }),
        TelegramRequest::SendMessage(request) => TelegramResult::Message(TelegramMessageResult {
            chat_id: request.chat_id,
            message_id: request.reply_to_message_id.unwrap_or(0).saturating_add(1),
            raw_passthrough: false,
        }),
        TelegramRequest::EditUi(request) => TelegramResult::Ui(TelegramUiResult {
            chat_id: request.chat_id,
            message_id: request.message_id,
            template: request.template.clone(),
            edited: true,
            raw_passthrough: false,
        }),
        TelegramRequest::Delete(request) => TelegramResult::Delete(TelegramDeleteResult {
            chat_id: request.chat_id,
            deleted: vec![request.message_id],
            failed: Vec::new(),
        }),
        TelegramRequest::DeleteMany(request) => TelegramResult::Delete(TelegramDeleteResult {
            chat_id: request.chat_id,
            deleted: request.message_ids.clone(),
            failed: Vec::new(),
        }),
        TelegramRequest::Restrict(request) => {
            TelegramResult::Restriction(TelegramRestrictionResult {
                chat_id: request.chat_id,
                user_id: request.user_id,
                until: request.until,
                permissions: request.permissions.clone(),
                changed: true,
            })
        }
        TelegramRequest::Unrestrict(request) => {
            TelegramResult::Restriction(TelegramRestrictionResult {
                chat_id: request.chat_id,
                user_id: request.user_id,
                until: None,
                permissions: TelegramPermissions::default(),
                changed: true,
            })
        }
        TelegramRequest::Ban(request) => TelegramResult::Ban(TelegramBanResult {
            chat_id: request.chat_id,
            user_id: request.user_id,
            until: request.until,
            delete_history: request.delete_history,
            changed: true,
        }),
        TelegramRequest::Unban(request) => TelegramResult::Ban(TelegramBanResult {
            chat_id: request.chat_id,
            user_id: request.user_id,
            until: None,
            delete_history: false,
            changed: true,
        }),
        TelegramRequest::GetChatAdministrators(request) => {
            TelegramResult::ChatAdministrators(TelegramChatAdministratorsResult {
                chat_id: request.chat_id,
                administrators: Vec::new(),
            })
        }
        TelegramRequest::GetChatMember(request) => {
            TelegramResult::ChatMember(TelegramChatMemberResult {
                chat_id: request.chat_id,
                user_id: request.user_id,
                member: TelegramChatMember {
                    user_id: request.user_id,
                    is_admin: false,
                    can_restrict_members: None,
                },
            })
        }
        TelegramRequest::AnswerCallback(request) => {
            TelegramResult::Callback(TelegramCallbackResult {
                callback_query_id: request.callback_query_id.clone(),
                answered: true,
                show_alert: request.show_alert,
                text: request.text.clone(),
            })
        }
    }
}
