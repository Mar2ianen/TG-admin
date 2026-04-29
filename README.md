# telegram-moderation-os

Telegram-бот для модерации чатов с live polling, встроенными командами модерации, аудитом действий, `undo` и поддержкой unit-скриптов на Rhai.

README ниже написан как инструкция по использованию и публикации проекта в текущем состоянии checkout.

## Что умеет бот

- работать в Telegram через long polling
- принимать и обрабатывать команды:
  - `/help`
  - `/ping`
  - `/warn`
  - `/mute`
  - `/ban`
  - `/del`
  - `/undo`
  - `/msg`
- вести аудит выполненных действий в SQLite
- защищаться от повторной обработки одного и того же update
- загружать unit-манифесты и запускать Rhai-скрипты по триггерам

## Что важно понимать сразу

- Публичные команды сейчас только две: `/help` и `/ping`.
- Команды модерации доступны только администраторам Telegram-чата или пользователям из `telegram.admin_user_ids`.
- Для live polling нужен валидный токен бота.
- Бот при старте умеет проверять свою готовность в чатах из `telegram.primary_chat_ids`.
- Встроенная template/UI-инфраструктура в проекте есть, но live transport пока не поддерживает `SendUi/EditUi`, поэтому пользовательские ответы сейчас отправляются обычными сообщениями.

## Быстрый старт

1. Скопируй конфиг:

```sh
cp config.example.toml config.toml
```

2. Заполни минимум:

```toml
[telegram]
polling = true
bot_token = ""
admin_user_ids = [123456789]
primary_chat_ids = [-1001234567890]
```

3. Передай токен одним из способов:

- либо впиши его в `telegram.bot_token`
- либо оставь `bot_token = ""` и задай переменную окружения `TMO_BOT_TOKEN`

Пример:

```sh
export TMO_BOT_TOKEN="123456:ABCDEF"
```

4. Запусти бота:

```sh
cargo run --release
```

Если нужен контейнерный запуск, см. [docs/DEPLOY_RUNBOOK.md](/home/arch/Документы/Teloxide/docs/DEPLOY_RUNBOOK.md:1).

## Как бот запускается

Live polling стартует только если одновременно выполнены оба условия:

- `telegram.polling = true`
- токен задан в `telegram.bot_token` или `TMO_BOT_TOKEN`

Если `polling = false`, бот работает в локальном/noop-режиме без Telegram ingress.

Если `polling = true`, но токен пустой, старт завершится ошибкой. Это fail-closed поведение, а не silent noop.

## Что нужно выдать боту в Telegram

Минимально:

- добавить бота в чат
- выдать права администратора, если хочешь использовать `/warn`, `/mute`, `/ban`, `/del`

Без админских прав бота:

- `/help` и `/ping` должны работать
- модерационные команды будут отклоняться

## Публичные команды

### `/help`

Показывает краткую справку по доступным командам.

Особенности:

- не требует прав администратора
- не зависит от bot-admin bootstrap
- отвечает отдельным сообщением, без `reply_to`

### `/ping`

Проверка, что бот жив и отвечает.

Ожидаемый ответ:

```text
pong
```

Особенности:

- не требует прав администратора
- не зависит от bot-admin bootstrap
- отвечает отдельным сообщением, без `reply_to`

## Команды модерации

Ниже описан реальный поддерживаемый синтаксис текущего parser/executor.

### Как указывать цель

Для `/warn`, `/mute`, `/ban` можно использовать:

- ответ на сообщение пользователя
- `@username`
- числовой user id
- `-user 123456`

Для `/del` цель другая:

- reply на сообщение
- `msg:123`
- `message:123`

Поддерживаемые target-формы из parser:

- `@username`
- `123456`
- `reply`
- `msg:123`
- `message:123`
- JSON selector вида `{"kind":"user","id":42}` или `{"kind":"user","username":"name"}`

### `/warn`

Выдать предупреждение пользователю.

Примеры:

```text
/warn @user спам
/warn 123456789 flood
/warn 2.8
```

Если команда отправлена reply-ем, цель можно не писать:

```text
/warn спам
```

Поддерживаемые флаги:

- `-s` — тихое действие
- `-pub` — отправить публичное уведомление
- `-dry` — dry run
- `-force` — force flag

Что делает:

- увеличивает `warn_count` в базе
- пишет запись в audit log
- может публиковать notice в чат, если указан `-pub`

### `/mute`

Ограничить пользователя на время.

Примеры:

```text
/mute @user 30m флуд
/mute @user 2h caps
/mute @user 7d spam
```

Поддерживаемые единицы времени:

- `s`
- `m`
- `h`
- `d`
- `w`

Примеры валидных duration:

- `30m`
- `2h`
- `7d`
- `1w`

Поддерживаемые флаги:

- `-s`
- `-pub`
- `-dry`
- `-force`

Pipe-поддержка:

```text
/mute @user 30m флуд | /msg Время мута истекло
```

Что делает:

- отправляет restrict в Telegram
- пишет audit entry
- при pipe создаёт scheduled job на follow-up `/msg`

### `/ban`

Заблокировать пользователя.

Примеры:

```text
/ban @user спам
/ban 123456789 scam
```

Поддерживаемые флаги:

- `-s`
- `-pub`
- `-del` — удалить историю сообщений при бане
- `-dry`
- `-force`

Что делает:

- отправляет ban в Telegram
- пишет audit entry
- действие reversible через `/undo`

### `/del`

Удалить сообщения вокруг указанного anchor message.

Примеры:

```text
/del msg:811
/del msg:811 -up 3 -dn 2
/del -up 2 -dn 2
/del msg:811 -user 99
```

Особенности:

- если команда отправлена reply-ем, можно удалять вокруг replied message
- если цель не указана, команда пытается использовать контекст текущего сообщения
- фильтр `-user` должен разрешиться в числовой `user_id`

Поддерживаемые флаги:

- `-up N` — сколько сообщений взять вверх
- `-dn N` — сколько сообщений взять вниз
- `-user ID` — фильтр по пользователю
- `-since 30m` — ограничение по времени
- `-dry`
- `-force`

Что делает:

- выбирает сообщения из journal
- удаляет пачкой через Telegram
- пишет audit entry

### `/undo`

Отменить последнее обратимое действие модерации по текущему контексту.

Пример:

```text
/undo
```

Особенности:

- обычно используется reply-ем в нужном контексте
- требует audit trail
- умеет компенсировать mute и ban
- повторный `undo` того же действия не пройдёт

Поддерживаемые флаги:

- `-dry`
- `-force`

### `/msg`

Отправить обычное текстовое сообщение.

Пример:

```text
/msg Проверка связи
```

Также используется как pipe-команда после `/mute`.

## Порядок обработки прав

Для команд модерации проверяется:

1. отправитель должен быть админом чата или быть в `telegram.admin_user_ids`
2. бот должен иметь админские права в чате для реальных moderation actions

Исключения:

- `/help`
- `/ping`

Они публичные и обходят эти проверки.

## Конфигурация

Полный пример: [config.example.toml](/home/arch/Документы/Teloxide/config.example.toml:1)

### Основное

#### `[telegram]`

- `bot_token`
  - токен бота
  - если пустой, можно использовать `TMO_BOT_TOKEN`
  - default: `None`
- `polling`
  - включать ли live polling
  - default: `true`
- `admin_user_ids`
  - allowlist пользователей, которым разрешены команды модерации
  - default: `[]`
- `primary_chat_ids`
  - список чатов, где бот проверяет свою готовность на старте
  - default: `[]`
- `allowed_webhook_hosts`
  - сейчас в runtime path не используется
  - default: `[]`

#### `[moderation]`

Текущие дефолты:

- `delete_unknown = true`
- `delete_executed = true`
- `delete_targets = true`

Что это значит:

- неизвестные команды по умолчанию считаются удаляемыми
- выполненные moderation-команды считаются удаляемыми
- target cleanup тоже включён по умолчанию

Если не хочешь агрессивного удаления команд, смотри эти флаги в конфиге.

#### `[paths]`

Defaults:

- `data_dir = "data"`
- `database_path = "data/runtime.sqlite3"`
- `units_dir = "units"`
- `scripts_dir = "scripts"`
- `templates_dir = "templates"`
- `log_dir = "data/logs"`

#### `[storage]`

Defaults:

- `sqlite_journal_mode = "WAL"`
- `sqlite_synchronous = "NORMAL"`
- `sqlite_busy_timeout_ms = 5000`

#### `[runtime]`

Defaults:

- `tokio_worker_threads = null`
- `shutdown_grace_period_ms = 5000`
- `reload_enabled = true`
- `manual_mode_enabled = false`
- `degraded_mode_enabled = false`
- `counters.reset_hour = 4`

#### `[ml_server]`

- `base_url = "http://localhost:11434"`

#### `[limits]`

Defaults:

- `max_message_text_bytes = 16384`
- `max_caption_bytes = 4096`
- `max_callback_data_bytes = 256`
- `max_username_bytes = 128`
- `max_units_per_event = 16`
- `max_pipeline_depth = 4`
- `max_batch_ops = 16`
- `max_queue_depth_ingest = 2048`
- `max_queue_depth_dispatch = 1024`

#### `[fetch_policy]`

Defaults:

- `enabled = true`
- `deny_private_ip_ranges = true`
- `deny_localhost = true`
- `max_concurrent_fetches = 32`
- `connect_timeout_ms = 1500`
- `request_timeout_ms = 5000`
- `max_response_body_bytes = 1048576`
- `max_decompressed_body_bytes = 4194304`
- `max_redirects = 3`
- `allowed_domains = []`
- `blocked_domains = []`

#### `[scheduler]`

Defaults:

- `tick_interval_ms = 500`
- `max_concurrent_jobs = 32`
- `max_scheduler_lag_ms = 10000`
- `retry_backoff_base_ms = 1000`
- `retry_backoff_max_ms = 60000`

#### `[observability]`

Defaults:

- `log_level = "info"`
- `json_logs = true`
- `metrics_enabled = true`
- `trace_sampling = "low"`

#### `[features]`

Defaults:

- `hot_reload = true`
- `semantic = true`
- `bloom_prefilter = true`

## Что хранится в базе

SQLite используется для:

- audit log
- processed updates
- message journal
- jobs scheduler
- user state
- KV store

Файл базы по умолчанию:

```text
data/runtime.sqlite3
```

## Unit-скрипты

Бот умеет загружать unit-манифесты и Rhai-скрипты.

Это нужно, если ты хочешь расширять поведение без переписывания core.

### Где лежат unit-файлы

- manifests: `paths.units_dir`
- scripts: `paths.scripts_dir`

### Минимальный manifest

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

### Что уже доступно в скриптах

- `event`
- `ctx_current_json()`
- `db_kv_get(...)`
- `db_kv_set(...)`
- `db_user_get_json(...)`
- `unit_log(...)`
- `unit_warn(...)`
- `load_template(...)`
- `render_auto(...)`

## Дефолтное поведение бота

Если просто запустить проект с дефолтными настройками кода:

- `telegram.polling = true`
- токен обязателен
- `admin_user_ids = []`
- `primary_chat_ids = []`
- логи в JSON
- SQLite в `WAL`
- неизвестные команды удаляются, если включён соответствующий moderation default

Если взять именно `config.example.toml` без изменений:

- `polling = false`
- live Telegram ingress не стартует
- бот будет в локальном/noop-режиме

Это важно: дефолты кода и пример конфига не одно и то же.

## Ограничения текущей версии

- webhook path сейчас не является основным runtime path
- `allowed_webhook_hosts` пока не участвует в реальном runtime
- live `SendUi/EditUi` через `teloxide-core` transport пока не поддержан
- README описывает текущую рабочую execution model, а не будущий roadmap

## Типовой smoke test после запуска

1. Убедись, что бот добавлен в чат.
2. Убедись, что бот админ, если хочешь тестировать модерацию.
3. Отправь:

```text
/ping
```

Ожидаемый ответ:

```text
pong
```

4. Отправь:

```text
/help
```

Ожидаемый ответ:

- русская справка
- список публичных команд
- список команд модерации

5. Проверь простую админ-команду:

```text
/msg Проверка
```

## Разработка

Полезные команды:

```sh
cargo test
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
```

Для деплоя и live-эксплуатации см. [docs/DEPLOY_RUNBOOK.md](/home/arch/Документы/Teloxide/docs/DEPLOY_RUNBOOK.md:1).
