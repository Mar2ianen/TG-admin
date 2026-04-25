use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::event::{
    AuthorSourceClass, ChatRouteClass, CommandSource, EventContext, ExecutionMode,
    MessageContentKind, UnitContext, UpdateType,
};
use crate::moderation::{ModerationEngine, ModerationError, ModerationEventResult};
use crate::unit::{TriggerSpec, UnitEventType, UnitRegistry, UnitStatus};
use regex::Regex;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct EventClassifier;

impl Default for EventClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl EventClassifier {
    pub fn new() -> Self {
        Self
    }

    pub fn classify(&self, event: &EventContext) -> ClassifiedEvent {
        let mut traits = Vec::new();
        push_unique(&mut traits, event_trait_for_update_type(event.update_type));

        if event
            .message
            .as_ref()
            .and_then(|message| message.text.as_deref())
            .is_some_and(|text| !text.trim().is_empty())
        {
            push_unique(&mut traits, EventTrait::Text);
        }
        if event.reply.is_some() {
            push_unique(&mut traits, EventTrait::Reply);
        }
        if event
            .message
            .as_ref()
            .is_some_and(|message| message.has_media)
        {
            push_unique(&mut traits, EventTrait::Media);
        }
        if let Some(content_kind) = event
            .message
            .as_ref()
            .and_then(|message| message.content_kind)
        {
            push_unique(&mut traits, event_trait_for_content_kind(content_kind));
            if !matches!(content_kind, MessageContentKind::Text) {
                push_unique(&mut traits, EventTrait::Media);
            }
        }
        if event
            .message
            .as_ref()
            .and_then(|message| message.media_group_id.as_deref())
            .is_some()
        {
            push_unique(&mut traits, EventTrait::MediaGroup);
        }
        if event
            .callback
            .as_ref()
            .and_then(|callback| callback.data.as_deref())
            .is_some()
        {
            push_unique(&mut traits, EventTrait::CallbackData);
        }
        if event.is_linked_channel_style_approx() {
            push_unique(&mut traits, EventTrait::LinkedChannelStyle);
        }

        let command_name = extract_command_name(event);
        if command_name.is_some() {
            push_unique(&mut traits, EventTrait::Command);
        }

        ClassifiedEvent {
            ingress_class: ingress_class_for(event.execution_mode),
            chat_scope: chat_scope_for(event),
            author_kind: author_kind_for(event),
            update_type: event.update_type,
            traits,
            command_name,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum IngressClass {
    Realtime,
    Recovery,
    Scheduled,
    Manual,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum EventTrait {
    Message,
    EditedMessage,
    ChannelPost,
    EditedChannelPost,
    CallbackQuery,
    ChatMember,
    MyChatMember,
    JoinRequest,
    Job,
    System,
    Text,
    Command,
    Reply,
    Media,
    MediaGroup,
    CallbackData,
    LinkedChannelStyle,
    Photo,
    Voice,
    Video,
    Audio,
    Document,
    Sticker,
    Animation,
    VideoNote,
    Contact,
    Location,
    Poll,
    Dice,
    Venue,
    Game,
    Invoice,
    Story,
    UnknownMedia,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum RouteBucket {
    IngressClass(IngressClass),
    ChatScope(ChatScope),
    AuthorKind(AuthorKind),
    EventTrait(EventTrait),
    CommandIndex(String),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ClassifiedEvent {
    pub ingress_class: IngressClass,
    pub chat_scope: ChatScope,
    pub author_kind: AuthorKind,
    pub update_type: UpdateType,
    pub traits: Vec<EventTrait>,
    pub command_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExecutionLane {
    BuiltInModeration,
    UnitDispatch,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum ChatScope {
    Private,
    Group,
    Supergroup,
    Channel,
    Unknown,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum AuthorKind {
    HumanAdmin,
    HumanMember,
    Bot,
    ChannelIdentity,
    System,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RoutePlan {
    pub classified: ClassifiedEvent,
    pub matched_buckets: Vec<RouteBucket>,
    pub lanes: Vec<ExecutionLane>,
}

#[derive(Debug, Clone)]
pub struct RouterIndex {
    ingress_index: BTreeMap<IngressClass, Vec<ExecutionLane>>,
    chat_scope_index: BTreeMap<ChatScope, Vec<ExecutionLane>>,
    author_index: BTreeMap<AuthorKind, Vec<ExecutionLane>>,
    trait_index: BTreeMap<EventTrait, Vec<ExecutionLane>>,
    command_index: BTreeMap<String, Vec<ExecutionLane>>,
}

impl RouterIndex {
    pub fn new() -> Self {
        Self {
            ingress_index: BTreeMap::new(),
            chat_scope_index: BTreeMap::new(),
            author_index: BTreeMap::new(),
            trait_index: BTreeMap::new(),
            command_index: BTreeMap::new(),
        }
    }

    pub fn with_builtin_moderation_commands() -> Self {
        let mut index = Self::new();
        for command in ["warn", "mute", "ban", "del", "undo", "msg"] {
            index
                .command_index
                .insert(command.to_owned(), vec![ExecutionLane::BuiltInModeration]);
        }
        index
    }

    pub fn from_registry(registry: &UnitRegistry) -> Self {
        let mut index = Self::with_builtin_moderation_commands();

        for descriptor in registry.entries() {
            if matches!(descriptor.status, UnitStatus::Failed | UnitStatus::Disabled) {
                continue;
            }
            let Some(manifest) = descriptor.manifest.as_ref() else {
                continue;
            };

            match &manifest.trigger {
                TriggerSpec::Command { commands } => {
                    for command in commands {
                        index = index.register_command_lane(
                            command.trim().to_ascii_lowercase(),
                            ExecutionLane::UnitDispatch,
                        );
                    }
                }
                TriggerSpec::Regex { .. } => {
                    index =
                        index.register_trait_lane(EventTrait::Text, ExecutionLane::UnitDispatch);
                }
                TriggerSpec::EventType { events } => {
                    for event in events {
                        index = index.register_trait_lane(
                            event_trait_for_unit_event(*event),
                            ExecutionLane::UnitDispatch,
                        );
                    }
                }
            }
        }

        index
    }

    pub fn register_ingress_lane(
        mut self,
        ingress_class: IngressClass,
        lane: ExecutionLane,
    ) -> Self {
        self.ingress_index
            .entry(ingress_class)
            .or_default()
            .push(lane);
        self
    }

    pub fn register_trait_lane(mut self, event_trait: EventTrait, lane: ExecutionLane) -> Self {
        self.trait_index.entry(event_trait).or_default().push(lane);
        self
    }

    pub fn register_chat_scope_lane(mut self, chat_scope: ChatScope, lane: ExecutionLane) -> Self {
        self.chat_scope_index
            .entry(chat_scope)
            .or_default()
            .push(lane);
        self
    }

    pub fn register_author_lane(mut self, author_kind: AuthorKind, lane: ExecutionLane) -> Self {
        self.author_index.entry(author_kind).or_default().push(lane);
        self
    }

    pub fn register_command_lane(
        mut self,
        command_name: impl Into<String>,
        lane: ExecutionLane,
    ) -> Self {
        self.command_index
            .entry(command_name.into())
            .or_default()
            .push(lane);
        self
    }

    pub fn plan(&self, classified: ClassifiedEvent) -> RoutePlan {
        let mut matched_buckets = vec![
            RouteBucket::IngressClass(classified.ingress_class),
            RouteBucket::ChatScope(classified.chat_scope),
            RouteBucket::AuthorKind(classified.author_kind),
        ];
        let mut lanes = Vec::new();

        if let Some(indexed_lanes) = self.ingress_index.get(&classified.ingress_class) {
            extend_unique(&mut lanes, indexed_lanes.iter().copied());
        }
        if let Some(indexed_lanes) = self.chat_scope_index.get(&classified.chat_scope) {
            extend_unique(&mut lanes, indexed_lanes.iter().copied());
        }
        if let Some(indexed_lanes) = self.author_index.get(&classified.author_kind) {
            extend_unique(&mut lanes, indexed_lanes.iter().copied());
        }

        for event_trait in &classified.traits {
            matched_buckets.push(RouteBucket::EventTrait(*event_trait));
            if let Some(indexed_lanes) = self.trait_index.get(event_trait) {
                extend_unique(&mut lanes, indexed_lanes.iter().copied());
            }
        }

        if let Some(command_name) = classified.command_name.as_ref() {
            matched_buckets.push(RouteBucket::CommandIndex(command_name.clone()));
            if let Some(indexed_lanes) = self.command_index.get(command_name) {
                extend_unique(&mut lanes, indexed_lanes.iter().copied());
            }
        }

        RoutePlan {
            classified,
            matched_buckets,
            lanes,
        }
    }

    pub fn stats(&self) -> RouterIndexStats {
        RouterIndexStats {
            ingress_routes: self.ingress_index.len(),
            chat_scope_routes: self.chat_scope_index.len(),
            author_routes: self.author_index.len(),
            trait_routes: self.trait_index.len(),
            command_routes: self.command_index.len(),
        }
    }
}

impl Default for RouterIndex {
    fn default() -> Self {
        Self::with_builtin_moderation_commands()
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionRouter {
    classifier: EventClassifier,
    index: Rc<RefCell<RouterIndex>>,
    moderation: Option<ModerationEngine>,
    registry: Rc<RefCell<UnitRegistry>>,
}

impl ExecutionRouter {
    pub fn new() -> Self {
        Self {
            classifier: EventClassifier::new(),
            index: Rc::new(RefCell::new(RouterIndex::default())),
            moderation: None,
            registry: Rc::new(RefCell::new(UnitRegistry::default())),
        }
    }

    pub fn with_index(self, index: RouterIndex) -> Self {
        *self.index.borrow_mut() = index;
        self
    }

    pub fn with_moderation(mut self, moderation: ModerationEngine) -> Self {
        self.moderation = Some(moderation);
        self
    }

    pub fn with_registry(self, registry: UnitRegistry) -> Self {
        self.sync_registry(registry);
        self
    }

    pub fn plan(&self, event: &EventContext) -> RoutePlan {
        self.index.borrow().plan(self.classifier.classify(event))
    }

    pub fn index_stats(&self) -> RouterIndexStats {
        self.index.borrow().stats()
    }

    pub fn sync_registry(&self, registry: UnitRegistry) {
        let index = RouterIndex::from_registry(&registry);
        *self.registry.borrow_mut() = registry;
        *self.index.borrow_mut() = index;
    }

    pub async fn route(&self, event: &EventContext) -> Result<ExecutionOutcome, RoutingError> {
        let plan = self.plan(event);
        let unit_invocations = {
            let registry = self.registry.borrow();
            select_unit_dispatches(&registry, event)
        };

        if plan.lanes.contains(&ExecutionLane::BuiltInModeration) {
            let deferred_lanes = deferred_lanes(&plan, ExecutionLane::BuiltInModeration);
            let moderation = self
                .moderation
                .as_ref()
                .ok_or(RoutingError::MissingLaneExecutor {
                    lane: ExecutionLane::BuiltInModeration,
                })?;
            let built_in_event = bind_manifest_unit_for_built_in(event, &unit_invocations);
            let result = moderation.handle_event(&built_in_event).await?;
            return Ok(ExecutionOutcome::BuiltInModeration {
                plan,
                result,
                deferred_lanes,
            });
        }

        if plan.lanes.contains(&ExecutionLane::UnitDispatch) && !unit_invocations.is_empty() {
            return Ok(ExecutionOutcome::UnitDispatch {
                plan,
                invocations: unit_invocations,
            });
        }

        Ok(ExecutionOutcome::Unhandled(plan))
    }
}

impl Default for ExecutionRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub enum RoutingError {
    MissingLaneExecutor { lane: ExecutionLane },
    Moderation(ModerationError),
}

impl std::fmt::Display for RoutingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingLaneExecutor { lane } => {
                write!(f, "missing executor for routing lane {:?}", lane)
            }
            Self::Moderation(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RoutingError {}

impl From<ModerationError> for RoutingError {
    fn from(value: ModerationError) -> Self {
        Self::Moderation(value)
    }
}

#[derive(Debug)]
pub enum ExecutionOutcome {
    BuiltInModeration {
        plan: RoutePlan,
        result: ModerationEventResult,
        deferred_lanes: Vec<ExecutionLane>,
    },
    UnitDispatch {
        plan: RoutePlan,
        invocations: Vec<UnitDispatchInvocation>,
    },
    Unhandled(RoutePlan),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UnitDispatchInvocation {
    pub unit_id: String,
    pub exec_start: String,
    pub entry_point: Option<String>,
    pub trigger: UnitDispatchTrigger,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UnitDispatchTrigger {
    Command { command: String },
    Regex { pattern: String },
    EventType { event: UnitEventType },
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct RouterIndexStats {
    pub ingress_routes: usize,
    pub chat_scope_routes: usize,
    pub author_routes: usize,
    pub trait_routes: usize,
    pub command_routes: usize,
}

fn ingress_class_for(mode: ExecutionMode) -> IngressClass {
    match mode {
        ExecutionMode::Realtime => IngressClass::Realtime,
        ExecutionMode::Recovery => IngressClass::Recovery,
        ExecutionMode::Scheduled => IngressClass::Scheduled,
        ExecutionMode::Manual => IngressClass::Manual,
    }
}

fn chat_scope_for(event: &EventContext) -> ChatScope {
    match event.chat.as_ref().map(|chat| chat.route_class()) {
        Some(ChatRouteClass::Private) => ChatScope::Private,
        Some(ChatRouteClass::GroupLike) => {
            match event.chat.as_ref().map(|chat| chat.chat_type.as_str()) {
                Some("group") => ChatScope::Group,
                Some("supergroup") => ChatScope::Supergroup,
                _ => ChatScope::Unknown,
            }
        }
        Some(ChatRouteClass::Channel) => ChatScope::Channel,
        _ => ChatScope::Unknown,
    }
}

fn author_kind_for(event: &EventContext) -> AuthorKind {
    match event.author_source_class() {
        AuthorSourceClass::HumanAdmin => AuthorKind::HumanAdmin,
        AuthorSourceClass::HumanMember => AuthorKind::HumanMember,
        AuthorSourceClass::Bot => AuthorKind::Bot,
        AuthorSourceClass::ChannelStyleNoSender => AuthorKind::ChannelIdentity,
        AuthorSourceClass::Unknown => AuthorKind::System,
    }
}

fn event_trait_for_update_type(update_type: UpdateType) -> EventTrait {
    match update_type {
        UpdateType::Message => EventTrait::Message,
        UpdateType::EditedMessage => EventTrait::EditedMessage,
        UpdateType::ChannelPost => EventTrait::ChannelPost,
        UpdateType::EditedChannelPost => EventTrait::EditedChannelPost,
        UpdateType::CallbackQuery => EventTrait::CallbackQuery,
        UpdateType::ChatMember => EventTrait::ChatMember,
        UpdateType::MyChatMember => EventTrait::MyChatMember,
        UpdateType::JoinRequest => EventTrait::JoinRequest,
        UpdateType::Job => EventTrait::Job,
        UpdateType::System => EventTrait::System,
    }
}

fn event_trait_for_content_kind(content_kind: MessageContentKind) -> EventTrait {
    match content_kind {
        MessageContentKind::Text => EventTrait::Text,
        MessageContentKind::Photo => EventTrait::Photo,
        MessageContentKind::Voice => EventTrait::Voice,
        MessageContentKind::Video => EventTrait::Video,
        MessageContentKind::Audio => EventTrait::Audio,
        MessageContentKind::Document => EventTrait::Document,
        MessageContentKind::Sticker => EventTrait::Sticker,
        MessageContentKind::Animation => EventTrait::Animation,
        MessageContentKind::VideoNote => EventTrait::VideoNote,
        MessageContentKind::Contact => EventTrait::Contact,
        MessageContentKind::Location => EventTrait::Location,
        MessageContentKind::Poll => EventTrait::Poll,
        MessageContentKind::Dice => EventTrait::Dice,
        MessageContentKind::Venue => EventTrait::Venue,
        MessageContentKind::Game => EventTrait::Game,
        MessageContentKind::Invoice => EventTrait::Invoice,
        MessageContentKind::Story => EventTrait::Story,
        MessageContentKind::UnknownMedia => EventTrait::UnknownMedia,
    }
}

fn event_trait_for_unit_event(event_type: UnitEventType) -> EventTrait {
    match event_type {
        UnitEventType::Message => EventTrait::Message,
        UnitEventType::CallbackQuery => EventTrait::CallbackQuery,
        UnitEventType::Job => EventTrait::Job,
    }
}

fn extract_command_name(event: &EventContext) -> Option<String> {
    match event.command_source()? {
        CommandSource::MessageText(text) | CommandSource::CallbackData(text) => {
            parse_command_name(text)
        }
    }
}

fn parse_command_name(raw: &str) -> Option<String> {
    let command = raw.trim().strip_prefix('/')?;
    let token = command.split_whitespace().next()?;
    let command_name = token.split('@').next().unwrap_or(token);
    if command_name.is_empty()
        || !command_name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }

    Some(command_name.to_ascii_lowercase())
}

fn push_unique<T>(items: &mut Vec<T>, item: T)
where
    T: PartialEq,
{
    if !items.contains(&item) {
        items.push(item);
    }
}

fn extend_unique<T>(items: &mut Vec<T>, iter: impl IntoIterator<Item = T>)
where
    T: PartialEq,
{
    for item in iter {
        push_unique(items, item);
    }
}

fn deferred_lanes(plan: &RoutePlan, executed_lane: ExecutionLane) -> Vec<ExecutionLane> {
    plan.lanes
        .iter()
        .copied()
        .filter(|lane| *lane != executed_lane)
        .collect()
}

fn bind_manifest_unit_for_built_in(
    event: &EventContext,
    invocations: &[UnitDispatchInvocation],
) -> EventContext {
    let Some(invocation) = invocations.first() else {
        return event.clone();
    };

    event.clone().bind_unit(
        UnitContext::new(invocation.unit_id.clone())
            .with_trigger(unit_trigger_name(&invocation.trigger)),
    )
}

fn unit_trigger_name(trigger: &UnitDispatchTrigger) -> &'static str {
    match trigger {
        UnitDispatchTrigger::Command { .. }
        | UnitDispatchTrigger::Regex { .. }
        | UnitDispatchTrigger::EventType { .. } => "telegram",
    }
}

fn select_unit_dispatches(
    registry: &UnitRegistry,
    event: &EventContext,
) -> Vec<UnitDispatchInvocation> {
    registry
        .entries()
        .iter()
        .filter(|descriptor| matches!(descriptor.status, UnitStatus::Loaded | UnitStatus::Active))
        .filter_map(|descriptor| {
            let manifest = descriptor.manifest.as_ref()?;
            let trigger = match_trigger(&manifest.trigger, event)?;

            Some(UnitDispatchInvocation {
                unit_id: descriptor.id.clone(),
                exec_start: manifest.service.exec_start.clone(),
                entry_point: manifest.service.entry_point.clone(),
                trigger,
            })
        })
        .collect()
}

fn match_trigger(trigger: &TriggerSpec, event: &EventContext) -> Option<UnitDispatchTrigger> {
    match trigger {
        TriggerSpec::Command { commands } => {
            let command = extract_command_name(event)?;
            commands
                .iter()
                .find(|candidate| candidate.trim().eq_ignore_ascii_case(&command))
                .map(|_| UnitDispatchTrigger::Command { command })
        }
        TriggerSpec::Regex { pattern } => {
            let haystack = trigger_text(event)?;
            Regex::new(pattern)
                .ok()
                .filter(|regex| regex.is_match(haystack))
                .map(|_| UnitDispatchTrigger::Regex {
                    pattern: pattern.clone(),
                })
        }
        TriggerSpec::EventType { events } => {
            let unit_event = unit_event_type_for(event.update_type)?;
            events
                .iter()
                .copied()
                .find(|candidate| *candidate == unit_event)
                .map(|event| UnitDispatchTrigger::EventType { event })
        }
    }
}

fn trigger_text(event: &EventContext) -> Option<&str> {
    event
        .message
        .as_ref()
        .and_then(|message| message.text.as_deref())
        .or_else(|| {
            event
                .callback
                .as_ref()
                .and_then(|callback| callback.data.as_deref())
        })
}

fn unit_event_type_for(update_type: UpdateType) -> Option<UnitEventType> {
    match update_type {
        UpdateType::Message
        | UpdateType::EditedMessage
        | UpdateType::ChannelPost
        | UpdateType::EditedChannelPost => Some(UnitEventType::Message),
        UpdateType::CallbackQuery => Some(UnitEventType::CallbackQuery),
        UpdateType::Job => Some(UnitEventType::Job),
        UpdateType::ChatMember
        | UpdateType::MyChatMember
        | UpdateType::JoinRequest
        | UpdateType::System => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorKind, ChatScope, EventClassifier, EventTrait, ExecutionLane, ExecutionOutcome,
        ExecutionRouter, IngressClass, RouteBucket, RouterIndex, RoutingError,
        UnitDispatchInvocation, UnitDispatchTrigger,
    };
    use crate::event::{
        CallbackContext, ChatContext, EventContext, EventNormalizer, ExecutionMode,
        ManualInvocationInput, MessageContentKind, MessageContext, ScheduledJobInput,
        SenderContext, SystemContext, TelegramUpdateInput, UnitContext, UpdateType,
    };
    use crate::moderation::{ModerationEngine, ModerationEventResult};
    use crate::storage::Storage;
    use crate::tg::TelegramGateway;
    use crate::unit::{
        CapabilitiesSpec, ServiceSpec, TriggerSpec, UnitDefinition, UnitEventType, UnitManifest,
        UnitRegistry,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use tempfile::tempdir;

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 22, 12, 0, 0)
            .single()
            .expect("valid timestamp")
    }

    fn chat() -> ChatContext {
        ChatContext {
            id: -100123,
            chat_type: "supergroup".to_owned(),
            title: Some("Moderation HQ".to_owned()),
            username: Some("mod_hq".to_owned()),
            thread_id: Some(11),
        }
    }

    fn private_chat() -> ChatContext {
        ChatContext {
            id: 42,
            chat_type: "private".to_owned(),
            title: None,
            username: Some("dm_user".to_owned()),
            thread_id: None,
        }
    }

    fn admin_sender() -> SenderContext {
        SenderContext {
            id: 42,
            username: Some("admin".to_owned()),
            display_name: Some("Admin".to_owned()),
            is_bot: false,
            is_admin: true,
            role: Some("owner".to_owned()),
        }
    }

    fn manual_event(command_text: &str) -> EventContext {
        let normalizer = EventNormalizer::new();
        let mut input = ManualInvocationInput::new(
            UnitContext::new("moderation.test").with_trigger("manual"),
            command_text,
        );
        input.received_at = ts();
        input.chat = Some(chat());
        input.sender = Some(admin_sender());
        normalizer
            .normalize_manual(input)
            .expect("manual event normalizes")
    }

    fn scheduled_event(command_text: &str) -> EventContext {
        let normalizer = EventNormalizer::new();
        let mut input = ScheduledJobInput::new(
            "job_123",
            UnitContext::new("moderation.test").with_trigger("schedule"),
            json!({ "kind": "scheduled" }),
            ts(),
            ts(),
        );
        input.received_at = ts();
        input.chat = Some(chat());
        input.sender = Some(admin_sender());
        input.command_text = Some(command_text.to_owned());
        normalizer
            .normalize_scheduled(input)
            .expect("scheduled event normalizes")
    }

    fn callback_event(data: &str) -> EventContext {
        let mut event = EventContext::new(
            "evt_callback",
            UpdateType::CallbackQuery,
            ExecutionMode::Realtime,
            SystemContext::realtime(),
        );
        event.update_id = Some(1001);
        event.chat = Some(chat());
        event.sender = Some(admin_sender());
        event.callback = Some(CallbackContext {
            query_id: "cbq-1".to_owned(),
            data: Some(data.to_owned()),
            message_id: Some(700),
            origin_chat_id: Some(-100123),
            from_user_id: 42,
        });
        event
    }

    fn realtime_text_event(text: &str) -> EventContext {
        let mut input = TelegramUpdateInput::message(
            1002,
            chat(),
            admin_sender(),
            MessageContext {
                id: 701,
                date: ts(),
                text: Some(text.to_owned()),
                content_kind: Some(MessageContentKind::Text),
                entities: vec![],
                has_media: false,
                file_ids: Vec::new(),
                reply_to_message_id: None,
                media_group_id: None,
            },
        );
        input.received_at = ts();
        EventNormalizer::new()
            .normalize_telegram(input)
            .expect("telegram event normalizes")
    }

    fn private_text_event(text: &str) -> EventContext {
        let mut input = TelegramUpdateInput::message(
            1003,
            private_chat(),
            admin_sender(),
            MessageContext {
                id: 702,
                date: ts(),
                text: Some(text.to_owned()),
                content_kind: Some(MessageContentKind::Text),
                entities: vec![],
                has_media: false,
                file_ids: Vec::new(),
                reply_to_message_id: None,
                media_group_id: None,
            },
        );
        input.received_at = ts();
        EventNormalizer::new()
            .normalize_telegram(input)
            .expect("private telegram event normalizes")
    }

    fn linked_channel_style_group_event() -> EventContext {
        let mut event = EventContext::new(
            "evt_linked_channel_style",
            UpdateType::Message,
            ExecutionMode::Realtime,
            SystemContext::realtime(),
        );
        event.update_id = Some(1004);
        event.chat = Some(chat());
        event.message = Some(MessageContext {
            id: 703,
            date: ts(),
            text: Some("channel-style post".to_owned()),
            content_kind: Some(MessageContentKind::Text),
            entities: vec![],
            has_media: false,
            file_ids: Vec::new(),
            reply_to_message_id: None,
            media_group_id: None,
        });
        event
    }

    fn registry_with_caps(caps: &[&str]) -> UnitRegistry {
        let mut manifest = UnitManifest::new(
            UnitDefinition::new("moderation.test"),
            TriggerSpec::command(["warn", "mute", "ban", "del", "undo", "msg"]),
            ServiceSpec::new("scripts/moderation/test.rhai"),
        );
        manifest.capabilities = CapabilitiesSpec {
            allow: caps.iter().map(|value| (*value).to_owned()).collect(),
            deny: Vec::new(),
        };
        UnitRegistry::load_manifests(vec![manifest]).registry
    }

    fn registry_from_manifests(manifests: Vec<UnitManifest>) -> UnitRegistry {
        UnitRegistry::load_manifests(manifests).registry
    }

    fn router_with_moderation() -> ExecutionRouter {
        let dir = tempdir().expect("tempdir");
        let storage = Storage::new(dir.path().join("runtime.sqlite3"))
            .bootstrap()
            .expect("bootstrap")
            .into_connection();
        let gateway = TelegramGateway::new(false);
        let engine = ModerationEngine::new(storage, gateway)
            .with_unit_registry(registry_with_caps(&[]))
            .with_admin_user_ids([42]);
        ExecutionRouter::new().with_moderation(engine)
    }

    #[test]
    fn classifier_marks_manual_command_reply_traits() {
        let classifier = EventClassifier::new();
        let mut event = manual_event("/warn @spam spam");
        event.reply = Some(crate::event::ReplyContext {
            message_id: 900,
            sender_user_id: Some(99),
            sender_username: Some("spam".to_owned()),
            text: Some("spam".to_owned()),
            has_media: false,
        });
        event.message = event.message.map(|message| message.with_reply(Some(900)));

        let classified = classifier.classify(&event);

        assert_eq!(classified.ingress_class, IngressClass::Manual);
        assert_eq!(classified.command_name.as_deref(), Some("warn"));
        assert!(classified.traits.contains(&EventTrait::System));
        assert!(classified.traits.contains(&EventTrait::Text));
        assert!(classified.traits.contains(&EventTrait::Command));
        assert!(classified.traits.contains(&EventTrait::Reply));
    }

    #[test]
    fn classifier_marks_scheduled_job_and_command() {
        let classifier = EventClassifier::new();
        let classified = classifier.classify(&scheduled_event("/mute @spam 1h"));

        assert_eq!(classified.ingress_class, IngressClass::Scheduled);
        assert_eq!(classified.command_name.as_deref(), Some("mute"));
        assert!(classified.traits.contains(&EventTrait::Job));
        assert!(classified.traits.contains(&EventTrait::Text));
        assert!(classified.traits.contains(&EventTrait::Command));
    }

    #[test]
    fn classifier_marks_callback_command_bucket() {
        let classifier = EventClassifier::new();
        let classified = classifier.classify(&callback_event("/undo"));

        assert_eq!(classified.ingress_class, IngressClass::Realtime);
        assert_eq!(classified.command_name.as_deref(), Some("undo"));
        assert!(classified.traits.contains(&EventTrait::CallbackQuery));
        assert!(classified.traits.contains(&EventTrait::CallbackData));
        assert!(classified.traits.contains(&EventTrait::Command));
    }

    #[test]
    fn classifier_marks_private_chat_scope_and_human_admin_author() {
        let classifier = EventClassifier::new();
        let classified = classifier.classify(&private_text_event("hello"));

        assert_eq!(classified.chat_scope, ChatScope::Private);
        assert_eq!(classified.author_kind, AuthorKind::HumanAdmin);
        assert!(classified.traits.contains(&EventTrait::Message));
        assert!(classified.traits.contains(&EventTrait::Text));
    }

    #[test]
    fn classifier_marks_linked_channel_style_approximation() {
        let classifier = EventClassifier::new();
        let classified = classifier.classify(&linked_channel_style_group_event());

        assert_eq!(classified.chat_scope, ChatScope::Supergroup);
        assert_eq!(classified.author_kind, AuthorKind::ChannelIdentity);
        assert!(classified.traits.contains(&EventTrait::LinkedChannelStyle));
        assert!(classified.traits.contains(&EventTrait::Message));
    }

    #[test]
    fn classifier_marks_voice_bucket_when_content_kind_is_voice() {
        let classifier = EventClassifier::new();
        let mut event = realtime_text_event("");
        event.message = Some(MessageContext {
            id: 704,
            date: ts(),
            text: None,
            content_kind: Some(MessageContentKind::Voice),
            entities: vec![],
            has_media: true,
            file_ids: vec!["voice-file".to_owned()],
            reply_to_message_id: None,
            media_group_id: None,
        });

        let classified = classifier.classify(&event);

        assert!(classified.traits.contains(&EventTrait::Voice));
        assert!(classified.traits.contains(&EventTrait::Media));
        assert!(!classified.traits.contains(&EventTrait::Text));
    }

    #[test]
    fn classifier_maps_every_message_content_kind_to_its_bucket() {
        let cases = [
            (MessageContentKind::Text, EventTrait::Text, false),
            (MessageContentKind::Photo, EventTrait::Photo, true),
            (MessageContentKind::Voice, EventTrait::Voice, true),
            (MessageContentKind::Video, EventTrait::Video, true),
            (MessageContentKind::Audio, EventTrait::Audio, true),
            (MessageContentKind::Document, EventTrait::Document, true),
            (MessageContentKind::Sticker, EventTrait::Sticker, true),
            (MessageContentKind::Animation, EventTrait::Animation, true),
            (MessageContentKind::VideoNote, EventTrait::VideoNote, true),
            (MessageContentKind::Contact, EventTrait::Contact, true),
            (MessageContentKind::Location, EventTrait::Location, true),
            (MessageContentKind::Poll, EventTrait::Poll, true),
            (MessageContentKind::Dice, EventTrait::Dice, true),
            (MessageContentKind::Venue, EventTrait::Venue, true),
            (MessageContentKind::Game, EventTrait::Game, true),
            (MessageContentKind::Invoice, EventTrait::Invoice, true),
            (MessageContentKind::Story, EventTrait::Story, true),
            (
                MessageContentKind::UnknownMedia,
                EventTrait::UnknownMedia,
                true,
            ),
        ];
        let classifier = EventClassifier::new();

        for (content_kind, expected_trait, expects_media_trait) in cases {
            let mut event = realtime_text_event("");
            event.message = Some(MessageContext {
                id: 900,
                date: ts(),
                text: matches!(content_kind, MessageContentKind::Text).then(|| "hello".to_owned()),
                content_kind: Some(content_kind),
                entities: vec![],
                has_media: expects_media_trait,
                file_ids: Vec::new(),
                reply_to_message_id: None,
                media_group_id: None,
            });

            let classified = classifier.classify(&event);

            assert!(
                classified.traits.contains(&expected_trait),
                "expected trait {expected_trait:?} for {content_kind:?}"
            );
            assert_eq!(
                classified.traits.contains(&EventTrait::Media),
                expects_media_trait,
                "unexpected media trait state for {content_kind:?}"
            );
        }
    }

    #[test]
    fn router_plan_tracks_buckets_for_non_command_text() {
        let router = ExecutionRouter::new();
        let plan = router.plan(&realtime_text_event("hello"));

        assert!(
            plan.matched_buckets
                .contains(&RouteBucket::IngressClass(IngressClass::Realtime))
        );
        assert!(
            plan.matched_buckets
                .contains(&RouteBucket::ChatScope(ChatScope::Supergroup))
        );
        assert!(
            plan.matched_buckets
                .contains(&RouteBucket::AuthorKind(AuthorKind::HumanAdmin))
        );
        assert!(
            plan.matched_buckets
                .contains(&RouteBucket::EventTrait(EventTrait::Message))
        );
        assert!(
            plan.matched_buckets
                .contains(&RouteBucket::EventTrait(EventTrait::Text))
        );
        assert!(plan.lanes.is_empty());
    }

    #[test]
    fn router_index_builds_unit_dispatch_routes_from_registry_triggers() {
        let command_manifest = UnitManifest::new(
            UnitDefinition::new("command.stats.unit"),
            TriggerSpec::command(["stats"]),
            ServiceSpec::new("scripts/command/stats.rhai"),
        );
        let callback_manifest = UnitManifest::new(
            UnitDefinition::new("callback.resolve.unit"),
            TriggerSpec::event_type([UnitEventType::CallbackQuery]),
            ServiceSpec::new("scripts/callback/resolve.rhai"),
        );
        let regex_manifest = UnitManifest::new(
            UnitDefinition::new("message.link_filter.unit"),
            TriggerSpec::regex("https?://"),
            ServiceSpec::new("scripts/message/link_filter.rhai"),
        );
        let index = RouterIndex::from_registry(&registry_from_manifests(vec![
            command_manifest,
            callback_manifest,
            regex_manifest,
        ]));
        let stats = index.stats();

        assert!(stats.command_routes >= 7);
        assert!(stats.trait_routes >= 2);

        let command_plan = index.plan(EventClassifier::new().classify(&manual_event("/stats")));
        assert!(command_plan.lanes.contains(&ExecutionLane::UnitDispatch));
        assert!(
            command_plan
                .matched_buckets
                .contains(&RouteBucket::CommandIndex("stats".to_owned()))
        );

        let callback_plan = index.plan(EventClassifier::new().classify(&callback_event("resolve")));
        assert!(
            callback_plan
                .matched_buckets
                .contains(&RouteBucket::EventTrait(EventTrait::CallbackQuery))
        );
        assert!(callback_plan.lanes.contains(&ExecutionLane::UnitDispatch));
    }

    #[test]
    fn router_sync_registry_rebuilds_from_live_registry_state() {
        let stats_manifest = UnitManifest::new(
            UnitDefinition::new("command.stats.unit"),
            TriggerSpec::command(["stats"]),
            ServiceSpec::new("scripts/command/stats.rhai"),
        );
        let audit_manifest = UnitManifest::new(
            UnitDefinition::new("command.audit.unit"),
            TriggerSpec::command(["audit"]),
            ServiceSpec::new("scripts/command/audit.rhai"),
        );
        let router = ExecutionRouter::new()
            .with_registry(registry_from_manifests(vec![stats_manifest]));

        assert!(
            router
                .plan(&manual_event("/stats"))
                .lanes
                .contains(&ExecutionLane::UnitDispatch)
        );
        assert!(
            !router
                .plan(&manual_event("/audit"))
                .lanes
                .contains(&ExecutionLane::UnitDispatch)
        );

        router.sync_registry(registry_from_manifests(vec![audit_manifest]));

        assert!(
            !router
                .plan(&manual_event("/stats"))
                .lanes
                .contains(&ExecutionLane::UnitDispatch)
        );
        assert!(
            router
                .plan(&manual_event("/audit"))
                .lanes
                .contains(&ExecutionLane::UnitDispatch)
        );
    }

    #[tokio::test]
    async fn router_executes_built_in_moderation_for_indexed_command() {
        let router = router_with_moderation();
        let mut event = manual_event("/warn @spam spam");
        event.reply = Some(crate::event::ReplyContext {
            message_id: 900,
            sender_user_id: Some(99),
            sender_username: Some("spam".to_owned()),
            text: Some("spam".to_owned()),
            has_media: false,
        });
        event.message = event.message.map(|message| message.with_reply(Some(900)));

        let outcome = router.route(&event).await.expect("routing succeeds");

        let ExecutionOutcome::BuiltInModeration {
            plan,
            result,
            deferred_lanes,
        } = outcome
        else {
            panic!("expected built-in moderation outcome");
        };
        assert!(
            plan.matched_buckets
                .contains(&RouteBucket::CommandIndex("warn".to_owned()))
        );
        assert!(deferred_lanes.is_empty());
        assert!(matches!(result, ModerationEventResult::Executed(_)));
    }

    #[tokio::test]
    async fn router_surfaces_deferred_unit_dispatch_when_built_in_and_unit_match_same_command() {
        let unit_manifest = UnitManifest::new(
            UnitDefinition::new("moderation.warn.shadow"),
            TriggerSpec::command(["warn"]),
            ServiceSpec::new("scripts/moderation/warn_shadow.rhai"),
        );
        let registry = registry_from_manifests(vec![unit_manifest]);
        let router = router_with_moderation()
            .with_registry(registry.clone());
        let mut event = manual_event("/warn @spam spam");
        event.reply = Some(crate::event::ReplyContext {
            message_id: 900,
            sender_user_id: Some(99),
            sender_username: Some("spam".to_owned()),
            text: Some("spam".to_owned()),
            has_media: false,
        });
        event.message = event.message.map(|message| message.with_reply(Some(900)));

        let outcome = router.route(&event).await.expect("routing succeeds");

        let ExecutionOutcome::BuiltInModeration {
            plan,
            result,
            deferred_lanes,
        } = outcome
        else {
            panic!("expected built-in moderation outcome");
        };
        assert_eq!(
            plan.lanes,
            vec![
                ExecutionLane::BuiltInModeration,
                ExecutionLane::UnitDispatch,
            ]
        );
        assert_eq!(deferred_lanes, vec![ExecutionLane::UnitDispatch]);
        assert!(matches!(result, ModerationEventResult::Executed(_)));
    }

    #[tokio::test]
    async fn router_binds_manifest_unit_context_before_built_in_moderation() {
        let mut unit_manifest = UnitManifest::new(
            UnitDefinition::new("moderation.mute.shadow"),
            TriggerSpec::command(["mute"]),
            ServiceSpec::new("scripts/moderation/mute_shadow.rhai"),
        );
        unit_manifest.capabilities = CapabilitiesSpec {
            allow: vec!["tg.moderate.restrict".to_owned()],
            deny: Vec::new(),
        };
        let registry = registry_from_manifests(vec![unit_manifest]);
        let dir = tempdir().expect("tempdir");
        let storage = Storage::new(dir.path().join("runtime.sqlite3"))
            .bootstrap()
            .expect("bootstrap")
            .into_connection();
        let gateway = TelegramGateway::new(false);
        let engine = ModerationEngine::new(storage, gateway)
            .with_unit_registry(registry.clone())
            .with_dry_run(true)
            .with_admin_user_ids([42]);
        let router = ExecutionRouter::new()
            .with_registry(registry.clone())
            .with_moderation(engine);
        let mut event = manual_event("/mute 30m spam");
        event.reply = Some(crate::event::ReplyContext {
            message_id: 900,
            sender_user_id: Some(99),
            sender_username: Some("spam".to_owned()),
            text: Some("spam".to_owned()),
            has_media: false,
        });
        event.message = event.message.map(|message| message.with_reply(Some(900)));

        let outcome = router.route(&event).await.expect("routing succeeds");

        let ExecutionOutcome::BuiltInModeration {
            result,
            deferred_lanes,
            ..
        } = outcome
        else {
            panic!("expected built-in moderation outcome");
        };
        assert_eq!(deferred_lanes, vec![ExecutionLane::UnitDispatch]);
        assert!(matches!(result, ModerationEventResult::Executed(_)));
    }

    #[tokio::test]
    async fn router_dispatches_matching_command_units_with_service_envelope() {
        let mut manifest = UnitManifest::new(
            UnitDefinition::new("moderation.stats.audit"),
            TriggerSpec::command(["stats", "audit_stats"]),
            ServiceSpec::new("scripts/moderation/stats_audit.rhai"),
        );
        manifest.service.entry_point = Some("handle_stats".to_owned());
        let router = ExecutionRouter::new().with_registry(registry_from_manifests(vec![manifest]));

        let outcome = router
            .route(&manual_event("/stats"))
            .await
            .expect("routing succeeds");

        let ExecutionOutcome::UnitDispatch { plan, invocations } = outcome else {
            panic!("expected unit dispatch outcome");
        };
        assert_eq!(plan.lanes, vec![ExecutionLane::UnitDispatch]);
        assert_eq!(
            invocations,
            vec![UnitDispatchInvocation {
                unit_id: "moderation.stats.audit".to_owned(),
                exec_start: "scripts/moderation/stats_audit.rhai".to_owned(),
                entry_point: Some("handle_stats".to_owned()),
                trigger: UnitDispatchTrigger::Command {
                    command: "stats".to_owned(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn router_dispatches_matching_regex_units_only_when_pattern_matches() {
        let matching = UnitManifest::new(
            UnitDefinition::new("message.link.filter"),
            TriggerSpec::regex("https?://"),
            ServiceSpec::new("scripts/filter/link.rhai"),
        );
        let non_matching = UnitManifest::new(
            UnitDefinition::new("message.phone.filter"),
            TriggerSpec::regex("\\+\\d{11}"),
            ServiceSpec::new("scripts/filter/phone.rhai"),
        );
        let router = ExecutionRouter::new()
            .with_registry(registry_from_manifests(vec![matching, non_matching]));

        let outcome = router
            .route(&realtime_text_event("visit https://example.com now"))
            .await
            .expect("routing succeeds");

        let ExecutionOutcome::UnitDispatch { invocations, .. } = outcome else {
            panic!("expected unit dispatch outcome");
        };
        assert_eq!(
            invocations,
            vec![UnitDispatchInvocation {
                unit_id: "message.link.filter".to_owned(),
                exec_start: "scripts/filter/link.rhai".to_owned(),
                entry_point: None,
                trigger: UnitDispatchTrigger::Regex {
                    pattern: "https?://".to_owned(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn router_dispatches_matching_event_type_units() {
        let manifest = UnitManifest::new(
            UnitDefinition::new("callback.resolve"),
            TriggerSpec::event_type([UnitEventType::CallbackQuery]),
            ServiceSpec::new("scripts/callback/resolve.rhai"),
        );
        let router = ExecutionRouter::new().with_registry(registry_from_manifests(vec![manifest]));

        let outcome = router
            .route(&callback_event("resolve:123"))
            .await
            .expect("routing succeeds");

        let ExecutionOutcome::UnitDispatch { invocations, .. } = outcome else {
            panic!("expected unit dispatch outcome");
        };
        assert_eq!(
            invocations,
            vec![UnitDispatchInvocation {
                unit_id: "callback.resolve".to_owned(),
                exec_start: "scripts/callback/resolve.rhai".to_owned(),
                entry_point: None,
                trigger: UnitDispatchTrigger::EventType {
                    event: UnitEventType::CallbackQuery,
                },
            }]
        );
    }

    #[tokio::test]
    async fn router_reports_missing_executor_for_indexed_lane() {
        let router = ExecutionRouter::new();
        let mut event = manual_event("/warn @spam spam");
        event.reply = Some(crate::event::ReplyContext {
            message_id: 900,
            sender_user_id: Some(99),
            sender_username: Some("spam".to_owned()),
            text: Some("spam".to_owned()),
            has_media: false,
        });
        event.message = event.message.map(|message| message.with_reply(Some(900)));

        let error = router
            .route(&event)
            .await
            .expect_err("missing executor must fail");

        assert!(matches!(error, RoutingError::MissingLaneExecutor { .. }));
    }
}
