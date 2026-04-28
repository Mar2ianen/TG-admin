# Stabilization Roadmap And Agent Prompts

## Назначение

Этот документ фиксирует план стабилизации текущего checkout перед продолжением новых фич.

Главный принцип:

- сначала зеленая сборка;
- затем закрытие runtime boundary;
- затем unit/script execution;
- только после этого voice/BERT/templates как продуктовые фичи.

План основан на проверке текущего дерева:

- `cargo check --lib --all-features` проходит;
- `cargo check --all-targets --all-features` падает на тестах/бенчах из-за устаревших фикстур и старой сигнатуры `ExecutionRouter::new`;
- часть прежних замечаний уже неактуальна для текущего checkout: `HostApi` синтаксически собирается, `TriggerSpec::EventType { events }` уже используется, `MemberJoined` есть в `UnitEventType`, `sync_registry` уже пересобирает индекс;
- реальные открытые проблемы: all-targets сборка, capability gaps, unit execution path, template path config, scheduler recovery/dedupe, persistent Telegram idempotency, CI gate.

## Roadmap

## P0. Вернуть зеленый `cargo check --all-targets`

Цель:

- `cargo check --all-targets --all-features` проходит без ошибок.

Scope:

- обновить тестовые/bench фикстуры под текущие `ChatContext` и `SenderContext`;
- обновить старые вызовы `ExecutionRouter::new()` на текущую сигнатуру `ExecutionRouter::new(bot_id, delete_unknown_commands)`;
- не менять runtime поведение.

Основные файлы:

- `benches/parsing.rs`
- `tests/snapshots_test.rs`
- `tests/json_perf_test.rs`
- `src/router/tests.rs`

Validation:

```bash
cargo check --all-targets --all-features
```

Exit criteria:

- нет compile errors;
- если остаются warnings, они не блокируют этот этап.

## P1. Host API capability и template paths

Цель:

- destructive/script-visible операции не обходят capability model;
- template loading не зависит от текущего working directory.

Scope:

- `HostApiOperation::MlTranscribe` требует `ml.stt`;
- `HostApiOperation::TgSendMessage` требует `tg.write_message`;
- добавить builder для Telegram gateway, если нужен тестовый/custom gateway;
- передать `config.paths.templates_dir` в `HostApi`;
- оставить `bundled_templates` как fallback, но не привязывать custom templates к cwd.

Основные файлы:

- `src/host_api.rs`
- `src/host_api/validation.rs`
- `src/runtime.rs`
- `src/config.rs` при необходимости
- `src/host_api/tests*.rs`

Validation:

```bash
cargo test host_api --all-features
cargo check --all-targets --all-features
```

Exit criteria:

- tests подтверждают denial без capability;
- tests подтверждают allow с `ml.stt` и `tg.write_message`;
- template lookup использует configured path.

## P2. Замкнуть минимальный unit execution path

Цель:

- `Event -> Router -> UnitDispatchInvocation -> ScriptRunner -> HostApi` работает хотя бы для одного сухого Rhai unit.

Scope:

- перестать хранить `HostApi` только как неиспользуемый аргумент в `with_script_runner`;
- сохранить `HostApi` внутри router или выделить малый executor рядом с router;
- при `ExecutionLane::UnitDispatch` выполнять matching invocations через `ScriptRunner::execute`;
- built-in moderation path не ломать;
- errors from scripts должны возвращаться как routing/execution error или фиксироваться явно, без silent success.

Ограничение:

- не добавлять voice/update_trait в этом этапе;
- не переписывать весь router;
- не делать async Rhai redesign.

Основные файлы:

- `src/router/mod.rs`
- `src/router/types.rs`
- `src/script.rs`
- `src/runtime.rs`
- `src/router/tests.rs`

Validation:

```bash
cargo test router --all-features
cargo check --all-targets --all-features
```

Exit criteria:

- есть тест, где unit dispatch реально вызывает тестовый script;
- dry-run `tg_send_message` path можно будет добавить после регистрации bridge function;
- built-in `/warn` route tests продолжают проходить.

## P3. Script API contract cleanup

Цель:

- зарегистрированные Rhai-функции совпадают с Host API contract и capability model.

Scope:

- добавить Rhai bridge `tg_send_message(chat_id, text)`;
- убедиться, что `ml_transcribe(base_url, file_id)` требует `ml.stt`;
- зафиксировать контракт доступа к event через `ctx_current_json` или documented Rhai map fields;
- не добавлять scripts, которые обращаются к несуществующим `event.message.voice.file_id`.

Основные файлы:

- `src/script.rs`
- `src/host_api.rs`
- `src/host_api/contract.rs`
- `src/script/tests.rs` или существующий test module

Validation:

```bash
cargo test script --all-features
cargo check --all-targets --all-features
```

Exit criteria:

- Rhai script с `tg_send_message` компилируется и возвращает dry-run result через Host API;
- failure path при отсутствии capability покрыт тестом.

## P4. Scheduler recovery и dedupe

Цель:

- jobs не зависают навсегда в `processing`;
- `dedupe_key` реально защищает replay/idempotency.

Scope:

- добавить partial unique index по `jobs(dedupe_key) WHERE dedupe_key IS NOT NULL`;
- заменить обычный `insert_job` на conflict-aware behavior;
- сделать recovery method для старых `processing` jobs;
- claim jobs атомарно или хотя бы conditionally update `scheduled -> processing`;
- начать использовать `retry_count` / `max_retries` в явной модели.

Ограничение:

- не переписывать весь scheduler в отдельный service;
- не добавлять distributed locking.

Основные файлы:

- `src/storage/schema.rs`
- `src/storage/connection.rs`
- `src/runtime.rs`
- `src/storage/tests.rs`

Validation:

```bash
cargo test storage::tests::jobs --all-features
cargo test runtime --all-features
cargo check --all-targets --all-features
```

Exit criteria:

- duplicate dedupe key не создает второй job;
- stale processing job возвращается в scheduled или retry_wait;
- claim не перетирает job, уже claimed другим tick.

## P5. Persistent Telegram idempotency

Цель:

- destructive Telegram effects не повторяются после restart/replay.

Scope:

- оставить in-memory cache как ускоритель;
- добавить storage-backed source of truth через `audit_log` или новую таблицу `external_effects`;
- перед Telegram call проверять idempotency key;
- после successful call сохранять result;
- при replay возвращать сохраненный result.

Ограничение:

- не менять public `TelegramRequest` shape без необходимости;
- не смешивать audit compensation и transport cache, если выбран отдельный `external_effects`.

Основные файлы:

- `src/tg/mod.rs`
- `src/tg/types.rs`
- `src/storage/schema.rs`
- `src/storage/connection.rs`
- `src/moderation/*`
- `src/tg/tests.rs`

Validation:

```bash
cargo test tg --all-features
cargo test moderation --all-features
cargo check --all-targets --all-features
```

Exit criteria:

- replay после создания нового `TelegramGateway` возвращает saved result;
- destructive commands сохраняют idempotency key до/после call в понятном порядке;
- existing in-memory behavior не ломается.

## P6. CI gate

Цель:

- main больше не принимает код, который не проходит базовые Rust gates.

Scope:

- добавить GitHub Actions workflow;
- команды: fmt, clippy, test;
- если текущий clippy слишком красный из-за warnings, сначала поставить `cargo check`/`cargo test`, а `clippy -D warnings` включить после cleanup.

Основные файлы:

- `.github/workflows/ci.yml`

Validation:

```bash
cargo fmt --check
cargo test --all-targets --all-features
```

Exit criteria:

- локальные команды совпадают с CI;
- workflow не требует секретов.

## Suggested Agent Order

Параллельно можно запускать только задачи с непересекающимися write scopes.

Первый batch:

- Agent 1: P0 all-targets compile fixtures.
- Agent 2: P1 Host API capabilities/templates.

После Agent 1:

- Agent 3: P6 CI gate.

После Agent 2:

- Agent 4: P3 Script API bridge cleanup.

После P0/P1:

- Agent 5: P2 Unit execution path.

Отдельно, после зеленого P0:

- Agent 6: P4 Scheduler recovery/dedupe.
- Agent 7: P5 Persistent Telegram idempotency.

## Agent Prompt 1: P0 all-targets compile fixtures

Model: `gpt-5.4-mini`

```text
You are a gpt-5.4-mini coding subagent working in /home/arch/Документы/Teloxide.

Task: make `cargo check --all-targets --all-features` compile by fixing stale tests and benches only.

Context:
- `cargo check --lib --all-features` already passes.
- Current all-targets errors are stale fixture/API errors, not runtime behavior failures.
- Known issues:
  - `ChatContext` initializers in tests/benches miss `photo_file_id`.
  - `SenderContext` initializers in tests/benches miss `first_name`, `last_name`, `photo_file_id`.
  - some tests still call `ExecutionRouter::new()` but current signature is `ExecutionRouter::new(bot_id: i64, delete_unknown_commands: bool)`.

Ownership:
- You may edit only test/bench files unless a tiny test helper in src is clearly needed.
- Primary files:
  - benches/parsing.rs
  - tests/snapshots_test.rs
  - tests/json_perf_test.rs
  - src/router/tests.rs

Constraints:
- Do not change production runtime behavior.
- Do not remove tests just to make compilation pass.
- Do not revert unrelated user changes.
- Prefer realistic default fixture values: `None` for optional names/photos unless the test needs a value.

Validation:
- Run `cargo check --all-targets --all-features`.

Final response:
- List changed files.
- Include the final command result.
- Mention any remaining warnings but do not try to clean unrelated warnings.
```

## Agent Prompt 2: P1 Host API capabilities and template paths

Model: `gpt-5.4-mini`

```text
You are a gpt-5.4-mini coding subagent working in /home/arch/Документы/Teloxide.

Task: close Host API capability gaps for `MlTranscribe` and `TgSendMessage`, and make template loading use configured paths instead of cwd-relative `templates`.

Current facts to verify in code:
- `required_capability` currently returns `None` for `HostApiOperation::MlTranscribe | HostApiOperation::TgSendMessage`.
- valid capabilities include `ml.stt` and `tg.write_message`.
- `HostApi::load_template` currently reads from `templates/*.txt` and `bundled_templates/*.txt`.
- `PathsConfig` already has `templates_dir`.

Ownership:
- Primary files:
  - src/host_api.rs
  - src/host_api/validation.rs
  - src/runtime.rs
  - src/host_api/tests*.rs
  - src/config.rs only if needed for a clean path type

Required behavior:
- `MlTranscribe` requires `ml.stt`.
- `TgSendMessage` requires `tg.write_message`.
- `HostApi` can be built with configured `templates_dir`.
- Existing bundled template fallback still works.
- Add focused tests for capability allow/deny and configured template lookup.

Constraints:
- Keep changes small.
- Do not redesign HostApi visibility or all response types in this task.
- Do not touch scheduler/router execution.
- Do not revert unrelated user changes.

Validation:
- Run `cargo test host_api --all-features`.
- Run `cargo check --all-targets --all-features` if P0 is already green; otherwise run `cargo check --lib --all-features`.

Final response:
- List changed files.
- Describe behavior added.
- Include exact validation commands and results.
```

## Agent Prompt 3: P3 Script API bridge cleanup

Model: `gpt-5.4-mini`

```text
You are a gpt-5.4-mini coding subagent working in /home/arch/Документы/Teloxide.

Task: make the Rhai ScriptRunner bridge match the Host API contract for Telegram send and ML transcribe.

Prerequisite:
- Host API capabilities should already require `tg.write_message` for `TgSendMessage` and `ml.stt` for `MlTranscribe`.

Current facts to verify in code:
- `script.rs` registers `ml_transcribe(base_url, file_id)`.
- `script.rs` does not register `tg_send_message(chat_id, text)`.
- `HostApiRequest::TgSendMessage` and `TgSendMessageRequest` already exist.

Ownership:
- Primary files:
  - src/script.rs
  - script-related tests, if present
  - src/host_api/tests*.rs only if a small test helper is needed

Required behavior:
- Register `tg_send_message(chat_id: i64, text: String) -> i64` or a similarly simple return value.
- The function must call `HostApiRequest::TgSendMessage`.
- Errors should log and return a neutral value, consistent with existing bridge functions.
- Add a focused test that executes a tiny Rhai script using `tg_send_message` in dry-run mode.
- Add or update a test for `ml_transcribe` if current coverage is missing and easy.

Constraints:
- Do not implement full unit execution path here.
- Do not add voice/event schema support.
- Do not change `ml_chat` signature in this task unless tests prove it is broken.
- Do not revert unrelated user changes.

Validation:
- Run `cargo test script --all-features` if a script test target/module exists.
- Run `cargo check --all-targets --all-features` if P0 is green; otherwise run `cargo check --lib --all-features`.

Final response:
- List changed files.
- Include exact Rhai function signatures added/verified.
- Include validation commands and results.
```

## Agent Prompt 4: P2 Minimal unit execution path

Model: `gpt-5.4-mini`

```text
You are a gpt-5.4-mini coding subagent working in /home/arch/Документы/Teloxide.

Task: connect the minimal runtime path from router unit dispatch to ScriptRunner execution.

Current facts to verify in code:
- `ExecutionRouter::with_script_runner(runner, host_api)` currently stores the runner but ignores the host_api argument.
- `ExecutionRouter::route()` selects `UnitDispatchInvocation`s.
- For `ExecutionLane::UnitDispatch`, route currently returns `ExecutionOutcome::UnitDispatch { plan, invocations }` without executing scripts.
- Built-in moderation path must continue to work.

Ownership:
- Primary files:
  - src/router/mod.rs
  - src/router/types.rs
  - src/router/tests.rs
  - src/runtime.rs only if construction needs a small adjustment

Required behavior:
- Store enough executor state to call `ScriptRunner::execute(exec_start, entry_point, event, host_api)`.
- When unit dispatch lane is selected and a script runner is configured, execute matching invocations.
- Return an outcome that still exposes plan/invocations for tests/observability.
- If no script runner is configured, preserve the existing planning/deferred behavior or return an explicit missing-executor error, whichever matches local tests better.
- Add a test with a temp scripts dir and a tiny Rhai script that proves route executes the script.

Constraints:
- Do not implement update_trait/voice triggers here.
- Do not make HostApi async.
- Do not rewrite router architecture.
- Do not change built-in moderation command semantics.
- Do not revert unrelated user changes.

Validation:
- Run `cargo test router --all-features`.
- Run `cargo check --all-targets --all-features` if P0 is green; otherwise run `cargo check --lib --all-features`.

Final response:
- List changed files.
- Explain how unit dispatch now executes.
- Include validation commands and results.
```

## Agent Prompt 5: P4 Scheduler recovery and dedupe

Model: `gpt-5.4-mini`

```text
You are a gpt-5.4-mini coding subagent working in /home/arch/Документы/Teloxide.

Task: make scheduler job dedupe and crash recovery minimally safe.

Current facts to verify in code:
- `jobs` has `dedupe_key`, `retry_count`, and `max_retries`.
- schema currently lacks a unique partial index on `dedupe_key`.
- `insert_job` does a plain INSERT.
- scheduler polls due jobs, then separately sets status to `processing`.
- processing jobs can remain stuck after a crash.

Ownership:
- Primary files:
  - src/storage/schema.rs
  - src/storage/connection.rs
  - src/storage/tests.rs
  - src/runtime.rs if scheduler startup/tick needs recovery call

Required behavior:
- Add migration for unique partial index:
  `CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_dedupe_key ON jobs(dedupe_key) WHERE dedupe_key IS NOT NULL;`
- Make inserting a duplicate dedupe key deterministic: return existing job or surface a typed storage error that HostApi can handle.
- Add a method to recover stale `processing` jobs back to `scheduled` or another explicit retry state.
- Improve claim to avoid blindly marking a job if status changed since polling.
- Add storage tests for duplicate dedupe key and stale processing recovery.

Constraints:
- Keep this as an MVP recovery fix, not a full scheduler rewrite.
- Do not introduce external queues or distributed locks.
- Do not revert unrelated user changes.

Validation:
- Run `cargo test storage --all-features`.
- Run `cargo check --all-targets --all-features` if P0 is green; otherwise run `cargo check --lib --all-features`.

Final response:
- List changed files.
- Describe dedupe behavior precisely.
- Include validation commands and results.
```

## Agent Prompt 6: P5 Persistent Telegram idempotency design spike

Model: `gpt-5.4-mini`

```text
You are a gpt-5.4-mini coding subagent working in /home/arch/Документы/Teloxide.

Task: prepare a narrow implementation plan for persistent Telegram idempotency, and implement only if the required storage boundary is obvious and small.

Current facts to verify in code:
- `TelegramGateway` currently keeps `idempotency_cache: Arc<Mutex<HashMap<String, TelegramResult>>>`.
- That cache does not survive restart.
- `audit_log` already has `idempotency_key`.

Ownership:
- If implementation is small:
  - src/tg/mod.rs
  - src/tg/types.rs
  - src/storage/schema.rs
  - src/storage/connection.rs
  - src/tg/tests.rs
- If implementation is not small, create a focused doc under docs/ and do not half-implement.

Required output:
- Decide between using `audit_log` as source of truth or adding `external_effects`.
- Explain the exact order:
  1. check persisted idempotency key;
  2. execute Telegram request only if absent;
  3. persist result;
  4. return persisted result on replay.
- If implementing, add a test proving replay works across a fresh `TelegramGateway` instance.

Constraints:
- Do not rewrite all moderation audit logic.
- Do not change public Telegram request/result contracts unless required.
- Do not remove in-memory cache; it may remain as an accelerator.
- Do not revert unrelated user changes.

Validation:
- If implementation: run `cargo test tg --all-features`.
- If doc-only: run `cargo check --lib --all-features` to ensure no accidental code changes broke build.

Final response:
- State whether this was implemented or documented only.
- List changed files.
- Include validation commands and results.
```

## Agent Prompt 7: P6 CI gate

Model: `gpt-5.4-mini`

```text
You are a gpt-5.4-mini coding subagent working in /home/arch/Документы/Teloxide.

Task: add a minimal GitHub Actions CI workflow matching the current local stabilization gates.

Prerequisite:
- Prefer doing this after `cargo check --all-targets --all-features` is green.

Ownership:
- You may edit only:
  - .github/workflows/ci.yml
  - docs/STABILIZATION_ROADMAP_AND_AGENT_PROMPTS.md only if you need to update the listed CI commands

Required behavior:
- Run on push and pull_request.
- Use stable Rust.
- Run:
  - `cargo fmt --check`
  - `cargo test --all-targets --all-features`
- Add `cargo clippy --all-targets --all-features -- -D warnings` only if it currently passes locally. If it does not pass, leave a TODO comment or separate follow-up note rather than adding permanently red CI.

Constraints:
- Do not add secrets.
- Do not add deployment.
- Do not change Rust code.
- Do not revert unrelated user changes.

Validation:
- Run `cargo fmt --check`.
- Run `cargo test --all-targets --all-features` if practical.

Final response:
- List changed files.
- State whether clippy was included or deferred and why.
- Include validation commands and results.
```

## Controller Notes

Перед запуском каждого субагента полезно дать ему текущий вывод:

```bash
cargo check --all-targets --all-features
git status --short
```

После каждого субагента:

```bash
git diff --stat
cargo check --all-targets --all-features
```

Правило интеграции:

- сначала принимать патчи, которые уменьшают красную сборку;
- не смешивать scheduler/idempotency с router/script execution в одном review;
- если субагент предлагает широкий rewrite, отклонять и возвращать к ownership scope из prompt.
