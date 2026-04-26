// Теперь используем умный рендерер
let chat_id = event.chat.id;

// render_auto сам достанет имя, chat_title, дату и время
let text = render_auto("welcome");

tg_send_message(chat_id, text);
