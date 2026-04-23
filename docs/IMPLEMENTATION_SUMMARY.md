# Implementation Summary

## Контекст

Этот документ фиксирует реальное состояние репозитория на `2026-04-22`.

Здесь важно разделять:

- что уже собрано в коде
- что является целевой архитектурой
- какие инварианты нельзя потерять при рефакторе

Главная поправка к прежней формулировке: в проекте уже есть сильный vertical slice moderation, `Runtime`, `ExecutionRouter`, `IngressPipeline` и polling-based live runtime path, но полноценный unit-aware runtime router по целевой модели еще не собран.

## Текущее состояние коротко

В коде уже есть:

- compile-safe crate и runtime bootstrap
- typed config и observability bootstrap
- `EventContext` и normalizers для `manual`, `scheduled`, `telegram`
- parser/dispatch stack для moderation DSL
- typed unit manifests, validation и registry
- SQLite storage baseline с audit/jobs/message journal/processed updates
- typed Host API surface
- typed Telegram gateway contract c dry-run и idempotency shell
- built-in moderation slice для `/warn`, `/mute`, `/ban`, `/del`, `/undo`, `/msg`

Дополнительно уже закрыт важный bugfix-срез:

- fail-closed capability checks в moderation и Host API
- replay-safe `processed_updates` с `pending/completed`
- fail-fast для явного `TMO_CONFIG`
- реальное применение storage config из `AppConfig`
- startup storage bootstrap в `Application`
- более строгая нормализация `manual/scheduled` inputs
- `audit.compensate` больше не делает тихий success на битом recipe или повторной компенсации

Проверки на текущем состоянии:

- `cargo test` зеленый
- `cargo clippy --all-targets --all-features -- -D warnings` зеленый

## Что уже собрано по слоям

## Application lifecycle layer

Собрано:

- `main -> config -> logging -> Application`
- startup/shutdown lifecycle
- startup-time bootstrap storage
- fail-fast по явному config path и невалидному storage config

Основные файлы:

- [src/main.rs](/home/arch/Документы/Teloxide/src/main.rs:1)
- [src/app.rs](/home/arch/Документы/Teloxide/src/app.rs:1)
- [src/config.rs](/home/arch/Документы/Teloxide/src/config.rs:1)
- [src/observability.rs](/home/arch/Документы/Teloxide/src/observability.rs:1)
- [src/shutdown.rs](/home/arch/Документы/Teloxide/src/shutdown.rs:1)

Важно:

`Application` остается lifecycle shell и делегирует execution graph в `Runtime`. При этом runtime пока поднимает пустой `UnitRegistry::default()`, без bootstrap из `config.paths.units_dir`.

## Event and parser layer

Собрано:

- единый `EventContext`
- строгие event invariants
- normalizers для `manual`, `scheduled`, `telegram`
- parser для moderation-команд
- alias expansion
- dispatch layer поверх normalized event

Текущий входной shape, который уже виден в коде:

- `telegram` normalizer покрывает live Telegram-origin events
- `manual` normalizer покрывает synthetic manual invoke
- `scheduled` normalizer покрывает synthetic scheduler/job path
- `recovery` пока выражается через `ExecutionMode::Recovery` и `SystemOrigin::RecoveryReplay`, но еще не собран как отдельный runtime classifier/index lane

Основные файлы:

- [src/event.rs](/home/arch/Документы/Teloxide/src/event.rs:1)
- [src/parser/command.rs](/home/arch/Документы/Teloxide/src/parser/command.rs:1)
- [src/parser/duration.rs](/home/arch/Документы/Teloxide/src/parser/duration.rs:1)
- [src/parser/target.rs](/home/arch/Документы/Teloxide/src/parser/target.rs:1)
- [src/parser/reason.rs](/home/arch/Документы/Teloxide/src/parser/reason.rs:1)
- [src/parser/dispatch.rs](/home/arch/Документы/Teloxide/src/parser/dispatch.rs:1)

## Unit layer

Собрано:

- typed `UnitManifest`
- TOML loading
- dependency validation
- capability validation
- in-memory registry
- safe reload semantics

Основной файл:

- [src/unit.rs](/home/arch/Документы/Teloxide/src/unit.rs:1)

Важно:

registry и manifests пока существуют как отдельный слой, но еще не встроены в runtime routing path.

## Storage layer

Собрано:

- SQLite bootstrap
- schema versioning
- typed accessors
- `users`
- `kv_store`
- `message_journal`
- `jobs`
- `audit_log`
- `processed_updates`

Дополнительно уже есть:

- `processed_updates.status = pending|completed`
- более безопасная replay/dedupe semantics

Основной файл:

- [src/storage.rs](/home/arch/Документы/Teloxide/src/storage.rs:1)

## Host API layer

Собрано:

- `ctx.current`
- `ctx.resolve_target`
- `ctx.parse_duration`
- `ctx.expand_reason`
- `db.user_get`
- `db.user_patch`
- `db.user_incr`
- `db.kv_get`
- `db.kv_set`
- `msg.window`
- `msg.by_user`
- `job.schedule_after`
- `audit.find`
- `audit.compensate`
- `unit.status`

Отдельно важно:

- capability checks уже fail-closed
- validation errors уже structured
- dry-run semantics уже часть surface, а не декоративный флаг

Основной файл:

- [src/host_api.rs](/home/arch/Документы/Teloxide/src/host_api.rs:1)

## Telegram layer

Собрано:

- typed `TelegramRequest`
- typed `TelegramResult`
- request validation
- dry-run prediction
- idempotency cache для destructive ops

Основной файл:

- [src/tg.rs](/home/arch/Документы/Teloxide/src/tg.rs:1)

Важно:

живой transport уже подключается через `teloxide-core`, когда задан `telegram.bot_token`. Без токена gateway остается `noop`.

## Built-in moderation layer

Собрано:

- локальный command route:
  `EventContext -> dispatch -> ParsedCommandLine -> ExpandedCommandLine -> ModerationEngine`
- built-in execution для `/warn`, `/mute`, `/ban`, `/del`, `/undo`, `/msg`
- audit trail
- compensation recipes
- replay guard через `processed_updates`

Основной файл:

- [src/moderation.rs](/home/arch/Документы/Teloxide/src/moderation.rs:1)

Важно:

это рабочий локальный built-in route и текущий executor для moderation-команд, но он еще не является верхним runtime router.

## Как routing работает сейчас

На текущем коде есть уже верхний runtime path и built-in execution path:

1. `Application` поднимает lifecycle и делегирует startup/run/shutdown в `Runtime`.
2. `Runtime` собирает `ExecutionRouter`, при наличии `bot_token` поднимает `IngressPipeline` с polling ingest loop и маршрутизирует live updates в router.
3. `ModerationEngine` остается основной реально работающей built-in execution lane для moderation-команд.

То есть текущий working path такой:

`Telegram update -> IngressPipeline -> EventContext -> ExecutionRouter -> parser/dispatch -> ModerationEngine -> built-in moderation execution -> storage/tg/audit/jobs`

А вот такого пути пока нет end-to-end:

`config.paths.units_dir -> registry bootstrap -> unit-aware dispatch set -> actual unit execution lane`

Это ключевая граница между тем, что уже собрано, и тем, что еще предстоит собрать.

## Целевая routing model

Целевая модель проекта больше не должна описываться как плоский router, который перебирает все юниты подряд.

Целевая модель:

- `Indexed Execution Router`
- или `Bucketed Router`

Смысл модели:

1. на startup или reload runtime читает manifests и built-in descriptors
2. строит индексы обработчиков по типам событий и trigger traits
3. при приходе события сначала делает cheap classification
4. получает не один handler, а `dispatch set` релевантных buckets
5. исполняет только подходящие endpoint groups

В текущем коде cheap classification, router и live ingress path уже есть, но unit manifests не bootstrap-ятся из `config.paths.units_dir`, а unit execution lane пока не исполняется end-to-end.

Это не “одна корзина на событие”.

Правильная модель:

`event classification -> relevant bucket set -> executor selection`

## Какие buckets считаются целевыми

Минимальный целевой набор:

- ingress class:
  - realtime
  - recovery
  - scheduled
  - manual
- update traits:
  - text
  - callback
  - photo
  - voice
  - job
- command index:
  - `warn`
  - `mute`
  - `ban`
  - `del`
  - `undo`
  - `msg`
- execution lane:
  - built-in moderation
  - unit-driven scriptable path

Команда при этом не должна считаться отдельным transport-level update type.

Команда — это отдельный индекс поверх:

- text message
- callback payload
- возможно scheduled/manual command text

Ingress class при этом тоже не равен transport type. Для текущего проекта полезная модель такая:

- `realtime` = live Telegram-origin update
- `recovery` = replay того же Telegram-origin потока
- `scheduled` = synthetic scheduler/job input
- `manual` = synthetic manual invoke

## Что еще не собрано

На `2026-04-22` в коде отсутствуют:

- реальный Telegram ingestion loop
- runtime-level event classification
- bucket index build на startup/reload
- `event -> dispatch set` router
- unit bootstrap из `config.paths.units_dir`
- явный `event -> unit -> execution envelope`
- живая граница built-in vs scriptable execution
- production Telegram transport поверх `teloxide-core`

То есть bucketed/indexed router уже выбран как целевая модель, но пока не реализован.

## Ключевые архитектурные решения, которые уже приняты

## 1. Event-first flow

Любой внешний вход должен сначала становиться `EventContext`.

Это уже нельзя откатывать назад.

## 2. Typed parser вместо ad-hoc handling

Команды не должны исполняться прямо из raw text.

Сохраняем цепочку:

- raw source
- `ParsedCommandLine`
- `ExpandedCommandLine`
- execution layer

## 3. Dry-run как first-class behavior

`dry_run` влияет на:

- Host API
- Telegram execution
- moderation execution
- side effects

Это не “не дергать transport”, а полноценный execution mode c предсказанным typed result.

## 4. Replay safety и tg idempotency — разные вещи

Нельзя сливать:

- tg-level idempotency
- storage-level replay safety

Они решают разные задачи.

## 5. Audit — это business contract

`audit_log` уже влияет на поведение `/undo`.

Значит audit schema нельзя рассматривать как просто лог.

## 6. Routing должен быть индексированным, а не плоским

Это новая целевая архитектурная фиксация:

- не global scan по всем units
- не хаотичный `if/else` growth
- а cheap classification + prebuilt indexes

При этом отдельно фиксируем:

- `command` — это command index
- `text/callback/photo/voice/job` — это update traits
- `realtime/recovery/scheduled/manual` — это ingress classes

## Что нельзя потерять при рефакторе

## Event invariants

Нельзя размыть проверки в [src/event.rs](/home/arch/Документы/Teloxide/src/event.rs:1):

- `event_id` не пустой
- `reply` невозможен без `message`
- `message` невозможен без `chat`
- callback/job shape совпадает с `update_type`
- `SystemOrigin` соответствует `execution_mode`
- manual/scheduled normalization остается строгой

## Parser contract

Нельзя ломать без отдельной миграции:

- AST shape
- target resolution order
- `reply` fallback
- `-user` selector semantics
- pipe support только для `mute`

## Storage contract

Нельзя молча менять смысл полей:

- `jobs.dedupe_key`
- `jobs.audit_action_id`
- `audit_log.compensation_json`
- `audit_log.trigger_message_id`
- `processed_updates.update_id`
- `processed_updates.status`

## Moderation semantics

Нельзя потерять:

- `warn` как reversible action
- `mute` как reversible action
- `del` как non-reversible action
- `undo` как audit-driven compensation path
- replay guard перед destructive side effects

## Что сейчас правильнее считать статусом проекта

Текущее состояние лучше формулировать так:

- core contracts собраны
- built-in moderation vertical slice собран
- критичные локальные баги по replay/capability/config уже частично зачищены
- runtime integration еще не завершена
- indexed/bucketed router еще не реализован
- production Telegram transport еще не подключен
- scriptable unit execution еще не собран end-to-end

Иными словами:

это уже не пустой skeleton,
но это еще не MVP runtime по целевой routing model.
