use crate::event::{ChatContext, EventContext, SenderContext};
use chrono::Utc;
use std::collections::HashMap;

pub struct TemplateContext {
    vars: HashMap<String, String>,
}

impl TemplateContext {
    pub fn new() -> Self {
        Self {
            vars: HashMap::new(),
        }
    }

    pub fn with_user(mut self, sender: &SenderContext) -> Self {
        self.vars.insert(
            "user_name".to_owned(),
            sender.display_name.clone().unwrap_or_default(),
        );
        self.vars
            .insert("user_id".to_owned(), sender.id.to_string());
        self.vars.insert(
            "user_link".to_owned(),
            format!(
                "[{}](tg://user?id={})",
                sender.display_name.clone().unwrap_or_default(),
                sender.id
            ),
        );
        self.vars
            .insert("user_is_admin".to_owned(), sender.is_admin.to_string());
        self
    }

    pub fn with_chat(mut self, chat: &ChatContext) -> Self {
        self.vars.insert(
            "chat_title".to_owned(),
            chat.title.clone().unwrap_or_default(),
        );
        self.vars.insert("chat_id".to_owned(), chat.id.to_string());
        self
    }

    pub fn with_cron_metadata(mut self) -> Self {
        let now = Utc::now();
        self.vars
            .insert("date".to_owned(), now.format("%Y-%m-%d").to_string());
        self.vars
            .insert("time".to_owned(), now.format("%H:%M").to_string());
        self
    }

    pub fn into_map(self) -> HashMap<String, String> {
        self.vars
    }
}

pub fn render_template(template: &str, vars: HashMap<String, String>) -> String {
    let mut rendered = template.to_owned();
    for (key, val) in vars {
        rendered = rendered.replace(&format!("{{{}}}", key), &val);
    }
    rendered
}
