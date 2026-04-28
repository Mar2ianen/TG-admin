# telegram-moderation-os

A Telegram moderation bot runtime with built-in command execution and unit-driven scripting via Rhai.

---

## Status

Built-in moderation is working end-to-end: the runtime ingests live Telegram updates, classifies them, routes them through an indexed dispatcher, and executes moderation commands (`/warn`, `/mute`, `/ban`, `/del`, `/undo`, `/msg`) with full audit trail and replay safety.

Unit scripting is available: unit manifests are loaded on startup and participate in routing and indexing. Rhai script execution is supported for command-trigger units. Full hot-reload and advanced scheduler integration are post-MVP work.

---

## Quick Start

**Prerequisites**

- Rust 1.85+
- A Telegram bot token ([@BotFather](https://t.me/BotFather))

**Setup**

```sh
cp config.example.toml config.toml
```

Edit `config.toml` — at minimum set:

```toml
[telegram]
bot_token = "YOUR_BOT_TOKEN_HERE"
admin_user_ids = [YOUR_TELEGRAM_USER_ID]
```

For local/offline runs, set `telegram.polling = false`. For live polling without
storing the token in `config.toml`, export `TMO_BOT_TOKEN` and leave
`bot_token` empty.

**Run**

```sh
cargo run --release
```

The bot only starts live polling when `telegram.polling = true` and a non-empty
token is available in `config.toml` or `TMO_BOT_TOKEN`. With `polling = false`,
the runtime stays in local/noop mode.

For deployment details, see [`docs/DEPLOY_RUNBOOK.md`](docs/DEPLOY_RUNBOOK.md).

---

## Configuration

All fields live in `config.toml`. The full annotated example is in [`config.example.toml`](config.example.toml).

| Field | Description |
|---|---|
| `telegram.bot_token` | Required for live polling unless `TMO_BOT_TOKEN` is set. Leave empty with `telegram.polling = false` for local/noop mode. |
| `telegram.admin_user_ids` | List of Telegram user IDs permitted to run moderation commands. |
| `paths.units_dir` | Directory scanned for unit manifest `.toml` files on startup. |
| `paths.scripts_dir` | Directory containing `.rhai` scripts referenced by unit manifests. |
| `paths.database_path` | Path to the SQLite database file. Created automatically if absent. |
| `storage.sqlite_journal_mode` | `WAL` is recommended for concurrent read/write workloads. |
| `observability.log_level` | `info`, `debug`, `warn`, or `error`. Overridable via `RUST_LOG`. |
| `observability.json_logs` | `true` emits newline-delimited JSON; `false` emits compact human-readable text. |

---

## Built-in Commands

All commands are admin-only. Target a user by replying to their message or by including a `@username` / user ID in the command.

| Command | Description |
|---|---|
| `/warn <target> <reason>` | Record a warning against the user. |
| `/mute <target> <duration> <reason>` | Restrict the user for a duration (e.g. `30m`, `2h`, `7d`). |
| `/ban <target> <reason>` | Ban the user from the chat. |
| `/del [target] [window]` | Delete the replied-to message, or a batch of recent messages from the target. |
| `/undo` | Compensate the last reversible action (unmute, unban, restore). |
| `/msg <text>` | Send a plain text message. Also usable as a pipe target: `/mute @user 30m spam \| /msg "You have been muted."` |

---

## Unit Manifests

Place `.toml` manifest files in `paths.units_dir`. Each manifest declares what triggers a unit, what script it runs, and what capabilities it requires.

Minimal example (`units/my_unit.toml`):

```toml
[Unit]
Name = "my.unit"

[Trigger]
Type = "command"
Commands = ["stats"]

[Service]
ExecStart = "scripts/my_unit.rhai"

[Capabilities]
Allow = ["db.kv.read", "db.kv.write"]
```

The runtime loads all manifests on startup, builds routing indexes from their trigger definitions, and dispatches matching events to the corresponding script.

---

## Rhai Scripts

Scripts receive the event context through a set of built-in globals and functions.

| Symbol | Description |
|---|---|
| `event` | Map containing the current event context: `chat`, `sender`, `message`, `update_id`, etc. |
| `ctx_current_json()` | Returns the full serialised event context as a JSON string. |
| `db_kv_get(scope_kind, scope_id, key)` | Read a value from the KV store. Returns the stored string or `()`. |
| `db_kv_set(scope_kind, scope_id, key, value)` | Write a value to the KV store. Returns `true` on success. |
| `db_user_get_json(user_id)` | Return the user record for `user_id` as a JSON string, or `()` if not found. |
| `unit_log(msg)` | Emit a structured `info`-level log line tagged with the unit name. |
| `unit_warn(msg)` | Emit a structured `warn`-level log line tagged with the unit name. |

---

## Architecture

Incoming Telegram updates are picked up by the polling ingest loop and passed to `IngressPipeline`, which normalises each raw update into an `EventContext`. The pipeline then classifies the event along three orthogonal axes — ingress class (`realtime`, `recovery`, `scheduled`, `manual`), update traits (`text`, `callback`, `photo`, `voice`, `reply`, `job`), and command index (`warn`, `mute`, `ban`, `del`, `undo`, `msg`) — and produces a dispatch set. `ExecutionRouter` resolves the dispatch set against startup-built handler indexes and forwards the event to either the built-in moderation lane (handled by `ModerationEngine` with full audit and undo semantics) or the unit-driven lane (which loads the matched manifest, checks capabilities, and executes the associated Rhai script). Every side-effecting action is journalled before execution; duplicate updates are detected and skipped via a replay guard.

---

## Development

```sh
cargo test
```

The test suite runs 217+ unit and integration tests covering the ingress pipeline, moderation engine, router index construction, audit correctness, and dry-run / replay semantics.
