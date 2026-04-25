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
    pub update_type: crate::event::UpdateType,
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

#[derive(Debug)]
pub enum ExecutionOutcome {
    BuiltInModeration {
        plan: RoutePlan,
        result: crate::moderation::ModerationEventResult,
        deferred_invocations: Vec<UnitDispatchInvocation>,
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
    EventType { event: crate::unit::UnitEventType },
}
