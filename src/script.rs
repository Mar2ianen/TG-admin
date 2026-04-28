//! Rhai script execution sandbox for unit-driven dispatch.
//!
//! Scripts receive the current [`crate::event::EventContext`] as a Rhai map variable
//! named `event`, and can call back into the runtime via registered bridge functions
//! (`db_kv_get`, `db_kv_set`, `db_user_get_json`, `ctx_current_json`, `unit_log`,
//! `unit_warn`, `ml_transcribe`, `tg_send_message`).
//!
//! The bridge uses thread-local storage to pass [`HostApi`] and [`EventContext`]
//! references into sync Rhai closures. This is safe because the runtime uses
//! `flavor = "current_thread"` (single OS thread) and [`BridgeGuard`] ensures
//! the thread-local is cleared before the references become invalid.

use crate::event::EventContext;

use crate::host_api::contract::HostApiValue;
use crate::host_api::contract::{
    DbKvGetRequest, DbKvSetRequest, DbUserGetRequest, HostApiRequest, MlTranscribeRequest,
    TgSendMessageRequest,
};
use crate::host_api::ml::{
    MlChatCompletionsRequest, MlChatMessage, MlEmbedTextRequest, MlHealthRequest, MlModelsRequest,
};
use crate::host_api::HostApi;
use crate::storage::KvEntry;
use rhai::{Dynamic, Engine, Scope};
use std::cell::RefCell;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Thread-local bridge
// ---------------------------------------------------------------------------

struct BridgeState {
    host_api: *const HostApi,
    event: *const EventContext,
}

// SAFETY: BridgeState is only ever accessed on the thread that set it (via
// thread_local!). BridgeGuard guarantees the pointers are cleared before
// the referents are freed. The runtime is single-threaded (current_thread).
unsafe impl Send for BridgeState {}
unsafe impl Sync for BridgeState {}

thread_local! {
    static BRIDGE: RefCell<Option<BridgeState>> = const { RefCell::new(None) };
}

struct BridgeGuard;

impl BridgeGuard {
    fn enter(host_api: &HostApi, event: &EventContext) -> Self {
        BRIDGE.with(|b| {
            *b.borrow_mut() = Some(BridgeState {
                host_api: host_api as *const _,
                event: event as *const _,
            });
        });
        BridgeGuard
    }
}

impl Drop for BridgeGuard {
    fn drop(&mut self) {
        BRIDGE.with(|b| {
            *b.borrow_mut() = None;
        });
    }
}

fn with_bridge<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&HostApi, &EventContext) -> R,
{
    BRIDGE.with(|b| {
        b.borrow().as_ref().map(|bridge| {
            // SAFETY: see BridgeState above
            let host_api = unsafe { &*bridge.host_api };
            let event = unsafe { &*bridge.event };
            f(host_api, event)
        })
    })
}

// ---------------------------------------------------------------------------
// ScriptRunner
// ---------------------------------------------------------------------------

/// Executes Rhai unit scripts with a sandboxed engine and HostApi bridge.
#[derive(Debug, Clone)]
pub struct ScriptRunner {
    /// Directory where unit scripts live (maps to `config.paths.scripts_dir`).
    pub scripts_dir: PathBuf,
    /// Maximum Rhai operations per execution (DoS guard).
    pub max_operations: u64,
}

impl ScriptRunner {
    pub(crate) fn new(scripts_dir: PathBuf) -> Self {
        Self {
            scripts_dir,
            max_operations: 500_000,
        }
    }

    /// Execute a unit script.
    ///
    /// - `exec_start`: relative path to the `.rhai` file (from `scripts_dir`)
    /// - `entry_point`: optional function name to call; defaults to `"main"`;
    ///   if no function with that name exists, falls back to running top-level code
    /// - `event`: current event context (passed as `event` variable in script scope)
    /// - `host_api`: HostApi instance for bridge callbacks
    pub(crate) fn execute(
        &self,
        exec_start: &str,
        entry_point: Option<&str>,
        event: &EventContext,
        host_api: &HostApi,
    ) -> Result<(), ScriptError> {
        let script_path = self.scripts_dir.join(exec_start);
        let source = std::fs::read_to_string(&script_path).map_err(|e| ScriptError::Load {
            path: exec_start.to_owned(),
            source: e.to_string(),
        })?;

        let engine = build_engine(self.max_operations);

        // Install bridge — cleared on drop
        let _guard = BridgeGuard::enter(host_api, event);

        let mut scope = Scope::new();

        // Serialize EventContext to Rhai Dynamic (requires rhai's `serde` feature)
        let event_dynamic =
            rhai::serde::to_dynamic(event).map_err(|e| ScriptError::Init(e.to_string()))?;
        scope.push("event", event_dynamic);

        let ast = engine
            .compile(&source)
            .map_err(|e| ScriptError::Compile(e.to_string()))?;

        let entry = entry_point.unwrap_or("main");
        let has_entry = ast.iter_functions().any(|f| f.name == entry);

        if has_entry {
            engine
                .call_fn::<Dynamic>(&mut scope, &ast, entry, ())
                .map(|_| ())
                .map_err(|e| ScriptError::Runtime(e.to_string()))
        } else if entry_point.is_none() {
            // No `main` defined — run top-level module code
            engine
                .run_ast_with_scope(&mut scope, &ast)
                .map_err(|e| ScriptError::Runtime(e.to_string()))
        } else {
            Err(ScriptError::EntryPointNotFound(entry.to_owned()))
        }
    }
}

impl Default for ScriptRunner {
    fn default() -> Self {
        Self::new(PathBuf::new())
    }
}

// ---------------------------------------------------------------------------
// Engine + bridge function registration
// ---------------------------------------------------------------------------

fn build_engine(max_ops: u64) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(max_ops);
    engine.set_max_string_size(1_048_576); // 1 MiB
    engine.set_max_array_size(4_096);
    engine.set_max_map_size(1_024);

    // --- ctx ---

    // Returns the current EventContext serialized as a JSON string.
    engine.register_fn("ctx_current_json", || -> String {
        with_bridge(|_, event| serde_json::to_string(event).unwrap_or_default()).unwrap_or_default()
    });

    // --- db.kv ---

    // Get a KV entry value JSON string, or empty string if not found.
    engine.register_fn(
        "db_kv_get",
        |scope_kind: String, scope_id: String, key: String| -> String {
            with_bridge(|host_api: &crate::host_api::HostApi, event| {
                let req = HostApiRequest::DbKvGet(DbKvGetRequest {
                    scope_kind,
                    scope_id,
                    key,
                });
                match host_api.call(event, req) {
                    Ok(resp) => {
                        if let HostApiValue::DbKvGet(val) = resp.value {
                            val.entry.map(|e| e.value_json).unwrap_or_default()
                        } else {
                            String::new()
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "script bridge: db_kv_get failed");
                        String::new()
                    }
                }
            })
            .unwrap_or_default()
        },
    );

    // Set a KV entry. Returns true on success.
    engine.register_fn(
        "db_kv_set",
        |scope_kind: String, scope_id: String, key: String, value_json: String| -> bool {
            with_bridge(|host_api: &crate::host_api::HostApi, event| {
                let entry = KvEntry {
                    scope_kind,
                    scope_id,
                    key,
                    value_json,
                    updated_at: chrono::Utc::now().to_rfc3339(),
                };
                let req = HostApiRequest::DbKvSet(DbKvSetRequest { entry });
                match host_api.call(event, req) {
                    Ok(_) => true,
                    Err(e) => {
                        tracing::warn!(error = %e, "script bridge: db_kv_set failed");
                        false
                    }
                }
            })
            .unwrap_or(false)
        },
    );

    // --- db.user ---

    // Get a user record as a JSON string, or empty string if not found.
    engine.register_fn("db_user_get_json", |user_id: i64| -> String {
        with_bridge(|host_api: &crate::host_api::HostApi, event| {
            let req = HostApiRequest::DbUserGet(DbUserGetRequest { user_id });
            match host_api.call(event, req) {
                Ok(resp) => {
                    if let HostApiValue::DbUserGet(val) = resp.value {
                        val.user
                            .map(|u| serde_json::to_string(&u).unwrap_or_default())
                            .unwrap_or_default()
                    } else {
                        String::new()
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "script bridge: db_user_get_json failed");
                    String::new()
                }
            }
        })
        .unwrap_or_default()
    });

    // --- logging ---

    engine.register_fn("unit_log", |msg: String| {
        tracing::info!(target: "unit_script", "{}", msg);
    });

    engine.register_fn("unit_warn", |msg: String| {
        tracing::warn!(target: "unit_script", "{}", msg);
    });

    // --- ml.health ---

    // Check ML server health. `base_url` can be empty string to use the configured default.
    // Returns JSON string of MlHealthValue, or empty string on error.
    engine.register_fn("ml_health_json", |base_url: String| -> String {
        with_bridge(|host_api: &crate::host_api::HostApi, event| {
            let req = HostApiRequest::MlHealth(MlHealthRequest {
                base_url: if base_url.is_empty() {
                    None
                } else {
                    Some(base_url)
                },
            });
            match host_api.call(event, req) {
                Ok(resp) => serde_json::to_string(&resp.value).unwrap_or_default(),
                Err(e) => {
                    tracing::warn!(error = %e, "script bridge: ml_health_json failed");
                    String::new()
                }
            }
        })
        .unwrap_or_default()
    });

    // --- ml.models ---

    // List available ML models. Returns JSON string of MlModelsValue.
    engine.register_fn("ml_models_json", |base_url: String| -> String {
        with_bridge(|host_api: &crate::host_api::HostApi, event| {
            let req = HostApiRequest::MlModels(MlModelsRequest {
                base_url: if base_url.is_empty() {
                    None
                } else {
                    Some(base_url)
                },
            });
            match host_api.call(event, req) {
                Ok(resp) => serde_json::to_string(&resp.value).unwrap_or_default(),
                Err(e) => {
                    tracing::warn!(error = %e, "script bridge: ml_models_json failed");
                    String::new()
                }
            }
        })
        .unwrap_or_default()
    });

    // --- templates ---

    engine.register_fn("load_template", |name: String| -> String {
        with_bridge(|host_api: &crate::host_api::HostApi, _| host_api.load_template(&name))
            .unwrap_or_default()
    });

    engine.register_fn("render_auto", |template_name: String| -> String {
        with_bridge(|host_api: &crate::host_api::HostApi, event| {
            let template = host_api.load_template(&template_name);
            let ctx = crate::host_api::template::TemplateContext::new().with_event(event);

            host_api.render_template(&template, ctx.into_map())
        })
        .unwrap_or_default()
    });

    // Transcribe a voice file.
    //
    // `base_url`: ML server base URL.
    // `file_id`: file identifier.
    //
    // Returns the transcript string, or empty string on error.
    engine.register_fn(
        "ml_transcribe",
        |base_url: String, file_id: String| -> String {
            with_bridge(|host_api: &crate::host_api::HostApi, event| {
                let req = HostApiRequest::MlTranscribe(MlTranscribeRequest {
                    base_url: if base_url.is_empty() {
                        None
                    } else {
                        Some(base_url)
                    },
                    file_id,
                });
                match host_api.call(event, req) {
                    Ok(resp) => {
                        if let HostApiValue::MlTranscribe(val) = resp.value {
                            val.text.unwrap_or_default()
                        } else {
                            String::new()
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "script bridge: ml_transcribe failed");
                        String::new()
                    }
                }
            })
            .unwrap_or_default()
        },
    );

    // --- tg.send_message ---

    // Send a Telegram message.
    //
    // `chat_id`: target chat identifier.
    // `text`: plain text message body.
    //
    // Returns the sent message id, or `0` on error.
    engine.register_fn("tg_send_message", |chat_id: i64, text: String| -> i64 {
        with_bridge(|host_api: &crate::host_api::HostApi, event| {
            let req = HostApiRequest::TgSendMessage(TgSendMessageRequest { chat_id, text });
            match host_api.call(event, req) {
                Ok(resp) => {
                    if let HostApiValue::TgSendMessage(val) = resp.value {
                        i64::from(val.message_id)
                    } else {
                        0
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "script bridge: tg_send_message failed");
                    0
                }
            }
        })
        .unwrap_or(0)
    });

    // --- ml.chat ---

    // Send a chat completion request to the ML server.
    //
    // `model`: model name (e.g. "llama3")
    // `messages`: Rhai array of maps, each with "role" and "content" keys.
    //
    // Returns the assistant's reply as a plain string, or empty string on error.
    engine.register_fn(
        "ml_chat",
        |model: String, messages: rhai::Array| -> String {
            with_bridge(|host_api: &crate::host_api::HostApi, event| {
                let messages: Vec<MlChatMessage> = messages
                    .into_iter()
                    .filter_map(|item| {
                        let map = item.try_cast::<rhai::Map>()?;
                        let role = map.get("role")?.clone().try_cast::<String>()?;
                        let content = map.get("content")?.clone().try_cast::<String>()?;
                        Some(MlChatMessage { role, content })
                    })
                    .collect();

                if messages.is_empty() {
                    tracing::warn!("script bridge: ml_chat called with no valid messages");
                    return String::new();
                }

                let req = HostApiRequest::MlChatCompletions(MlChatCompletionsRequest {
                    base_url: None,
                    model,
                    messages,
                    max_tokens: Some(1024),
                });
                match host_api.call(event, req) {
                    Ok(resp) => {
                        if let HostApiValue::MlChatCompletions(val) = resp.value {
                            val.content.unwrap_or_default()
                        } else {
                            String::new()
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "script bridge: ml_chat failed");
                        String::new()
                    }
                }
            })
            .unwrap_or_default()
        },
    );

    // --- ml.embed ---

    // Embed a single text string using the ML server.
    //
    // `model`: model name (e.g. "nomic-embed-text"), or empty string for server default.
    // `text`: the text to embed.
    //
    // Returns the embedding as a Rhai array of floats, or an empty array on error.
    engine.register_fn("ml_embed", |model: String, text: String| -> rhai::Array {
        with_bridge(|host_api: &crate::host_api::HostApi, event| {
            let req = HostApiRequest::MlEmbedText(MlEmbedTextRequest {
                base_url: None,
                model: if model.is_empty() { None } else { Some(model) },
                input: vec![text],
            });
            match host_api.call(event, req) {
                Ok(resp) => {
                    if let HostApiValue::MlEmbedText(val) = resp.value {
                        val.embeddings
                            .into_iter()
                            .next()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|f| rhai::Dynamic::from_float(f as rhai::FLOAT))
                            .collect()
                    } else {
                        rhai::Array::new()
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "script bridge: ml_embed failed");
                    rhai::Array::new()
                }
            }
        })
        .unwrap_or_default()
    });

    engine
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ScriptError {
    Load { path: String, source: String },
    Init(String),
    Compile(String),
    Runtime(String),
    EntryPointNotFound(String),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load { path, source } => {
                write!(f, "failed to load script `{path}`: {source}")
            }
            Self::Init(e) => write!(f, "script init error: {e}"),
            Self::Compile(e) => write!(f, "script compile error: {e}"),
            Self::Runtime(e) => write!(f, "script runtime error: {e}"),
            Self::EntryPointNotFound(name) => {
                write!(f, "entry point `{name}` not found in script")
            }
        }
    }
}

impl std::error::Error for ScriptError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventContext, ExecutionMode};
    use crate::host_api::HostApi;
    use crate::storage::Storage;
    use crate::unit::{
        CapabilitiesSpec, ServiceSpec, TriggerSpec, UnitDefinition, UnitManifest, UnitRegistry,
    };
    use std::path::Path;
    use tempfile::TempDir;

    fn test_event() -> EventContext {
        EventContext::synthetic_for_unit(
            "evt_script_runner",
            ExecutionMode::Manual,
            "moderation.test",
        )
    }

    fn test_host_api(allow: &[&str]) -> (TempDir, HostApi) {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage_path = dir.path().join("host-api.sqlite3");
        let storage = Storage::new(storage_path).init().expect("storage init");

        let mut manifest = UnitManifest::new(
            UnitDefinition::new("moderation.test"),
            TriggerSpec::command(["warn"]),
            ServiceSpec::new("cargo run"),
        );
        manifest.capabilities = CapabilitiesSpec {
            allow: allow.iter().map(|value| (*value).to_owned()).collect(),
            deny: Vec::new(),
        };
        let registry = UnitRegistry::load_manifests(vec![manifest]).registry;

        let api = HostApi::new(true)
            .with_storage(storage)
            .with_unit_registry(registry);
        (dir, api)
    }

    fn write_script(dir: &Path, name: &str, source: &str) -> String {
        let script_path = dir.join(name);
        std::fs::write(&script_path, source).expect("write script");
        name.to_owned()
    }

    #[test]
    fn script_runner_tg_send_message_bridge_uses_dry_run_host_api() {
        let (dir, api) = test_host_api(&["tg.write_message"]);
        let scripts_dir = dir.path().join("scripts");
        std::fs::create_dir_all(&scripts_dir).expect("create scripts dir");
        let runner = ScriptRunner::new(scripts_dir.clone());
        let script_name = write_script(
            scripts_dir.as_path(),
            "send.rhai",
            r#"
                let message_id = tg_send_message(-100123, "hello from rhai");
                if message_id != 1 {
                    throw("unexpected message_id");
                }
            "#,
        );

        runner
            .execute(&script_name, None, &test_event(), &api)
            .expect("script executes");
    }

    #[test]
    fn script_runner_ml_transcribe_bridge_uses_dry_run_host_api() {
        let (dir, api) = test_host_api(&["ml.stt"]);
        let scripts_dir = dir.path().join("scripts");
        std::fs::create_dir_all(&scripts_dir).expect("create scripts dir");
        let runner = ScriptRunner::new(scripts_dir.clone());
        let script_name = write_script(
            scripts_dir.as_path(),
            "transcribe.rhai",
            r#"
                let transcript = ml_transcribe("", "voice-123");
                if transcript != "transcribed text" {
                    throw("unexpected transcript");
                }
            "#,
        );

        runner
            .execute(&script_name, None, &test_event(), &api)
            .expect("script executes");
    }
}
