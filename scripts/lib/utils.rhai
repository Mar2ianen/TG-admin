// Форматирует имя пользователя в кликабельную MarkdownV2 ссылку
// Экранируем символы, которые могут сломать MarkdownV2
fn format_user_link(name, user_id) {
    let escaped_name = name.replace("[", "\\[").replace("]", "\\]");
    return "[" + escaped_name + "](tg://user?id=" + user_id + ")";
}

// Полезная функция для логов
fn log_admin_action(op, target_id) {
    unit_log("Admin action: " + op + " on " + target_id);
}
