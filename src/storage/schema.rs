pub const CURRENT_SCHEMA_VERSION: u32 = 2;

pub const MIGRATION_V1_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_bootstrap (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_bootstrap (key, value)
VALUES ('storage_bootstrap', 'initialized');

CREATE TABLE IF NOT EXISTS users (
  user_id INTEGER PRIMARY KEY,
  username TEXT,
  display_name TEXT,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  warn_count INTEGER NOT NULL DEFAULT 0,
  shadowbanned INTEGER NOT NULL DEFAULT 0,
  reputation INTEGER NOT NULL DEFAULT 0,
  state_json TEXT,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS kv_store (
  scope_kind TEXT NOT NULL,
  scope_id TEXT NOT NULL,
  key TEXT NOT NULL,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (scope_kind, scope_id, key)
);

CREATE TABLE IF NOT EXISTS message_journal (
  chat_id INTEGER NOT NULL,
  message_id INTEGER NOT NULL,
  user_id INTEGER,
  date_utc TEXT NOT NULL,
  update_type TEXT NOT NULL,
  text TEXT,
  normalized_text TEXT,
  has_media INTEGER NOT NULL DEFAULT 0,
  reply_to_message_id INTEGER,
  file_ids_json TEXT,
  meta_json TEXT,
  PRIMARY KEY (chat_id, message_id)
);

CREATE INDEX IF NOT EXISTS idx_msg_chat_date
ON message_journal(chat_id, date_utc);

CREATE INDEX IF NOT EXISTS idx_msg_chat_user_date
ON message_journal(chat_id, user_id, date_utc);

CREATE INDEX IF NOT EXISTS idx_msg_chat_reply
ON message_journal(chat_id, reply_to_message_id);

CREATE TABLE IF NOT EXISTS jobs (
  job_id TEXT PRIMARY KEY,
  executor_unit TEXT NOT NULL,
  run_at TEXT NOT NULL,
  scheduled_at TEXT NOT NULL,
  status TEXT NOT NULL,
  dedupe_key TEXT,
  payload_json TEXT NOT NULL,
  retry_count INTEGER NOT NULL DEFAULT 0,
  max_retries INTEGER NOT NULL DEFAULT 0,
  last_error_code TEXT,
  last_error_text TEXT,
  audit_action_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_log (
  action_id TEXT PRIMARY KEY,
  trace_id TEXT,
  request_id TEXT,
  unit_name TEXT NOT NULL,
  execution_mode TEXT NOT NULL,
  op TEXT NOT NULL,
  actor_user_id INTEGER,
  chat_id INTEGER,
  target_kind TEXT,
  target_id TEXT,
  trigger_message_id INTEGER,
  idempotency_key TEXT,
  reversible INTEGER NOT NULL DEFAULT 0,
  compensation_json TEXT,
  args_json TEXT NOT NULL,
  result_json TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS processed_updates (
  update_id INTEGER PRIMARY KEY,
  event_id TEXT NOT NULL,
  processed_at TEXT NOT NULL,
  execution_mode TEXT NOT NULL
);

PRAGMA user_version = 1;
";

pub const MIGRATION_V2_SQL: &str = "
ALTER TABLE processed_updates
ADD COLUMN status TEXT NOT NULL DEFAULT 'completed';

PRAGMA user_version = 2;
";
